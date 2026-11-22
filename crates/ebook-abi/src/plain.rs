//! 标量 C 面：给 Asuka / Swift / JNI 直接调用的入口。
//!
//! `eb_request` 收发 postcard，适合 cgo 与 Swift（能读返回的缓冲区）；Asuka
//! 的 `extern` 只能拿 `Pointer`（不读内存），所以这里每条能力一个函数，
//! 参数与结果都是标量、字符串、结构体，路径由宿主给，数据写文件。
//!
//! 书用整数句柄：`eb_plain_open` 返回一个 id（`-1` 失败），ASuka 可以把它
//! 放进 state、store 里，不必持有指针。句柄表在本库内部，`eb_plain_close`
//! 释放。
//!
//! 线程：句柄表、字符串环、错误缓冲都是进程级的 `Mutex`，任意线程调用都安全；
//! 同一本书上的调用会被串行化（内部 `Mutex<Engine>`）。返回的字符串指针不是
//! TLS，所以在 A 线程取到、B 线程用也没问题，只要在下一次取字符串前拷走。
//!
//! 唯一不是「无状态」的地方：选区的矩形数组没地方返回，所以 `eb_plain_select`
//! 把最近一次选区留在句柄上，再用 `eb_plain_selection_rect` 按下标取；
//! `eb_plain_resolve_link` 的 fragment 同理。关书时一并丢掉。
//!
//! 位图落盘格式（`eb_plain_page_raw`）：
//!
//! ```text
//! "EQP1" | u32 width | u32 height | width * height * 4 bytes RGBA（直通 alpha）
//! ```
//!
//! 平台侧只读它：iOS 包成 `CGImage`，GTK 包成 `GdkMemoryTexture`。
//!
//! 滚动模式（`eb_plain_layout_scroll`）：整章排成一页，`eb_plain_content_height`
//! 给出内容高度，`eb_plain_page_slice_raw` 按 `y`/`height` 把一片写成同格式的
//! EQP1。宿主只分配瓦片大小的位图，不分配整章。

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_char, CStr, CString};
use std::fs::File;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use ebook_reader::{SelectionResult, Style, Viewport};

use crate::EbEngine;

/// 位图的魔数，平台侧据此拒绝别的文件。
const RAW_MAGIC: &[u8; 4] = b"EQP1";

/// `eb_struct_size` 的 kind，与头文件里的 `EB_STRUCT_*` 一一对应。
pub const EB_STRUCT_PAGE_INFO: u32 = 1;
pub const EB_STRUCT_LOC: u32 = 2;
pub const EB_STRUCT_SELECTION_RECT: u32 = 3;
pub const EB_STRUCT_LINK: u32 = 4;

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

/// 链接解析结果（`extern struct`）。fragment 用 `eb_plain_resolved_fragment` 取。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EbLink {
    /// 1 有，0 没有（外部链接也算「有」：`external` 为 1）。
    pub ok: i32,
    /// 落在哪一项 spine 上（-1：不在阅读顺序里）。
    pub spine: i32,
    /// 1 外部链接，0 书内。
    pub external: i32,
}

/// 句柄表里的所有权。
///
/// 必须是 `Arc` 而不是裸指针：`with_engine` 会在释放句柄表锁之后才去锁
/// `EbEngine::inner`，如果那里拿的是裸指针，并发 `eb_plain_close` 就能在
/// 两次加锁之间把对象释放掉（use-after-free）。`Arc` 让“查表”和“用完”
/// 之间持有一次强引用，close 只负责从表里摘掉，最后一个使用者走完才析构。
struct Handle(Arc<EbEngine>);

/// 句柄之外的状态：最近一次选区与最近一次链接解析。
///
/// 标量面只能返回标量/结构体，选区的矩形数组没地方放，所以在这里留一份，
/// 由 `eb_plain_selection_rect` 按下标取。关书时一并清掉。
#[derive(Default)]
struct Extras {
    selection: Option<SelectionResult>,
    link: Option<Resolved>,
}

/// `resolve_link` 的标量版：结构体带 `spine`/`external`，fragment 单独取。
struct Resolved {
    fragment: Option<String>,
}

static ENGINES: LazyLock<Mutex<BTreeMap<i32, Handle>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static EXTRAS: LazyLock<Mutex<BTreeMap<i32, Extras>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static NEXT_ID: AtomicI32 = AtomicI32::new(1);

/// 返回给宿主的字符串：`CString` 得活过返回值，宿主拷走就没人要了。
///
/// 进程级而不是 thread-local：宿主在哪个线程调都能拿到有效指针，也不依赖
/// "所有调用都在主线程" 的口头约定。只留最近 64 条，所以指针在**后续 64 次
/// 取字符串**前有效（返回后立刻拷贝的约定下绰绰有余）。
static STRINGS: LazyLock<Mutex<VecDeque<CString>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

/// 把字符串留在本进程，返回它的指针。
fn scratch(text: &str) -> *const c_char {
    let clean = if text.contains('\0') {
        text.replace('\0', " ")
    } else {
        text.to_string()
    };
    let c = CString::new(clean).unwrap_or_default();
    let mut strings = STRINGS.lock().unwrap_or_else(|poison| poison.into_inner());
    strings.push_back(c);
    while strings.len() > 64 {
        strings.pop_front();
    }
    strings
        .back()
        .map_or(std::ptr::null(), |last| last.as_ptr())
}

/// 上手柄：没这个 id、panic，都在这里变成 none / 负数。
fn with_engine<T>(id: i32, f: impl FnOnce(&mut ebook_reader::Engine) -> T) -> Option<T> {
    // 先在表锁内克隆一份强引用，出锁后对象至少活到本次调用结束。
    let engine = {
        let engines = ENGINES.lock().unwrap_or_else(|poison| poison.into_inner());
        engines.get(&id).map(|handle| Arc::clone(&handle.0))
    }?;
    let mut guard = engine
        .inner
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    Some(f(&mut guard))
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

/// 跑一段可能 panic 的代码，失败 → -1。
fn guard(f: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        crate::set_last_error("内部 panic");
        -1
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

/// 最近一次失败的原因（`""` 没有）。指针在本进程内一直有效，直到下一次失败。
#[no_mangle]
pub extern "C" fn eb_plain_last_error() -> *const c_char {
    crate::last_error_ptr()
}

// ---------------------------------------------------------------- 书

/// 打开一本书（EPUB 或纯文本，按魔数识别）。返回句柄（>=1），-1 失败，
/// 原因见 `eb_plain_last_error`。
///
/// # Safety
/// `data` 必须可读 `len` 字节。
#[no_mangle]
pub unsafe extern "C" fn eb_plain_open(data: *const u8, len: usize, name: *const c_char) -> i32 {
    guard(|| {
        if data.is_null() || len == 0 {
            crate::set_last_error("空的书籍数据");
            return -1;
        }
        let bytes = std::slice::from_raw_parts(data, len).to_vec();
        let name = c_path(name).unwrap_or_default();
        match ebook_reader::Engine::open(bytes, &name) {
            Ok(engine) => {
                let eb = Arc::new(EbEngine {
                    inner: Mutex::new(engine),
                });
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let mut engines = ENGINES.lock().unwrap_or_else(|poison| poison.into_inner());
                engines.insert(id, Handle(eb));
                crate::set_last_error("");
                id
            }
            Err(error) => {
                crate::set_last_error(format!("{error:?}"));
                -1
            }
        }
    })
}

/// 关掉一本书，释放它。
#[no_mangle]
pub extern "C" fn eb_plain_close(id: i32) -> i32 {
    guard(|| {
        let handle = {
            let mut engines = ENGINES.lock().unwrap_or_else(|poison| poison.into_inner());
            engines.remove(&id)
        };
        match handle {
            Some(Handle(engine)) => {
                // 只摘表；并发中的调用仍持有自己的 `Arc`，这里 drop 不会
                // 提前释放它们正在用的 `EbEngine`。
                drop(engine);
                EXTRAS
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .remove(&id);
                0
            }
            None => -1,
        }
    })
}

/// 书目字段的公共路径：取一个字符串，留在字符串环里。
fn meta(id: i32, pick: impl FnOnce(&mut ebook_reader::Engine) -> Option<String>) -> *const c_char {
    with_engine(id, |engine| {
        let value = pick(engine);
        scratch(value.as_deref().unwrap_or(""))
    })
    .unwrap_or_else(|| scratch(""))
}

/// 书名。指针在本进程内有效，直到后续 64 次取字符串。
#[no_mangle]
pub extern "C" fn eb_plain_title(id: i32) -> *const c_char {
    meta(id, |engine| engine.book().title.clone())
}

/// 语言（`dc:language`，没有时退回 `lang`）。
#[no_mangle]
pub extern "C" fn eb_plain_language(id: i32) -> *const c_char {
    meta(id, |engine| {
        let book = engine.book();
        book.lang.clone().or_else(|| book.language.clone())
    })
}

/// 作者（`dc:creator`）。
#[no_mangle]
pub extern "C" fn eb_plain_creator(id: i32) -> *const c_char {
    meta(id, |engine| engine.book().creator.clone())
}

/// 出版者（`dc:publisher`）。
#[no_mangle]
pub extern "C" fn eb_plain_publisher(id: i32) -> *const c_char {
    meta(id, |engine| engine.book().publisher.clone())
}

/// 标识符（`dc:identifier`，通常是 ISBN/UUID）。
#[no_mangle]
pub extern "C" fn eb_plain_identifier(id: i32) -> *const c_char {
    meta(id, |engine| engine.book().identifier.clone())
}

/// 出版日期（`dc:date`，原始字符串，不做格式解析）。
#[no_mangle]
pub extern "C" fn eb_plain_date(id: i32) -> *const c_char {
    meta(id, |engine| engine.book().date.clone())
}

/// 简介（`dc:description`，可能含原始空白）。
#[no_mangle]
pub extern "C" fn eb_plain_description(id: i32) -> *const c_char {
    meta(id, |engine| engine.book().description.clone())
}

/// 封面在归档内的路径（`""` 没有）。图片本身用 `eb_plain_cover_write`。
#[no_mangle]
pub extern "C" fn eb_plain_cover_href(id: i32) -> *const c_char {
    meta(id, |engine| engine.book().cover_href.clone())
}

/// 封面的 MIME（`image/png`、`image/jpeg`；`""` 没有封面）。
#[no_mangle]
pub extern "C" fn eb_plain_cover_mime(id: i32) -> *const c_char {
    meta(id, |engine| {
        engine.cover().map(|(_, mime)| mime.to_string())
    })
}

/// 封面字节数（0 没有封面）：宿主可据此决定要不要先缓存。
#[no_mangle]
pub extern "C" fn eb_plain_cover_len(id: i32) -> usize {
    with_engine(id, |engine| {
        engine.cover().map_or(0, |(bytes, _)| bytes.len())
    })
    .unwrap_or(0)
}

/// 把封面图片的原始字节写到 `path`（PNG/JPEG，扩展名由 `eb_plain_cover_mime` 定）。
/// 0 成功；-1 没有封面（或句柄无效）；-3 写文件失败。
#[no_mangle]
pub extern "C" fn eb_plain_cover_write(id: i32, path: *const c_char) -> i32 {
    guard(|| {
        let Some(path) = c_path(path) else {
            crate::set_last_error("封面路径为空");
            return -1;
        };
        let bytes = with_engine(id, |engine| engine.cover().map(|(bytes, _)| bytes.to_vec()));
        let Some(Some(bytes)) = bytes else {
            crate::set_last_error("这本书没有封面");
            return -1;
        };
        if let Err(error) = std::fs::write(&path, &bytes) {
            crate::set_last_error(format!("{path}: {error}"));
            return -3;
        }
        0
    })
}

/// 阅读顺序（spine）的项数。
#[no_mangle]
pub extern "C" fn eb_plain_spine_count(id: i32) -> u32 {
    with_engine(id, |engine| engine.book().spine.len() as u32).unwrap_or(0)
}

/// spine 第 `index` 项的 href（归档内路径）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_href(id: i32, index: u32) -> *const c_char {
    spine_field(id, index, |item| &item.href)
}

/// spine 第 `index` 项的 id（manifest 里的 id，宿主做锚点时常需要）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_id(id: i32, index: u32) -> *const c_char {
    spine_field(id, index, |item| &item.id)
}

/// spine 第 `index` 项的 MIME（`application/xhtml+xml`、`text/plain` 等）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_media_type(id: i32, index: u32) -> *const c_char {
    spine_field(id, index, |item| &item.media_type)
}

/// spine 第 `index` 项是否在阅读顺序里（1 是，0 不是/没有这一项）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_linear(id: i32, index: u32) -> i32 {
    with_engine(id, |engine| {
        engine
            .book()
            .spine
            .get(index as usize)
            .map_or(0, |item| i32::from(item.linear))
    })
    .unwrap_or(0)
}

/// spine 项的公共路径：取一个字段并留在字符串环里；越界给空串，不是 null
/// （宿主不必判空）。
fn spine_field(
    id: i32,
    index: u32,
    pick: impl FnOnce(&ebook_format::SpineItem) -> &str,
) -> *const c_char {
    with_engine(id, |engine| {
        engine
            .book()
            .spine
            .get(index as usize)
            .map_or_else(|| scratch(""), |item| scratch(pick(item)))
    })
    .unwrap_or_else(|| scratch(""))
}

/// 目录项数。
#[no_mangle]
pub extern "C" fn eb_plain_toc_count(id: i32) -> u32 {
    with_engine(id, |engine| engine.toc().len() as u32).unwrap_or(0)
}

/// 目录项的标题。
#[no_mangle]
pub extern "C" fn eb_plain_toc_title(id: i32, index: u32) -> *const c_char {
    with_engine(id, |engine| {
        engine
            .toc()
            .get(index as usize)
            .map_or_else(|| scratch(""), |entry| scratch(&entry.label))
    })
    .unwrap_or_else(|| scratch(""))
}

/// 目录项落在哪一项 spine 上（-1：不在阅读顺序里）。#[no_mangle]
pub extern "C" fn eb_plain_toc_spine(id: i32, index: u32) -> i32 {
    with_engine(id, |engine| {
        engine
            .toc()
            .get(index as usize)
            .map_or(-1, |entry| entry.spine as i32)
    })
    .unwrap_or(-1)
}

/// 目录项的层级（0 起算）。
#[no_mangle]
pub extern "C" fn eb_plain_toc_depth(id: i32, index: u32) -> u32 {
    with_engine(id, |engine| {
        engine
            .toc()
            .get(index as usize)
            .map_or(0, |entry| entry.depth as u32)
    })
    .unwrap_or(0)
}

/// 目录项的 href（归档内路径，不含 fragment）。
#[no_mangle]
pub extern "C" fn eb_plain_toc_href(id: i32, index: u32) -> *const c_char {
    with_engine(id, |engine| {
        engine
            .toc()
            .get(index as usize)
            .map_or_else(|| scratch(""), |entry| scratch(&entry.href))
    })
    .unwrap_or_else(|| scratch(""))
}

/// 目录项的 fragment（不带 `#`；`""` 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_toc_fragment(id: i32, index: u32) -> *const c_char {
    with_engine(id, |engine| {
        engine.toc().get(index as usize).map_or_else(
            || scratch(""),
            |entry| scratch(entry.fragment.as_deref().unwrap_or("")),
        )
    })
    .unwrap_or_else(|| scratch(""))
}

// ---------------------------------------------------------------- 字体

/// 注册一份字体（TTF/OTF/TTC 的第一个面）。返回字体 id（>=0），负数失败。
///
/// 排版前必须至少注册一个：没有字体时量出来的行是空的，位图上没有字。
///
/// # Safety
/// `bytes` 必须可读 `len` 字节。
#[no_mangle]
pub unsafe extern "C" fn eb_plain_register_font(id: i32, bytes: *const u8, len: usize) -> i32 {
    guard(|| {
        if bytes.is_null() || len == 0 {
            crate::set_last_error("字体数据为空");
            return -1;
        }
        let data = std::slice::from_raw_parts(bytes, len).to_vec();
        match with_engine(id, |engine| engine.register_font(data)) {
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
pub extern "C" fn eb_plain_register_font_face(id: i32, index: u32) -> i32 {
    guard(
        || match with_engine(id, |engine| engine.register_font_face(index as usize)) {
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
pub extern "C" fn eb_plain_font_count(id: i32) -> u32 {
    with_engine(id, |engine| engine.font_count()).unwrap_or(0)
}

// ---------------------------------------------------------------- 排版

/// 排一章（一章一个 `spine` 下标）。0 好，负数见 `eb_plain_last_error`。
#[no_mangle]
pub extern "C" fn eb_plain_layout(
    id: i32,
    spine: u32,
    width: f32,
    height: f32,
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
    font_scale: f32,
    line_scale: f32,
    letter_px: f32,
    word_px: f32,
    align: u32,
) -> i32 {
    layout_impl(
        id, spine, width, height, top, right, bottom, left, font_scale, line_scale, letter_px,
        word_px, align, false,
    )
}

/// 把一章排成整页滚动流。参数同 `eb_plain_layout`；`height` 只作为视口高
/// 度传入，不参与切页。`top`/`bottom` 应传 0（宿主自己留），`left`/`right`
/// 仍留在页里。排完后 `eb_plain_page_info(id, 0).height` 是内容高度，
/// `eb_plain_page_slice_raw` 按 `y` 取瓦片。
#[no_mangle]
pub extern "C" fn eb_plain_layout_scroll(
    id: i32,
    spine: u32,
    width: f32,
    height: f32,
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
    font_scale: f32,
    line_scale: f32,
    letter_px: f32,
    word_px: f32,
    align: u32,
) -> i32 {
    layout_impl(
        id, spine, width, height, top, right, bottom, left, font_scale, line_scale, letter_px,
        word_px, align, true,
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_impl(
    id: i32,
    spine: u32,
    width: f32,
    height: f32,
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
    font_scale: f32,
    line_scale: f32,
    letter_px: f32,
    word_px: f32,
    align: u32,
    scroll: bool,
) -> i32 {
    guard(|| {
        let mut viewport = Viewport::new(width, height);
        viewport.insets = ebook_reader::EdgesF {
            top,
            right,
            bottom,
            left,
        };
        let style = Style {
            font_scale,
            line_scale,
            letter_px,
            word_px,
            align,
        };
        let result = with_engine(id, |engine| {
            if scroll {
                engine.layout_scroll(spine as usize, viewport, style)
            } else {
                engine.layout(spine as usize, viewport, style)
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
    })
}

/// 最近一次排版的章节下标（-1 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_spine(id: i32) -> i32 {
    with_engine(id, |engine| engine.spine().map_or(-1, |spine| spine as i32)).unwrap_or(-1)
}

/// 排出来的页数。滚动模式下是 1（整章一页）。
#[no_mangle]
pub extern "C" fn eb_plain_page_count(id: i32) -> u32 {
    with_engine(id, |engine| engine.page_count()).unwrap_or(0)
}

/// 滚动模式的内容高度（本章流坐标总长）；分页模式/没排版时 0。
///
/// 宿主用它决定滚动容器的 contentSize；瓦片 y 就在 `[0, height)` 里。
#[no_mangle]
pub extern "C" fn eb_plain_content_height(id: i32) -> f32 {
    with_engine(id, |engine| engine.content_height()).unwrap_or(0.0)
}

/// 一页的逻辑尺寸与本章进度。
///
/// 走 `Engine::page`（按下标直取），不把整本章的页表拉出来：
/// 宿主逐页取元数据是常态，不应该是 O(页数) 每次。
#[no_mangle]
pub extern "C" fn eb_plain_page_info(id: i32, page: u32) -> EbPageInfo {
    let none = EbPageInfo {
        ok: 0,
        width: 0.0,
        height: 0.0,
        progress: -1.0,
    };
    with_engine(id, |engine| {
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
#[no_mangle]
pub extern "C" fn eb_plain_page_raw(
    id: i32,
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
        let bitmap = match with_engine(id, |engine| engine.bitmap(page, scale, ink, paper, bold)) {
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
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn eb_plain_page_slice_raw(
    id: i32,
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
        let bitmap = match with_engine(id, |engine| {
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

/// 把位图写成 `EQP1`：`"EQP1" | u32 width | u32 height | RGBA8`。
fn write_raw(path: &str, bitmap: &ebook_reader::Bitmap) -> i32 {
    let mut file = match File::create(path) {
        Ok(file) => file,
        Err(error) => {
            crate::set_last_error(format!("{path}: {error}"));
            return -3;
        }
    };
    let mut header = [0u8; 12];
    header[..4].copy_from_slice(RAW_MAGIC);
    header[4..8].copy_from_slice(&bitmap.width.to_le_bytes());
    header[8..12].copy_from_slice(&bitmap.height.to_le_bytes());
    if let Err(error) = file
        .write_all(&header)
        .and_then(|()| file.write_all(&bitmap.rgba))
    {
        crate::set_last_error(format!("{path}: {error}"));
        return -3;
    }
    0
}

// ---------------------------------------------------------------- 位置

/// 页内点（点坐标）→ 原文位置。
#[no_mangle]
pub extern "C" fn eb_plain_hit(id: i32, page: u32, x: f32, y: f32) -> EbLoc {
    let none = EbLoc {
        page: -1,
        line: 0,
        byte: 0,
        progress: -1.0,
    };
    with_engine(id, |engine| {
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
pub extern "C" fn eb_plain_progress(id: i32, page: u32, line: u32, byte: u32) -> f32 {
    with_engine(id, |engine| {
        engine
            .progress(ebook_reader::Location { page, line, byte })
            .unwrap_or(-1.0)
    })
    .unwrap_or(-1.0)
}

/// 页内点 → 链接地址（`""` 没有）。指针在本进程内有效，直到后续 64 次取字符串。
#[no_mangle]
pub extern "C" fn eb_plain_link_at(id: i32, page: u32, x: f32, y: f32) -> *const c_char {
    with_engine(id, |engine| {
        scratch(engine.link_at(page, x, y).unwrap_or(""))
    })
    .unwrap_or_else(|| scratch(""))
}

/// 当前章 `#fragment` 的行号；-1 表示本章没有这个锚点（或句柄/排版缺失）。
#[no_mangle]
pub extern "C" fn eb_plain_fragment_line(id: i32, fragment: *const c_char) -> i32 {
    guard(|| {
        let Some(fragment) = c_path(fragment) else {
            crate::set_last_error("fragment 为空");
            return -1;
        };
        match with_engine(id, |engine| {
            engine.fragment_line(fragment.trim_start_matches('#'))
        }) {
            Some(Some(line)) => line as i32,
            Some(None) => -1,
            None => -1,
        }
    })
}

// ---------------------------------------------------------------- 选区

/// 做一次选区，端点按 (`page`, `line`, `byte`) 给。返回矩形块数（>=0）。
///
/// 负数是失败：-1 句柄/排版缺失，-2 端点越界或选区为空。结果留在句柄上，
/// 用 `eb_plain_selection_rect` / `eb_plain_selection_text` 取，下一次
/// `eb_plain_select` 覆盖。
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn eb_plain_select(
    id: i32,
    a_page: u32,
    a_line: u32,
    a_byte: u32,
    b_page: u32,
    b_line: u32,
    b_byte: u32,
) -> i32 {
    guard(|| {
        let location = |page, line, byte| ebook_reader::Location { page, line, byte };
        let selection = with_engine(id, |engine| {
            engine.selection(
                location(a_page, a_line, a_byte),
                location(b_page, b_line, b_byte),
            )
        });
        let mut extras = EXTRAS.lock().unwrap_or_else(|poison| poison.into_inner());
        match selection {
            Some(Some(selection)) if !selection.rects.is_empty() => {
                let count = selection.rects.len() as i32;
                extras.entry(id).or_default().selection = Some(selection);
                count
            }
            Some(_) => {
                // 端点越界、或选区不落在任何行上：给 -2，并把上一份清掉，
                // 不留下一个对不上号的矩形表。
                crate::set_last_error("端点越界，或选区为空");
                extras.entry(id).or_default().selection = None;
                -2
            }
            None => {
                crate::set_last_error("句柄无效");
                -1
            }
        }
    })
}

/// 最近一次选区的矩形块数（没选过 / 关书后 0）。
#[no_mangle]
pub extern "C" fn eb_plain_selection_rect_count(id: i32) -> u32 {
    let extras = EXTRAS.lock().unwrap_or_else(|poison| poison.into_inner());
    extras
        .get(&id)
        .and_then(|extras| extras.selection.as_ref())
        .map_or(0, |selection| selection.rects.len() as u32)
}

/// 最近一次选区的第 `index` 块矩形。`ok == 0` 表示没有这一块。
#[no_mangle]
pub extern "C" fn eb_plain_selection_rect(id: i32, index: u32) -> EbSelectionRect {
    let none = EbSelectionRect {
        ok: 0,
        page: 0,
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };
    let rect = {
        let extras = EXTRAS.lock().unwrap_or_else(|poison| poison.into_inner());
        extras
            .get(&id)
            .and_then(|extras| extras.selection.as_ref())
            .and_then(|selection| selection.rects.get(index as usize))
            .copied()
    };
    match rect {
        Some(rect) => EbSelectionRect {
            ok: 1,
            page: rect.page,
            x: rect.rect.x,
            y: rect.rect.y,
            w: rect.rect.w,
            h: rect.rect.h,
        },
        None => none,
    }
}

/// 最近一次选区的原文（`""` 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_selection_text(id: i32) -> *const c_char {
    let text = {
        let extras = EXTRAS.lock().unwrap_or_else(|poison| poison.into_inner());
        extras
            .get(&id)
            .and_then(|extras| extras.selection.as_ref())
            .map(|selection| selection.text.clone())
    };
    scratch(text.as_deref().unwrap_or(""))
}

/// 选区规范化后的起点（`page == -1` 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_selection_start(id: i32) -> EbLoc {
    selection_endpoint(id, true)
}

/// 选区规范化后的终点（`page == -1` 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_selection_end(id: i32) -> EbLoc {
    selection_endpoint(id, false)
}

fn selection_endpoint(id: i32, start: bool) -> EbLoc {
    let none = EbLoc {
        page: -1,
        line: 0,
        byte: 0,
        progress: -1.0,
    };
    let location = {
        let extras = EXTRAS.lock().unwrap_or_else(|poison| poison.into_inner());
        extras
            .get(&id)
            .and_then(|extras| extras.selection.as_ref())
            .map(|selection| {
                if start {
                    selection.start
                } else {
                    selection.end
                }
            })
    };
    let Some(location) = location else {
        return none;
    };
    with_engine(id, |engine| EbLoc {
        page: location.page as i32,
        line: location.line,
        byte: location.byte,
        progress: engine.progress(location).unwrap_or(-1.0),
    })
    .unwrap_or(none)
}

// ---------------------------------------------------------------- 链接与上限

/// 在最近一次排版的那一章里解析书内链接（href 可以是相对路径或 `#fragment`）。
///
/// `ok == 0` 表示解不出（外部链接也算解出：`external == 1`，`spine == -1`）。
/// fragment 用 `eb_plain_resolved_fragment` 取，下一次解析覆盖。
#[no_mangle]
pub extern "C" fn eb_plain_resolve_link(id: i32, href: *const c_char) -> EbLink {
    let none = EbLink {
        ok: 0,
        spine: -1,
        external: 0,
    };
    let Some(href) = c_path(href) else {
        crate::set_last_error("链接为空");
        return none;
    };
    let resolved = with_engine(id, |engine| engine.resolve_link(&href));
    let Some(resolved) = resolved else {
        return none;
    };
    let Some(target) = resolved else {
        crate::set_last_error("这本书里没有这个链接");
        EXTRAS
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .entry(id)
            .or_default()
            .link = None;
        return none;
    };
    let wire = EbLink {
        ok: 1,
        // 外部链接在 reader 里 `spine` 是 0（相对当前章起算没意义），
        // 标量面统一说 -1，宿主不必再判 `external` 才能用 `spine`。
        spine: if target.external || target.spine == ebook_format::TocEntry::NO_SPINE {
            -1
        } else {
            target.spine as i32
        },
        external: i32::from(target.external),
    };
    EXTRAS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .entry(id)
        .or_default()
        .link = Some(Resolved {
        fragment: target.fragment,
    });
    wire
}

/// 最近一次 `eb_plain_resolve_link` 的 fragment（不带 `#`；`""` 没有）。
#[no_mangle]
pub extern "C" fn eb_plain_resolved_fragment(id: i32) -> *const c_char {
    let fragment = {
        let extras = EXTRAS.lock().unwrap_or_else(|poison| poison.into_inner());
        extras
            .get(&id)
            .and_then(|extras| extras.link.as_ref())
            .and_then(|link| link.fragment.clone())
    };
    scratch(fragment.as_deref().unwrap_or(""))
}

/// 资源上限：单页像素与书级图片缓存（解码后的 RGBA 字节数）。0 好，-1 句柄无效。
///
/// 默认 `4096×4096` / 64 MiB（见 `ebook_reader::Limits`）；手机自己知道内存时
/// 传小一点，比事后再抓 OOM 强。
#[no_mangle]
pub extern "C" fn eb_plain_set_limits(
    id: i32,
    max_page_pixels: u64,
    image_cache_bytes: u64,
) -> i32 {
    guard(|| {
        let limits = ebook_reader::Limits {
            max_page_pixels,
            image_cache_bytes: image_cache_bytes.min(usize::MAX as u64) as usize,
        };
        match with_engine(id, |engine| engine.set_limits(limits)) {
            Some(()) => 0,
            None => -1,
        }
    })
}
