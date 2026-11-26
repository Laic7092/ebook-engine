//! 标量 C 面：给 Asuka / Swift / JNI 直接调用的入口。
//!
//! `eb_request` 收发 postcard，适合 cgo 与 Swift（能读返回的缓冲区）；Asuka
//! 的 `extern` 只能拿 `Pointer`（不读内存），所以这里每条能力一个函数，
//! 参数与结果都是标量、字符串、结构体；数据既可以写文件，也可以走 `_bytes`
//! 直接返回内存。
//!
//! 书用不透明指针 `EbEngine *`：`eb_plain_open` / `eb_plain_new_book` 返回，
//! 失败 NULL；`eb_plain_close` 释放。关闭后不得再使用。
//!
//! 线程：句柄内部是 `Mutex<Engine>`，同一本书上的调用串行化；任意线程调用都
//! 安全。返回的字符串存在句柄自己的只增池里，A 线程取到、B 线程用也可以，
//! 直到句柄关闭。
//!
//! 选区是**调用方拥有的快照**：`eb_plain_select` 返回 `EbSelection *`，用完
//! 调 `eb_plain_selection_free`；链接解析把 fragment 直接放进 `EbLink`。
//! 句柄上只留封面解压缓存，不再留「最近一次选区/链接」。
//!
//! 位图落盘格式（`eb_plain_page_raw`）：
//!
//! ```text
//! "EQP1" | u32 width | u32 height | width * height * 4 bytes RGBA（直通 alpha）
//! ```
//!
//! 平台侧只读它：iOS 包成 `CGImage`，GTK 包成 `GdkMemoryTexture`。
//!
//! 滚动模式（`eb_plain_layout` 的 `scroll = 1`）：整章排成一页，`eb_plain_content_height`
//! 给出内容高度，`eb_plain_page_slice_raw` 按 `y`/`height` 把一片写成同格式的
//! EQP1。宿主只分配瓦片大小的位图，不分配整章。
//!
//! 跨章滚动（`eb_plain_scroll_begin`）：章节起点、全书总高、跨章瓦片
//! （`eb_plain_scroll_band_raw` / `_band_bytes`）、命中与锚点换算都由引擎给，
//! 宿主不用自己维护“章节 top 表”。
//!
//! 所有出位图的入口都收 `ink` / `paper`，是 `0xRRGGBBAA`：`ink == 0` 不修正墨色，
//! `paper == 0` 是白纸。`ink != 0` 只替换**默认字色**（正文主导色，通常就是
//! 书的默认 `color`），书里显式写的链接色、强调色保留；否则夜间主题会把整页
//! 染成一种颜色。
//!
//! 落盘先写 `{path}.{pid}.{seq}.part` 再 `rename`：宿主在别的线程用「文件存在」
//! 判断可用，不能让它读到只写了一半的 `EQP1`。

use std::ffi::{c_char, CStr, CString};
use std::fs::File;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use ebook_reader::{EdgesF, Style, Viewport};

use crate::EbEngine;

/// 位图的魔数，平台侧据此拒绝别的文件。
const RAW_MAGIC: &[u8; 4] = b"EQP1";

/// `eb_struct_size` 的 kind，与头文件里的 `EB_STRUCT_*` 一一对应。
pub const EB_STRUCT_PAGE_INFO: u32 = 1;
pub const EB_STRUCT_LOC: u32 = 2;
pub const EB_STRUCT_SELECTION_RECT: u32 = 3;
pub const EB_STRUCT_LINK: u32 = 4;
pub const EB_STRUCT_SCROLL_CHAPTER: u32 = 5;
pub const EB_STRUCT_SCROLL_LOC: u32 = 6;
pub const EB_STRUCT_METADATA: u32 = 7;
pub const EB_STRUCT_IMAGE_INFO: u32 = 8;
pub const EB_STRUCT_LAYOUT_OPTIONS: u32 = 9;
pub const EB_STRUCT_SCROLL_SLICE_OPTIONS: u32 = 10;
pub const EB_STRUCT_SELECTION: u32 = 11;

pub const EB_CAP_EPUB: u32 = 1;
pub const EB_CAP_TXT: u32 = 2;
pub const EB_CAP_STREAM: u32 = 4;
pub const EB_CAP_IMAGES: u32 = 8;
pub const EB_CAP_FONTS: u32 = 16;

/// 一页的元数据（`extern struct`，按值回传）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbPageInfo {
    /// 1 有，0 没有。
    pub ok: i32,
    /// 内容盒的宽高（点），不含留白。
    pub width: f32,
    pub height: f32,
    /// 本章内进度 0..1。
    pub progress: f32,
}

/// 文本位置（`extern struct`，按值回传）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbLoc {
    /// -1 没有。
    pub page: i32,
    pub line: u32,
    pub byte: u32,
    /// 全书进度 0..1（-1 未知）。
    pub progress: f32,
}

/// 选区里的一块矩形（`extern struct`，按值回传）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbSelectionRect {
    /// 1 有，0 没有（下标越界）。
    pub ok: i32,
    /// 属于哪一页。
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 选区快照（由 `eb_plain_select` 分配，宿主用 `eb_plain_selection_free`
/// 释放）。`text` 是 NUL 结尾的 UTF-8；`rects` 是连续数组。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbSelection {
    pub start: EbLoc,
    pub end: EbLoc,
    pub rect_count: u32,
    /// 原文，归这个快照；不要单独 free。
    pub text: *mut c_char,
    /// `rect_count` 块矩形，归这个快照；不要单独 free。
    pub rects: *mut EbSelectionRect,
}

/// 链接解析结果（`extern struct`）。`fragment` 指向句柄字符串池，直到
/// `eb_plain_close` 前有效；没有 fragment 时是空串。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbLink {
    /// 1 有，0 没有（外部链接也算「有」：`external` 为 1）。
    pub ok: i32,
    /// 落在哪一项 spine 上（-1：不在阅读顺序里）。
    pub spine: i32,
    /// 1 外部链接，0 书内。
    pub external: i32,
    /// 不带 `#`。
    pub fragment: *const c_char,
}

/// 一次取齐的书目信息。所有字符串都指向句柄自己的池，在 `eb_plain_close`
/// 之前一直有效；`ok == 0` 表示句柄无效，其余指针都是空串。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbMetadata {
    pub ok: i32,
    /// 全部 `dc:creator` 的数量；`eb_plain_creator_at` 按 0..count 取。
    pub creator_count: u32,
    pub title: *const c_char,
    /// 第一作者，等价于 `eb_plain_creator_at(engine, 0)`。
    pub creator: *const c_char,
    pub language: *const c_char,
    pub publisher: *const c_char,
    pub identifier: *const c_char,
    pub date: *const c_char,
    pub description: *const c_char,
    pub cover_href: *const c_char,
}

/// 一次排版/滚动会话的全部参数。`size` 由宿主填 `sizeof(EbLayoutOptions)`，
/// `version` 填 `EB_LAYOUT_OPTIONS_VERSION`；以后加字段只增大 `size`。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbLayoutOptions {
    pub size: u32,
    pub version: u32,
    pub spine: u32,
    /// 0 = 分页，1 = 整章一页滚动流。
    pub scroll: u32,
    pub width: f32,
    pub height: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub font_scale: f32,
    pub line_scale: f32,
    pub letter_px: f32,
    pub word_px: f32,
    pub align: u32,
}

pub const EB_LAYOUT_OPTIONS_VERSION: u32 = 1;

/// 排一章并取一个滚动窗口的参数。内嵌完整的 `EbLayoutOptions`。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbScrollSliceOptions {
    pub size: u32,
    pub version: u32,
    pub layout: EbLayoutOptions,
    pub y: f32,
    pub slice_height: f32,
    pub scale: f32,
    pub ink: u32,
    pub paper: u32,
    pub bold: bool,
}

pub const EB_SCROLL_SLICE_OPTIONS_VERSION: u32 = 1;

/// 当前章的一张图片。`id` 就是 `eb_plain_image_full_write` / `_bytes` 的入参。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbImageInfo {
    pub ok: i32,
    pub id: u32,
    /// 固有尺寸；格式头读不出来时是 0。
    pub width: u32,
    pub height: u32,
    pub mime: *const c_char,
    /// 归档内路径，宿主按它取原始字节。
    pub src: *const c_char,
    pub alt: *const c_char,
}

/// 滚动会话里的一章（`extern struct`，按值回传）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbScrollChapter {
    /// 1 这一章排好了；0 排不出来（`height` 按 0 计，不影响后面的章）。
    pub ok: i32,
    /// 章节下标；越界下标回 `u32::MAX`。
    pub spine: u32,
    /// 全书流坐标里的起点（前面所有章的高度之和）。
    pub top: f32,
    /// 内容高度。
    pub height: f32,
}

/// 滚动模式下的命中（`extern struct`，按值回传）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbScrollLoc {
    /// 命中的章；-1 没有。
    pub spine: i32,
    /// 全章行号。
    pub line: u32,
    /// 行内字节偏移。
    pub byte: u32,
    /// 该行行盒顶边在全书流坐标里的位置。
    pub y: f32,
}

/// 句柄上唯一不是「请求-响应」的状态：封面解压缓存。
#[derive(Default)]
pub(crate) struct Extras {
    cover: CoverState,
}

#[derive(Default)]
enum CoverState {
    #[default]
    Unknown,
    None,
    Some(Arc<Vec<u8>>, String),
}

/// 无效/空字符串统一返回这个静态指针，宿主不必判 null。
const EMPTY_STRING: &[u8] = b"\0";

fn empty_c_str() -> *const c_char {
    EMPTY_STRING.as_ptr() as *const c_char
}

/// 上手柄：空指针、panic 都在这里变成 none / 负数。
fn with_engine<T>(
    engine: *mut EbEngine,
    f: impl FnOnce(&mut ebook_reader::Engine) -> T,
) -> Option<T> {
    if engine.is_null() {
        return None;
    }
    // SAFETY: 句柄由 `eb_plain_open` / `eb_plain_new_book` 返回；宿主不得在
    // `eb_plain_close` 之后继续使用它。这是 C ABI 的标准生命周期约定。
    let engine = unsafe { &*engine };
    let mut guard = engine
        .inner
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    Some(f(&mut guard))
}

/// 在句柄自己的字符串池里存一个字符串。指针在 `eb_plain_close` 前一直有效。
fn intern(engine: *mut EbEngine, text: &str) -> *const c_char {
    if engine.is_null() {
        return empty_c_str();
    }
    unsafe { &*engine }.intern(text)
}

fn with_string(
    engine: *mut EbEngine,
    f: impl FnOnce(&mut ebook_reader::Engine) -> Option<String>,
) -> *const c_char {
    let value = with_engine(engine, f).flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

fn c_path(path: *const c_char) -> Option<String> {
    if path.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(path) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

/// 标量面的稳定位置编码：`spine:node:offset`。宿主只当不透明字符串保存；
/// 区间用 `|` 连接两个端点。
fn locator_text(locator: ebook_reader::Locator) -> String {
    format!("{}:{}:{}", locator.spine, locator.node, locator.offset)
}

fn parse_locator(text: &str) -> Option<ebook_reader::Locator> {
    let mut parts = text.split(':');
    Some(ebook_reader::Locator {
        spine: parts.next()?.parse().ok()?,
        node: parts.next()?.parse().ok()?,
        offset: parts.next()?.parse().ok()?,
    })
}

/// 跑一段可能 panic 的代码，失败 → -1。
fn guard(f: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        crate::set_last_error("内部 panic");
        -1
    })
}

/// 返回指针的入口用这个：panic 变成 null。
fn guard_ptr<T>(f: impl FnOnce() -> *mut T) -> *mut T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        crate::set_last_error("内部 panic");
        std::ptr::null_mut()
    })
}

/// 库版本（如 `ebook-abi 0.2.1`）。静态字符串，永不失效。
#[no_mangle]
pub extern "C" fn eb_plain_version() -> *const c_char {
    crate::eb_version()
}

/// 结构布局版本；应与头文件里的 `EB_ABI_VERSION` 一致。
#[no_mangle]
pub extern "C" fn eb_plain_abi_version() -> u32 {
    crate::ABI_VERSION
}

/// 最近一次失败的原因（`""` 没有）。错误串只增不减，旧指针不会失效。
#[no_mangle]
pub extern "C" fn eb_plain_last_error() -> *const c_char {
    crate::last_error_ptr()
}

// ---------------------------------------------------------------- 书

/// 打开一本书（EPUB 或纯文本，按魔数识别）。失败返回 null，原因见
/// `eb_plain_last_error`。
///
/// # Safety
/// `data` 必须可读 `len` 字节；`name` 可以为 null。
#[no_mangle]
pub unsafe extern "C" fn eb_plain_open(
    data: *const u8,
    len: usize,
    name: *const c_char,
) -> *mut EbEngine {
    crate::eb_open(data, len, name)
}

/// 新建一本空的书（宿主按章推送）。失败返回 null。
///
/// # Safety
/// `title` 可以为 null；否则必须是 NUL 结尾的可读字符串。
#[no_mangle]
pub unsafe extern "C" fn eb_plain_new_book(title: *const c_char) -> *mut EbEngine {
    crate::eb_engine_new_book(title)
}

/// 尾部追加一章纯文本。返回新的章数（>=1）；-1 句柄/参数错；
/// -2 不是流式书、空文本或超过单章上限。
#[no_mangle]
pub extern "C" fn eb_plain_append_chapter(
    engine: *mut EbEngine,
    title: *const c_char,
    text: *const c_char,
) -> i32 {
    guard(|| {
        let (Some(title), Some(text)) = (c_path(title), c_path(text)) else {
            crate::set_last_error("章节标题或正文为空");
            return -1;
        };
        match with_engine(engine, |engine| engine.append_chapter(&title, &text)) {
            None => -1,
            Some(Ok(count)) => count as i32,
            Some(Err(error)) => {
                crate::set_last_error(error.to_string());
                -2
            }
        }
    })
}

/// 替换一章。章数与其它章不变；0 成功，-1 句柄/参数错，-2 越界/空文本/超限。
#[no_mangle]
pub extern "C" fn eb_plain_replace_chapter(
    engine: *mut EbEngine,
    spine: u32,
    title: *const c_char,
    text: *const c_char,
) -> i32 {
    guard(|| {
        let (Some(title), Some(text)) = (c_path(title), c_path(text)) else {
            crate::set_last_error("章节标题或正文为空");
            return -1;
        };
        match with_engine(engine, |engine| {
            engine.replace_chapter(spine as usize, &title, &text)
        }) {
            None => -1,
            Some(Ok(())) => 0,
            Some(Err(error)) => {
                crate::set_last_error(error.to_string());
                -2
            }
        }
    })
}

/// 清空流式书的所有章节。0 成功；-1 句柄错；-2 不是流式书。
#[no_mangle]
pub extern "C" fn eb_plain_clear_chapters(engine: *mut EbEngine) -> i32 {
    guard(
        || match with_engine(engine, |engine| engine.clear_chapters()) {
            None => -1,
            Some(Ok(())) => 0,
            Some(Err(error)) => {
                crate::set_last_error(error.to_string());
                -2
            }
        },
    )
}

/// 该章内容版本：append/replace 后变化；0 表示没有这一章或不是流式书。
#[no_mangle]
pub extern "C" fn eb_plain_chapter_rev(engine: *mut EbEngine, spine: u32) -> u64 {
    with_engine(engine, |engine| {
        engine.chapter_rev(spine as usize).unwrap_or(0)
    })
    .unwrap_or(0)
}

/// 导出高度索引到 `path`。返回写入字节数；-1 没有滚动会话/句柄错，-3 写失败。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_index_write(engine: *mut EbEngine, path: *const c_char) -> i32 {
    guard(|| {
        let Some(path) = c_path(path) else {
            return -1;
        };
        let bytes = with_engine(engine, |engine| engine.scroll_index_export().ok());
        let Some(Some(bytes)) = bytes else {
            crate::set_last_error("还没有滚动会话");
            return -1;
        };
        if let Err(error) = std::fs::write(&path, bytes.as_slice()) {
            crate::set_last_error(format!("{path}: {error}"));
            return -3;
        }
        bytes.len() as i32
    })
}

/// 从 `path` 导入高度索引。1 成功，0 键/章数不匹配或文件不可读，-1 句柄/参数错。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_index_read(engine: *mut EbEngine, path: *const c_char) -> i32 {
    guard(|| {
        let Some(path) = c_path(path) else {
            return -1;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return 0;
        };
        match with_engine(engine, |engine| engine.scroll_index_import(&bytes)) {
            None => -1,
            Some(Ok(true)) => 1,
            Some(Ok(false)) => 0,
            Some(Err(_)) => 0,
        }
    })
}

/// 关掉一本书，释放它。0 好，-1 空句柄。关闭后不得再使用这个指针。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn eb_plain_close(engine: *mut EbEngine) -> i32 {
    if engine.is_null() {
        return -1;
    }
    // SAFETY: 调用方保证只关一次。
    unsafe { crate::eb_close(engine) };
    0
}

/// 书目字段的公共路径：把值放进句柄自己的字符串池。
fn meta(
    engine: *mut EbEngine,
    pick: impl FnOnce(&mut ebook_reader::Engine) -> Option<String>,
) -> *const c_char {
    with_string(engine, pick)
}

/// 书名。指针属于句柄，在 `eb_plain_close` 前一直有效。
#[no_mangle]
pub extern "C" fn eb_plain_title(engine: *mut EbEngine) -> *const c_char {
    meta(engine, |engine| engine.book().title.clone())
}

/// 语言（`dc:language`，没有时退回 `lang`）。
#[no_mangle]
pub extern "C" fn eb_plain_language(engine: *mut EbEngine) -> *const c_char {
    meta(engine, |engine| {
        let book = engine.book();
        book.lang.clone().or_else(|| book.language.clone())
    })
}

/// 一次取齐书目：书名、全部作者、语言、出版者、标识符、日期、简介、封面路径。
///
/// 字符串属于句柄，`eb_plain_close` 前一直有效。`ok == 0` 表示句柄无效。
#[no_mangle]
pub extern "C" fn eb_plain_metadata(engine: *mut EbEngine) -> EbMetadata {
    let Some((
        title,
        creator,
        creator_count,
        language,
        publisher,
        identifier,
        date,
        description,
        cover,
    )) = with_engine(engine, |core| {
        let book = core.book();
        let count = if book.creators.is_empty() {
            u32::from(book.creator.is_some())
        } else {
            book.creators.len() as u32
        };
        (
            book.title.clone(),
            book.creator.clone(),
            count,
            book.lang.clone().or_else(|| book.language.clone()),
            book.publisher.clone(),
            book.identifier.clone(),
            book.date.clone(),
            book.description.clone(),
            book.cover_href.clone(),
        )
    })
    else {
        let empty = empty_c_str();
        return EbMetadata {
            ok: 0,
            creator_count: 0,
            title: empty,
            creator: empty,
            language: empty,
            publisher: empty,
            identifier: empty,
            date: empty,
            description: empty,
            cover_href: empty,
        };
    };
    let text = |value: Option<String>| intern(engine, value.as_deref().unwrap_or(""));
    EbMetadata {
        ok: 1,
        creator_count,
        title: text(title),
        creator: text(creator),
        language: text(language),
        publisher: text(publisher),
        identifier: text(identifier),
        date: text(date),
        description: text(description),
        cover_href: text(cover),
    }
}

/// 作者（`dc:creator`，第一作者）。
#[no_mangle]
pub extern "C" fn eb_plain_creator(engine: *mut EbEngine) -> *const c_char {
    eb_plain_creator_at(engine, 0)
}

/// 作者数量。多个 `dc:creator` 全部保留，不再只取第一个。
#[no_mangle]
pub extern "C" fn eb_plain_creator_count(engine: *mut EbEngine) -> u32 {
    with_engine(engine, |core| {
        let book = core.book();
        if book.creators.is_empty() {
            u32::from(book.creator.is_some())
        } else {
            book.creators.len() as u32
        }
    })
    .unwrap_or(0)
}

/// 第 `index` 个作者；越界返回空串。`eb_plain_creator` 是它的 `index == 0`。
#[no_mangle]
pub extern "C" fn eb_plain_creator_at(engine: *mut EbEngine, index: u32) -> *const c_char {
    let value = with_engine(engine, |core| {
        let book = core.book();
        if let Some(creator) = book.creators.get(index as usize) {
            Some(creator.name.clone())
        } else if index == 0 {
            book.creator.clone()
        } else {
            None
        }
    })
    .flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

/// 出版者（`dc:publisher`）。
#[no_mangle]
pub extern "C" fn eb_plain_publisher(engine: *mut EbEngine) -> *const c_char {
    meta(engine, |engine| engine.book().publisher.clone())
}

/// 标识符（`dc:identifier`，通常是 ISBN/UUID）。
#[no_mangle]
pub extern "C" fn eb_plain_identifier(engine: *mut EbEngine) -> *const c_char {
    meta(engine, |engine| engine.book().identifier.clone())
}

/// 出版日期（`dc:date`，原始字符串，不做格式解析）。
#[no_mangle]
pub extern "C" fn eb_plain_date(engine: *mut EbEngine) -> *const c_char {
    meta(engine, |engine| engine.book().date.clone())
}

/// 简介（`dc:description`，可能含原始空白）。
#[no_mangle]
pub extern "C" fn eb_plain_description(engine: *mut EbEngine) -> *const c_char {
    meta(engine, |engine| engine.book().description.clone())
}

/// 封面在归档内的路径（`""` 没有）。图片本身用 `eb_plain_cover_write`。
#[no_mangle]
pub extern "C" fn eb_plain_cover_href(engine: *mut EbEngine) -> *const c_char {
    meta(engine, |engine| engine.book().cover_href.clone())
}

/// 封面只解压一次：MIME、长度、写入共用同一份字节。
fn cover_data(engine: *mut EbEngine) -> Option<(Arc<Vec<u8>>, String)> {
    if engine.is_null() {
        return None;
    }
    let handle = unsafe { &*engine };
    {
        let extras = handle
            .extras
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match &extras.cover {
            CoverState::Unknown => {}
            CoverState::None => return None,
            CoverState::Some(bytes, mime) => return Some((Arc::clone(bytes), mime.clone())),
        }
    }
    let loaded = with_engine(engine, |core| core.cover()).flatten();
    let mut extras = handle
        .extras
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    extras.cover = match loaded {
        Some((bytes, mime)) => CoverState::Some(Arc::new(bytes), mime),
        None => CoverState::None,
    };
    match &extras.cover {
        CoverState::Some(bytes, mime) => Some((Arc::clone(bytes), mime.clone())),
        _ => None,
    }
}

/// 封面的 MIME（`image/png`、`image/jpeg`；`""` 没有封面）。
#[no_mangle]
pub extern "C" fn eb_plain_cover_mime(engine: *mut EbEngine) -> *const c_char {
    let mime = cover_data(engine).map(|(_, mime)| mime);
    intern(engine, mime.as_deref().unwrap_or(""))
}

/// 封面字节数（0 没有封面）：宿主可据此决定要不要先缓存。
#[no_mangle]
pub extern "C" fn eb_plain_cover_len(engine: *mut EbEngine) -> usize {
    cover_data(engine).map_or(0, |(bytes, _)| bytes.len())
}

/// 把封面图片的原始字节写到 `path`（PNG/JPEG，扩展名由 `eb_plain_cover_mime` 定）。
/// 0 成功；-1 没有封面（或句柄无效）；-3 写文件失败。
#[no_mangle]
pub extern "C" fn eb_plain_cover_write(engine: *mut EbEngine, path: *const c_char) -> i32 {
    guard(|| {
        let Some(path) = c_path(path) else {
            crate::set_last_error("封面路径为空");
            return -1;
        };
        let Some((bytes, _)) = cover_data(engine) else {
            crate::set_last_error("这本书没有封面");
            return -1;
        };
        if let Err(error) = std::fs::write(&path, bytes.as_slice()) {
            crate::set_last_error(format!("{path}: {error}"));
            return -3;
        }
        0
    })
}

/// 阅读顺序（spine）的项数。
#[no_mangle]
pub extern "C" fn eb_plain_spine_count(engine: *mut EbEngine) -> u32 {
    with_engine(engine, |engine| engine.book().spine.len() as u32).unwrap_or(0)
}

/// spine 第 `index` 项的 href（归档内路径）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_href(engine: *mut EbEngine, index: u32) -> *const c_char {
    spine_field(engine, index, |item| &item.href)
}

/// spine 第 `index` 项的 id（manifest 里的 id，宿主做锚点时常需要）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_id(engine: *mut EbEngine, index: u32) -> *const c_char {
    spine_field(engine, index, |item| &item.id)
}

/// spine 第 `index` 项的 MIME（`application/xhtml+xml`、`text/plain` 等）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_media_type(engine: *mut EbEngine, index: u32) -> *const c_char {
    spine_field(engine, index, |item| &item.media_type)
}

/// spine 第 `index` 项是否在阅读顺序里（1 是，0 不是/没有这一项）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_linear(engine: *mut EbEngine, index: u32) -> i32 {
    with_engine(engine, |engine| {
        engine
            .book()
            .spine
            .get(index as usize)
            .map_or(0, |item| i32::from(item.linear))
    })
    .unwrap_or(0)
}

/// spine 项的公共路径：取一个字段并放进字符串池；越界给空串，不是 null。
fn spine_field(
    engine: *mut EbEngine,
    index: u32,
    pick: impl FnOnce(&ebook_format::SpineItem) -> &str,
) -> *const c_char {
    let value = with_engine(engine, |core| {
        core.book()
            .spine
            .get(index as usize)
            .map(|item| pick(item).to_string())
    })
    .flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

/// 目录项数。
#[no_mangle]
pub extern "C" fn eb_plain_toc_count(engine: *mut EbEngine) -> u32 {
    with_engine(engine, |engine| engine.toc().len() as u32).unwrap_or(0)
}

/// 目录项的标题。
#[no_mangle]
pub extern "C" fn eb_plain_toc_title(engine: *mut EbEngine, index: u32) -> *const c_char {
    let value = with_engine(engine, |core| {
        core.toc()
            .get(index as usize)
            .map(|entry| entry.label.clone())
    })
    .flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

/// 目录项落在哪一项 spine 上（-1：不在阅读顺序里）。
#[no_mangle]
pub extern "C" fn eb_plain_toc_spine(engine: *mut EbEngine, index: u32) -> i32 {
    with_engine(engine, |engine| {
        engine
            .toc()
            .get(index as usize)
            .map_or(-1, |entry| entry.spine as i32)
    })
    .unwrap_or(-1)
}

/// 目录项的层级（0 起算）。
#[no_mangle]
pub extern "C" fn eb_plain_toc_depth(engine: *mut EbEngine, index: u32) -> u32 {
    with_engine(engine, |engine| {
        engine
            .toc()
            .get(index as usize)
            .map_or(0, |entry| entry.depth as u32)
    })
    .unwrap_or(0)
}

/// 目录项的 href（归档内路径，不含 fragment）。
#[no_mangle]
pub extern "C" fn eb_plain_toc_href(engine: *mut EbEngine, index: u32) -> *const c_char {
    let value = with_engine(engine, |core| {
        core.toc()
            .get(index as usize)
            .map(|entry| entry.href.clone())
    })
    .flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

/// 目录项的 fragment（不带 `#`；`""` 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_toc_fragment(engine: *mut EbEngine, index: u32) -> *const c_char {
    let value = with_engine(engine, |core| {
        core.toc()
            .get(index as usize)
            .and_then(|entry| entry.fragment.clone())
    })
    .flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

/// 注册一份字体（TTF/OTF/TTC 的第一个面）。返回字体 id（>=0），负数失败。
///
/// 排版前必须至少注册一个：没有字体时量出来的行是空的，位图上没有字。
///
/// # Safety
/// `bytes` 必须可读 `len` 字节。
#[no_mangle]
pub unsafe extern "C" fn eb_plain_register_font(
    engine: *mut EbEngine,
    bytes: *const u8,
    len: usize,
) -> i32 {
    guard(|| {
        if bytes.is_null() || len == 0 {
            crate::set_last_error("字体数据为空");
            return -1;
        }
        let data = std::slice::from_raw_parts(bytes, len).to_vec();
        match with_engine(engine, |engine| engine.register_font(data)) {
            Some(Ok(font)) => font.0 as i32,
            Some(Err(message)) => {
                crate::set_last_error(message);
                -2
            }
            None => -1,
        }
    })
}

/// 注册字体集合（TTC/OTC）里的第 `index` 个面，先注册整份文件。
#[no_mangle]
pub extern "C" fn eb_plain_register_font_face(engine: *mut EbEngine, index: u32) -> i32 {
    guard(
        || match with_engine(engine, |engine| engine.register_font_face(index as usize)) {
            Some(Ok(font)) => font.0 as i32,
            Some(Err(message)) => {
                crate::set_last_error(message);
                -2
            }
            None => -1,
        },
    )
}

/// 已注册的字体数。
#[no_mangle]
pub extern "C" fn eb_plain_font_count(engine: *mut EbEngine) -> u32 {
    with_engine(engine, |engine| engine.font_count()).unwrap_or(0)
}

// ---------------------------------------------------------------- 排版

/// 排一章。`options` 必须指向 `EbLayoutOptions`；`size`/`version` 不对返回 -2。
///
/// # Safety
/// `options` 必须可读至少 `size` 字节。
#[no_mangle]
pub unsafe extern "C" fn eb_plain_layout(
    engine: *mut EbEngine,
    options: *const EbLayoutOptions,
) -> i32 {
    guard(|| {
        let Some(options) = layout_options(options) else {
            return -2;
        };
        apply_layout(engine, options)
    })
}

fn layout_options<'a>(options: *const EbLayoutOptions) -> Option<&'a EbLayoutOptions> {
    if options.is_null() {
        crate::set_last_error("layout options 为空");
        return None;
    }
    let options = unsafe { &*options };
    if options.version != EB_LAYOUT_OPTIONS_VERSION
        || options.size < core::mem::size_of::<EbLayoutOptions>() as u32
    {
        crate::set_last_error("layout options 版本或大小不匹配");
        return None;
    }
    Some(options)
}

fn apply_layout(engine: *mut EbEngine, options: &EbLayoutOptions) -> i32 {
    let mut viewport = Viewport::new(options.width, options.height);
    viewport.insets = EdgesF {
        top: options.top,
        right: options.right,
        bottom: options.bottom,
        left: options.left,
    };
    let style = Style {
        font_scale: options.font_scale,
        line_scale: options.line_scale,
        letter_px: options.letter_px,
        word_px: options.word_px,
        align: options.align,
    };
    let result = with_engine(engine, |core| {
        if options.scroll != 0 {
            core.layout_scroll(options.spine as usize, viewport, style)
        } else {
            core.layout(options.spine as usize, viewport, style)
        }
    });
    match result {
        Some(Ok(())) => 0,
        Some(Err(error)) => {
            crate::set_last_error(format!("{error:?}"));
            -2
        }
        None => -1,
    }
}

/// 最近一次排版的章节下标（-1 没有）。名字里带 `last`，避免和
/// `eb_plain_spine_count` 混淆。
#[no_mangle]
pub extern "C" fn eb_plain_last_spine(engine: *mut EbEngine) -> i32 {
    with_engine(engine, |engine| {
        engine.spine().map_or(-1, |spine| spine as i32)
    })
    .unwrap_or(-1)
}

/// 排出来的页数。滚动模式下是 1（整章一页）。
#[no_mangle]
pub extern "C" fn eb_plain_page_count(engine: *mut EbEngine) -> u32 {
    with_engine(engine, |engine| engine.page_count()).unwrap_or(0)
}

/// 滚动模式的内容高度（本章流坐标总长）；分页模式/没排版时 0。
///
/// 宿主用它决定滚动容器的 contentSize；瓦片 y 就在 `[0, height)` 里。
#[no_mangle]
pub extern "C" fn eb_plain_content_height(engine: *mut EbEngine) -> f32 {
    with_engine(engine, |engine| engine.content_height()).unwrap_or(0.0)
}

/// 一页的逻辑尺寸与本章进度。
///
/// 走 `Engine::page`（按下标直取），不把整本章的页表拉出来：
/// 宿主逐页取元数据是常态，不应该是 O(页数) 每次。
#[no_mangle]
pub extern "C" fn eb_plain_page_info(engine: *mut EbEngine, page: u32) -> EbPageInfo {
    let none = EbPageInfo {
        ok: 0,
        width: 0.0,
        height: 0.0,
        progress: -1.0,
    };
    with_engine(engine, |engine| {
        engine.page(page).map_or(none, |item| EbPageInfo {
            ok: 1,
            width: item.width,
            height: item.height,
            progress: item.progress,
        })
    })
    .unwrap_or(none)
}

/// 光栅化一页，写成 `EQP1` 原始位图文件。0 好。
///
/// `ink` / `paper` 见模块文档：`0` = 不修正 / 白纸，`ink` 只替换默认字色。
#[no_mangle]
pub extern "C" fn eb_plain_page_raw(
    engine: *mut EbEngine,
    page: u32,
    scale: f32,
    ink: u32,
    paper: u32,
    bold: bool,
    path: *const c_char,
) -> i32 {
    guard(|| {
        let Some(path) = c_path(path) else {
            crate::set_last_error("位图路径为空");
            return -1;
        };
        let bitmap = match with_engine(engine, |engine| {
            engine.bitmap(page, scale, ink, paper, bold)
        }) {
            Some(Ok(bitmap)) => bitmap,
            Some(Err(error)) => {
                crate::set_last_error(format!("{error:?}"));
                return -2;
            }
            None => return -1,
        };
        write_raw(&path, &bitmap)
    })
}

/// 光栅化滚动流的一个窗口，写成 `EQP1`：`y`/`height` 是点，`scale` 是设备
/// 像素比。滚动模式的瓦片就是它；同一个 y 再来一次会得到逐字节相同的文件。
///
/// `ink` / `paper` 见模块文档：`0` = 不修正 / 白纸，`ink` 只替换默认字色。
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn eb_plain_page_slice_raw(
    engine: *mut EbEngine,
    page: u32,
    y: f32,
    height: f32,
    scale: f32,
    ink: u32,
    paper: u32,
    bold: bool,
    path: *const c_char,
) -> i32 {
    guard(|| {
        let Some(path) = c_path(path) else {
            crate::set_last_error("位图路径为空");
            return -1;
        };
        let bitmap = match with_engine(engine, |engine| {
            engine.bitmap_window(page, y, height, scale, ink, paper, bold)
        }) {
            Some(Ok(bitmap)) => bitmap,
            Some(Err(error)) => {
                crate::set_last_error(format!("{error:?}"));
                return -2;
            }
            None => return -1,
        };
        write_raw(&path, &bitmap)
    })
}

/// 排一章并取一个滚动窗口，一次调用完成。
///
/// 宿主在后台线程取瓦片时，`layout_scroll` 和 `bitmap_window` 必须在同一次
/// 锁里：否则另一次调用可能在这两步之间换掉“最近一次排版”，画出来的就是
/// 另一章。
///
/// # Safety
/// `options` 必须指向 `EbScrollSliceOptions` 且可读 `size` 字节。
#[no_mangle]
pub unsafe extern "C" fn eb_plain_scroll_slice_raw(
    engine: *mut EbEngine,
    options: *const EbScrollSliceOptions,
    path: *const c_char,
) -> i32 {
    guard(|| {
        let Some(path) = c_path(path) else {
            crate::set_last_error("位图路径为空");
            return -1;
        };
        if options.is_null() {
            crate::set_last_error("scroll slice options 为空");
            return -1;
        }
        let options = unsafe { &*options };
        if options.version != EB_SCROLL_SLICE_OPTIONS_VERSION
            || options.size < core::mem::size_of::<EbScrollSliceOptions>() as u32
            || options.layout.version != EB_LAYOUT_OPTIONS_VERSION
            || options.layout.size < core::mem::size_of::<EbLayoutOptions>() as u32
        {
            crate::set_last_error("scroll slice options 版本或大小不匹配");
            return -2;
        }
        let mut viewport = Viewport::new(options.layout.width, options.layout.height);
        viewport.insets = EdgesF {
            top: options.layout.top,
            right: options.layout.right,
            bottom: options.layout.bottom,
            left: options.layout.left,
        };
        let style = Style {
            font_scale: options.layout.font_scale,
            line_scale: options.layout.line_scale,
            letter_px: options.layout.letter_px,
            word_px: options.layout.word_px,
            align: options.layout.align,
        };
        let result = with_engine(engine, |core| {
            core.layout_scroll(options.layout.spine as usize, viewport, style)?;
            core.bitmap_window(
                0,
                options.y,
                options.slice_height,
                options.scale,
                options.ink,
                options.paper,
                options.bold,
            )
        });
        match result {
            Some(Ok(bitmap)) => write_raw(&path, &bitmap),
            Some(Err(error)) => {
                crate::set_last_error(format!("{error:?}"));
                -2
            }
            None => -1,
        }
    })
}

// ---------------------------------------------------------------- 连续滚动

/// 开始（或重设）一次**跨章连续滚动**会话；返回章节数。
///
/// `options.scroll` 忽略，其余 viewport/style 同 `eb_plain_layout`。
/// `-1` 句柄/参数错，`-2` 引擎错。
///
/// # Safety
/// `options` 必须可读至少 `size` 字节。
#[no_mangle]
pub unsafe extern "C" fn eb_plain_scroll_begin(
    engine: *mut EbEngine,
    options: *const EbLayoutOptions,
) -> i32 {
    guard(|| {
        let Some(options) = layout_options(options) else {
            return -2;
        };
        let mut viewport = Viewport::new(options.width, options.height);
        viewport.insets = EdgesF {
            top: options.top,
            right: options.right,
            bottom: options.bottom,
            left: options.left,
        };
        let style = Style {
            font_scale: options.font_scale,
            line_scale: options.line_scale,
            letter_px: options.letter_px,
            word_px: options.word_px,
            align: options.align,
        };
        match with_engine(engine, |core| core.scroll_begin(viewport, style)) {
            Some(Ok(count)) => count as i32,
            Some(Err(error)) => {
                crate::set_last_error(format!("{error:?}"));
                -2
            }
            None => -1,
        }
    })
}

/// 书的章节数（滚动会话的章下标范围就是它；0 句柄/空书）。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_chapter_count(engine: *mut EbEngine) -> u32 {
    with_engine(engine, |engine| engine.chapter_count() as u32).unwrap_or(0)
}

/// 滚动会话里的一章：起点、高度、成没成。越界下标回 `ok == 0`。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_chapter(engine: *mut EbEngine, index: u32) -> EbScrollChapter {
    let none = EbScrollChapter {
        ok: 0,
        spine: u32::MAX,
        top: 0.0,
        height: 0.0,
    };
    with_engine(engine, |engine| {
        engine
            .scroll_chapter(index as usize)
            .map_or(none, |chapter| EbScrollChapter {
                ok: i32::from(chapter.ok),
                spine: chapter.spine as u32,
                top: chapter.top,
                height: chapter.height,
            })
    })
    .unwrap_or(none)
}

/// 全书流坐标总长（滚动容器的 contentSize）。首次调用会量完全书；出错回 `-1`。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_total_height(engine: *mut EbEngine) -> f32 {
    with_engine(engine, |engine| engine.scroll_total_height())
        .and_then(Result::ok)
        .unwrap_or(-1.0)
}

/// 全书流坐标 `y` 落在哪一章（`-1` 超出内容或没有会话）。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_spine_at(engine: *mut EbEngine, y: f32) -> i32 {
    with_engine(engine, |engine| engine.scroll_spine_at(y))
        .and_then(Result::ok)
        .flatten()
        .map_or(-1, |(spine, _)| spine as i32)
}

/// 某一章在全书流坐标里的起点；取不到回 `-1`。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_chapter_top(engine: *mut EbEngine, spine: u32) -> f32 {
    with_engine(engine, |engine| engine.scroll_chapter(spine as usize))
        .and_then(Result::ok)
        .map_or(-1.0, |chapter| chapter.top)
}

/// 光栅化全书流坐标的一段，写成 `EQP1`。可能跨章，章缝不会错行。
///
/// `ink` / `paper` 见模块文档：`0` = 不修正 / 白纸，`ink` 只替换默认字色。
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn eb_plain_scroll_band_raw(
    engine: *mut EbEngine,
    y: f32,
    height: f32,
    scale: f32,
    ink: u32,
    paper: u32,
    bold: bool,
    path: *const c_char,
) -> i32 {
    guard(|| {
        let Some(path) = c_path(path) else {
            crate::set_last_error("位图路径为空");
            return -1;
        };
        match with_engine(engine, |engine| {
            engine.scroll_slice(y, height, scale, ink, paper, bold)
        }) {
            Some(Ok(bitmap)) => write_raw(&path, &bitmap),
            Some(Err(error)) => {
                crate::set_last_error(format!("{error:?}"));
                -2
            }
            None => -1,
        }
    })
}

/// 同上，但把 `EQP1` 字节直接交给宿主（用 `eb_free(ptr, *out_len)` 释放）。
///
/// 能读内存的宿主（Swift/cgo）用它，就不必为了取瓦片先落一个文件、再自己做
/// 文件命名与磁盘缓存。失败回 null 且 `*out_len == 0`。
///
/// `ink` / `paper` 见模块文档：`0` = 不修正 / 白纸，`ink` 只替换默认字色。
///
/// # Safety
/// `out_len` 必须指向可写的一个 `size_t`。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn eb_plain_scroll_band_bytes(
    engine: *mut EbEngine,
    y: f32,
    height: f32,
    scale: f32,
    ink: u32,
    paper: u32,
    bold: bool,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { *out_len = 0 };
    catch_unwind(AssertUnwindSafe(|| {
        match with_engine(engine, |engine| {
            engine.scroll_slice(y, height, scale, ink, paper, bold)
        }) {
            Some(Ok(bitmap)) => {
                let (ptr, len) = crate::into_raw(raw_bytes(&bitmap));
                unsafe { *out_len = len };
                ptr
            }
            Some(Err(error)) => {
                crate::set_last_error(format!("{error:?}"));
                std::ptr::null_mut()
            }
            None => std::ptr::null_mut(),
        }
    }))
    .unwrap_or_else(|_| {
        crate::set_last_error("内部 panic");
        std::ptr::null_mut()
    })
}

/// 全书流坐标 → 原文位置（跨章）。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_hit(engine: *mut EbEngine, x: f32, y: f32) -> EbScrollLoc {
    let none = EbScrollLoc {
        spine: -1,
        line: 0,
        byte: 0,
        y: 0.0,
    };
    with_engine(engine, |engine| engine.scroll_hit(x, y))
        .and_then(Result::ok)
        .flatten()
        .map_or(none, |hit| EbScrollLoc {
            spine: hit.spine as i32,
            line: hit.line,
            byte: hit.byte,
            y: hit.y,
        })
}

/// 行号 → 全书流坐标（行盒顶边）；取不到回 `-1`。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_line_y(engine: *mut EbEngine, spine: u32, line: u32) -> f32 {
    with_engine(engine, |engine| engine.scroll_line_y(spine as usize, line))
        .and_then(Result::ok)
        .flatten()
        .unwrap_or(-1.0)
}

/// `#fragment` 锚点 → 全书流坐标；这一章没有这个锚点时回 `-1`。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_anchor_y(
    engine: *mut EbEngine,
    spine: u32,
    fragment: *const c_char,
) -> f32 {
    let Some(fragment) = c_path(fragment) else {
        return -1.0;
    };
    with_engine(engine, |engine| {
        engine.scroll_anchor_y(spine as usize, &fragment)
    })
    .and_then(Result::ok)
    .flatten()
    .unwrap_or(-1.0)
}

/// 全书流坐标 → 滚动进度 0..=1；出错回 `-1`。首次调用会量完全书。
#[no_mangle]
pub extern "C" fn eb_plain_scroll_progress(engine: *mut EbEngine, y: f32) -> f32 {
    with_engine(engine, |engine| engine.scroll_progress(y))
        .and_then(Result::ok)
        .unwrap_or(-1.0)
}

/// `EQP1` 字节：`"EQP1" | u32 width | u32 height | RGBA8`。
fn eqp1_bytes(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + rgba.len());
    out.extend_from_slice(RAW_MAGIC);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(rgba);
    out
}

fn raw_bytes(bitmap: &ebook_reader::Bitmap) -> Vec<u8> {
    eqp1_bytes(bitmap.width, bitmap.height, &bitmap.rgba)
}

/// 当前章的图片数量。配合 `eb_plain_image_info` 枚举，`image_id` 就是下标。
#[no_mangle]
pub extern "C" fn eb_plain_image_count(engine: *mut EbEngine) -> u32 {
    with_engine(engine, |core| core.images().len() as u32).unwrap_or(0)
}

/// 当前章第 `index` 张图片的元数据。`ok == 0` 表示越界或没排版。
#[no_mangle]
pub extern "C" fn eb_plain_image_info(engine: *mut EbEngine, index: u32) -> EbImageInfo {
    let empty = empty_c_str();
    let Some((width, height, mime, src, alt)) = with_engine(engine, |core| {
        let image = core.images().get(index as usize)?;
        Some((
            image.size.map_or(0, |size| size.w.max(0.0) as u32),
            image.size.map_or(0, |size| size.h.max(0.0) as u32),
            image.mime.clone(),
            image.src.clone(),
            image.alt.clone(),
        ))
    })
    .flatten() else {
        return EbImageInfo {
            ok: 0,
            id: index,
            width: 0,
            height: 0,
            mime: empty,
            src: empty,
            alt: empty,
        };
    };
    EbImageInfo {
        ok: 1,
        id: index,
        width,
        height,
        mime: intern(engine, mime.as_deref().unwrap_or("")),
        src: intern(engine, src.as_deref().unwrap_or("")),
        alt: intern(engine, alt.as_deref().unwrap_or("")),
    }
}

/// `EQP1` 字节：`"EQP1" | u32 width | u32 height | RGBA8`。宿主用 `eb_free` 释放。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn eb_plain_image_full_bytes(
    engine: *mut EbEngine,
    image_id: u32,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { *out_len = 0 };
    let Some(image) = with_engine(engine, |core| core.image_full(image_id as u16)).flatten() else {
        return std::ptr::null_mut();
    };
    let (ptr, len) = crate::into_raw(eqp1_bytes(image.w, image.h, &image.rgba));
    unsafe { *out_len = len };
    ptr
}

/// 当前页的 `EQP1` 字节。宿主用 `eb_free` 释放。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn eb_plain_page_bytes(
    engine: *mut EbEngine,
    page: u32,
    scale: f32,
    ink: u32,
    paper: u32,
    bold: bool,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { *out_len = 0 };
    let Some(bitmap) =
        with_engine(engine, |core| core.bitmap(page, scale, ink, paper, bold)).and_then(Result::ok)
    else {
        return std::ptr::null_mut();
    };
    let (ptr, len) = crate::into_raw(raw_bytes(&bitmap));
    unsafe { *out_len = len };
    ptr
}

/// 当前页一段 `EQP1` 字节（滚动分页模式）。宿主用 `eb_free` 释放。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn eb_plain_page_slice_bytes(
    engine: *mut EbEngine,
    page: u32,
    y: f32,
    height: f32,
    scale: f32,
    ink: u32,
    paper: u32,
    bold: bool,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { *out_len = 0 };
    let Some(bitmap) = with_engine(engine, |core| {
        core.bitmap_window(page, y, height, scale, ink, paper, bold)
    })
    .and_then(Result::ok) else {
        return std::ptr::null_mut();
    };
    let (ptr, len) = crate::into_raw(raw_bytes(&bitmap));
    unsafe { *out_len = len };
    ptr
}

/// 当前后端的能力位。宿主据此决定显示哪些入口，不用猜。
#[no_mangle]
pub extern "C" fn eb_plain_capabilities(engine: *mut EbEngine) -> u32 {
    with_engine(engine, |core| {
        let mut caps = match core.format() {
            "epub" => EB_CAP_EPUB,
            "stream" => EB_CAP_STREAM,
            _ => EB_CAP_TXT,
        };
        if !core.book().images.is_empty() {
            caps |= EB_CAP_IMAGES;
        }
        if !core.book().fonts.is_empty() {
            caps |= EB_CAP_FONTS;
        }
        caps
    })
    .unwrap_or(0)
}

/// 把当前章的一张图片按原分辨率写成 `EQP1`（`"EQP1" | u32 w | u32 h | RGBA8`）。
///
/// 页面渲染走降采样缓存；这条路径只给“点击查看原图”用，不改变页面。
/// 0 好，-1 图片不存在，-3 写失败。
#[no_mangle]
pub extern "C" fn eb_plain_image_full_write(
    engine: *mut EbEngine,
    image_id: u32,
    path: *const c_char,
) -> i32 {
    let Some(path) = c_path(path) else {
        return -3;
    };
    let Some(image) = with_engine(engine, |engine| engine.image_full(image_id as u16)).flatten()
    else {
        return -1;
    };
    write_image_raw(&path, image.w, image.h, &image.rgba)
}

fn write_image_raw(path: &str, width: u32, height: u32, rgba: &[u8]) -> i32 {
    use std::sync::atomic::AtomicU64;

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = format!("{path}.{}.{seq}.part", std::process::id());
    let write = || -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(RAW_MAGIC)?;
        file.write_all(&width.to_le_bytes())?;
        file.write_all(&height.to_le_bytes())?;
        file.write_all(rgba)?;
        Ok(())
    };
    let result = write().and_then(|()| std::fs::rename(&tmp, path));
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        crate::set_last_error(format!("{path}: {error}"));
        return -3;
    }
    0
}

/// 把位图写成 `EQP1` 文件：先写同目录下的唯一临时文件，再 `rename` 成目标路径。
///
/// 宿主用「文件存在」判断瓦片可用，直接 `File::create(path)` 会让它读到只写了
/// 一半的位图（魔数与宽高对、像素不够），平台侧解码失败就是一屏纸色。
/// 临时名带进程号与自增序号：并发写同一个目标路径时不会互相踩。
fn write_raw(path: &str, bitmap: &ebook_reader::Bitmap) -> i32 {
    use std::sync::atomic::AtomicU64;

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = format!("{path}.{}.{seq}.part", std::process::id());

    let write = || -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(&raw_bytes(bitmap))?;
        // 这里不需要 `sync_all`：读者看的是同一份页缓存，`rename` 之后看到的就是
        // 完整的文件；崩溃持久化不是这个函数的职责。
        Ok(())
    };
    let result = write().and_then(|()| std::fs::rename(&tmp, path));
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        crate::set_last_error(format!("{path}: {error}"));
        return -3;
    }
    0
}

// ---------------------------------------------------------------- 位置

/// 页内点（点坐标）→ 原文位置。
#[no_mangle]
pub extern "C" fn eb_plain_hit(engine: *mut EbEngine, page: u32, x: f32, y: f32) -> EbLoc {
    let none = EbLoc {
        page: -1,
        line: 0,
        byte: 0,
        progress: -1.0,
    };
    with_engine(engine, |engine| {
        engine.hit(page, x, y).map_or(none, |location| EbLoc {
            page: location.page as i32,
            line: location.line,
            byte: location.byte,
            progress: engine.progress(location).unwrap_or(-1.0),
        })
    })
    .unwrap_or(none)
}

/// 位置 → 全书进度 0..1（-1 未知）。
#[no_mangle]
pub extern "C" fn eb_plain_progress(engine: *mut EbEngine, page: u32, line: u32, byte: u32) -> f32 {
    with_engine(engine, |engine| {
        engine
            .progress(ebook_reader::Location { page, line, byte })
            .unwrap_or(-1.0)
    })
    .unwrap_or(-1.0)
}

/// 页内点 → 稳定位置（`spine:node:offset`；`""` 没有）。重排后仍有效。
#[no_mangle]
pub extern "C" fn eb_plain_locator_at(
    engine: *mut EbEngine,
    page: u32,
    x: f32,
    y: f32,
) -> *const c_char {
    let value = with_engine(engine, |core| core.locator_at(page, x, y).map(locator_text)).flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

/// 两个页内位置 → 稳定区间（`a|b`；`""` 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_locator_range(
    engine: *mut EbEngine,
    a_page: u32,
    a_line: u32,
    a_byte: u32,
    b_page: u32,
    b_line: u32,
    b_byte: u32,
) -> *const c_char {
    let a = ebook_reader::Location {
        page: a_page,
        line: a_line,
        byte: a_byte,
    };
    let b = ebook_reader::Location {
        page: b_page,
        line: b_line,
        byte: b_byte,
    };
    let value = with_engine(engine, |core| {
        core.locator_range(a, b)
            .map(|range| format!("{}|{}", locator_text(range.start), locator_text(range.end)))
    })
    .flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

/// 稳定位置（`spine:node:offset`）→ 当前排版的页/行/字节。
#[no_mangle]
pub extern "C" fn eb_plain_locate(engine: *mut EbEngine, locator: *const c_char) -> EbLoc {
    let none = EbLoc {
        page: -1,
        line: 0,
        byte: 0,
        progress: -1.0,
    };
    let Some(locator) = c_path(locator).and_then(|text| parse_locator(&text)) else {
        return none;
    };
    with_engine(engine, |engine| {
        engine
            .locate_locator(&locator)
            .map_or(none, |location| EbLoc {
                page: location.page as i32,
                line: location.line,
                byte: location.byte,
                progress: engine.progress(location).unwrap_or(-1.0),
            })
    })
    .unwrap_or(none)
}

/// 页内点 → 链接地址（`""` 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_link_at(
    engine: *mut EbEngine,
    page: u32,
    x: f32,
    y: f32,
) -> *const c_char {
    let value = with_engine(engine, |core| core.link_at(page, x, y).map(str::to_string)).flatten();
    intern(engine, value.as_deref().unwrap_or(""))
}

/// 当前章 `#fragment` 的行号；-1 表示本章没有这个锚点（或句柄/排版缺失）。
#[no_mangle]
pub extern "C" fn eb_plain_fragment_line(engine: *mut EbEngine, fragment: *const c_char) -> i32 {
    guard(|| {
        let Some(fragment) = c_path(fragment) else {
            crate::set_last_error("fragment 为空");
            return -1;
        };
        match with_engine(engine, |engine| {
            engine.fragment_line(fragment.trim_start_matches('#'))
        }) {
            Some(Some(line)) => line as i32,
            Some(None) => -1,
            None => -1,
        }
    })
}

// ---------------------------------------------------------------- 选区

/// 做一次选区，端点按 (`page`, `line`, `byte`) 给。成功返回一个由调用方
/// 拥有的快照，用完必须 `eb_plain_selection_free`；失败返回 NULL，原因见
/// `eb_plain_last_error`。端点顺序无所谓，库会规范化。
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn eb_plain_select(
    engine: *mut EbEngine,
    a_page: u32,
    a_line: u32,
    a_byte: u32,
    b_page: u32,
    b_line: u32,
    b_byte: u32,
) -> *mut EbSelection {
    guard_ptr(|| {
        let location = |page, line, byte| ebook_reader::Location { page, line, byte };
        let selection = with_engine(engine, |core| {
            core.selection(
                location(a_page, a_line, a_byte),
                location(b_page, b_line, b_byte),
            )
        });
        let Some(Some(selection)) = selection else {
            crate::set_last_error(if selection.is_none() {
                "句柄无效"
            } else {
                "端点越界，或选区为空"
            });
            return std::ptr::null_mut();
        };
        if selection.rects.is_empty() {
            crate::set_last_error("端点越界，或选区为空");
            return std::ptr::null_mut();
        }

        let endpoints = with_engine(engine, |core| {
            let start = EbLoc {
                page: selection.start.page as i32,
                line: selection.start.line,
                byte: selection.start.byte,
                progress: core.progress(selection.start).unwrap_or(-1.0),
            };
            let end = EbLoc {
                page: selection.end.page as i32,
                line: selection.end.line,
                byte: selection.end.byte,
                progress: core.progress(selection.end).unwrap_or(-1.0),
            };
            (start, end)
        })
        .unwrap_or_else(|| {
            (
                EbLoc {
                    page: -1,
                    line: 0,
                    byte: 0,
                    progress: -1.0,
                },
                EbLoc {
                    page: -1,
                    line: 0,
                    byte: 0,
                    progress: -1.0,
                },
            )
        });

        let text = CString::new(selection.text.replace('\0', ""))
            .unwrap_or_default()
            .into_raw();
        let rects: Vec<EbSelectionRect> = selection
            .rects
            .iter()
            .map(|rect| EbSelectionRect {
                ok: 1,
                page: rect.page,
                x: rect.rect.x,
                y: rect.rect.y,
                w: rect.rect.w,
                h: rect.rect.h,
            })
            .collect();
        let rect_count = rects.len() as u32;
        let rects = Box::into_raw(rects.into_boxed_slice()) as *mut EbSelectionRect;

        Box::into_raw(Box::new(EbSelection {
            start: endpoints.0,
            end: endpoints.1,
            rect_count,
            text,
            rects,
        }))
    })
}

/// 释放 `eb_plain_select` 返回的选区。NULL 安全；同一快照只能释放一次。
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn eb_plain_selection_free(selection: *mut EbSelection) {
    if selection.is_null() {
        return;
    }
    let selection = unsafe { Box::from_raw(selection) };
    if !selection.text.is_null() {
        drop(unsafe { CString::from_raw(selection.text) });
    }
    if !selection.rects.is_null() {
        let rects =
            std::ptr::slice_from_raw_parts_mut(selection.rects, selection.rect_count as usize);
        drop(unsafe { Box::from_raw(rects) });
    }
}

// ---------------------------------------------------------------- 链接与上限

/// 在最近一次排版的那一章里解析书内链接（href 可以是相对路径或 `#fragment`）。
///
/// `ok == 0` 表示解不出（外部链接也算解出：`external == 1`，`spine == -1`）。
/// `fragment` 直接放在返回值里，不带 `#`，指向句柄字符串池。
#[no_mangle]
pub extern "C" fn eb_plain_resolve_link(engine: *mut EbEngine, href: *const c_char) -> EbLink {
    let none = EbLink {
        ok: 0,
        spine: -1,
        external: 0,
        fragment: empty_c_str(),
    };
    let Some(href) = c_path(href) else {
        crate::set_last_error("链接为空");
        return none;
    };
    let resolved = with_engine(engine, |core| core.resolve_link(&href));
    let Some(resolved) = resolved else {
        return none;
    };
    let Some(target) = resolved else {
        crate::set_last_error("这本书里没有这个链接");
        return none;
    };
    EbLink {
        ok: 1,
        // 外部链接在 reader 里 `spine` 是 0（相对当前章起算没意义），
        // 标量面统一说 -1，宿主不必再判 `external` 才能用 `spine`。
        spine: if target.external || target.spine == ebook_format::TocEntry::NO_SPINE {
            -1
        } else {
            target.spine as i32
        },
        external: i32::from(target.external),
        fragment: intern(engine, target.fragment.as_deref().unwrap_or("")),
    }
}

/// 释放可再生的缓存：跨章 IR、断行缓存、图片 RGBA、字体位图。
///
/// 当前页 IR 保留，调用后仍可继续绘制；适合 iOS/Android 的内存警告。
/// 0 好，-1 句柄无效。
#[no_mangle]
pub extern "C" fn eb_plain_trim_memory(engine: *mut EbEngine) -> i32 {
    guard(
        || match with_engine(engine, |engine| engine.trim_memory()) {
            Some(()) => 0,
            None => -1,
        },
    )
}

/// 当前缓存占用的近似字节数；0 表示句柄无效或没有缓存。
#[no_mangle]
pub extern "C" fn eb_plain_memory_bytes(engine: *mut EbEngine) -> u64 {
    with_engine(engine, |engine| engine.memory_usage().total_bytes as u64).unwrap_or(0)
}

/// 资源上限：单页像素与书级图片缓存（解码后的 RGBA 字节数）。0 好，-1 句柄无效。
///
/// 默认 `4096×4096` / 24 MiB（见 `ebook_reader::Limits`）；手机自己知道内存时
/// 传小一点，比事后再抓 OOM 强。
#[no_mangle]
pub extern "C" fn eb_plain_set_limits(
    engine: *mut EbEngine,
    max_page_pixels: u64,
    image_cache_bytes: u64,
) -> i32 {
    guard(|| {
        let limits = ebook_reader::Limits {
            max_page_pixels,
            image_cache_bytes: image_cache_bytes.min(usize::MAX as u64) as usize,
        };
        match with_engine(engine, |engine| engine.set_limits(limits)) {
            Some(()) => 0,
            None => -1,
        }
    })
}
