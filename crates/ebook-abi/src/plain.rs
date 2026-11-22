//! 标量 C 面：给 Asuka / Swift / JNI 直接调用的入口。
//!
//! `eb_request` 收发 postcard，适合 cgo 与 Swift（能读返回的缓冲区）；Asuka
//! 的 `extern` 只能拿 `Pointer`（不读内存），所以这里每条能力一个函数，
//! 参数与结果都是标量、字符串、结构体，路径由宿主给，数据写文件。
//!
//! 书用整数句柄：`eb_plain_open` 返回一个 id（`-1` 失败），ASuka 可以把它
//! 放进 state、store 里，不必持有指针。句柄表在本库内部，`eb_plain_close`
//! 释放。调用都应在主线程（宿主本来就那么调）。
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
use std::sync::{LazyLock, Mutex};

use ebook_reader::{Style, Viewport};

use crate::EbEngine;

/// 位图的魔数，平台侧据此拒绝别的文件。
const RAW_MAGIC: &[u8; 4] = b"EQP1";

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

/// 引擎指针：`*mut` 不是 `Send`，句柄表要它。
struct Handle(*mut EbEngine);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

static ENGINES: LazyLock<Mutex<BTreeMap<i32, Handle>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static NEXT_ID: AtomicI32 = AtomicI32::new(1);

thread_local! {
    /// 返回给宿主的字符串：`CString` 得活过返回值，宿主拷走就没人要了。
    /// 只留最近 32 条，返回后立刻拷贝的约定下足够。
    static STRINGS: std::cell::RefCell<VecDeque<CString>> = const {
        std::cell::RefCell::new(VecDeque::new())
    };
}

/// 把字符串留在本线程，返回它的指针。
fn scratch(text: &str) -> *const c_char {
    let clean = if text.contains('\0') {
        text.replace('\0', " ")
    } else {
        text.to_string()
    };
    let c = CString::new(clean).unwrap_or_default();
    STRINGS.with(|slot| {
        let mut strings = slot.borrow_mut();
        strings.push_back(c);
        while strings.len() > 32 {
            strings.pop_front();
        }
        strings
            .back()
            .map_or(std::ptr::null(), |last| last.as_ptr())
    })
}

/// 上手柄：没这个 id、panic，都在这里变成 none / 负数。
fn with_engine<T>(id: i32, f: impl FnOnce(&mut ebook_reader::Engine) -> T) -> Option<T> {
    let pointer = {
        let engines = ENGINES.lock().unwrap_or_else(|poison| poison.into_inner());
        engines.get(&id).map(|handle| handle.0)
    };
    let pointer = pointer?;
    let eb = unsafe { &*pointer };
    let mut guard = eb.inner.lock().unwrap_or_else(|poison| poison.into_inner());
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

/// 最近一次失败的原因（`""` 没有）。指针在本线程有效，直到下一次取字符串。
#[no_mangle]
pub extern "C" fn eb_plain_last_error() -> *const c_char {
    let message =
        catch_unwind(|| crate::LAST_ERROR.with(|slot| slot.borrow().clone().unwrap_or_default()))
            .unwrap_or_default();
    scratch(&message)
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
                let eb = Box::into_raw(Box::new(EbEngine {
                    inner: Mutex::new(engine),
                }));
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
            Some(Handle(pointer)) => {
                drop(unsafe { Box::from_raw(pointer) });
                0
            }
            None => -1,
        }
    })
}

/// 书名。指针在本线程有效，直到下一次取字符串。
#[no_mangle]
pub extern "C" fn eb_plain_title(id: i32) -> *const c_char {
    with_engine(id, |engine| {
        scratch(engine.book().title.as_deref().unwrap_or(""))
    })
    .unwrap_or(std::ptr::null())
}

/// 阅读顺序（spine）的项数。
#[no_mangle]
pub extern "C" fn eb_plain_spine_count(id: i32) -> u32 {
    with_engine(id, |engine| engine.book().spine.len() as u32).unwrap_or(0)
}

/// spine 第 `index` 项的 href（归档内路径）。
#[no_mangle]
pub extern "C" fn eb_plain_spine_href(id: i32, index: u32) -> *const c_char {
    with_engine(id, |engine| {
        engine
            .book()
            .spine
            .get(index as usize)
            .map_or_else(|| scratch(""), |item| scratch(&item.href))
    })
    .unwrap_or(std::ptr::null())
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
    .unwrap_or(std::ptr::null())
}

/// 目录项落在哪一项 spine 上（-1：不在阅读顺序里）。
#[no_mangle]
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
#[no_mangle]
pub extern "C" fn eb_plain_page_info(id: i32, page: u32) -> EbPageInfo {
    let none = EbPageInfo {
        ok: 0,
        width: 0.0,
        height: 0.0,
        progress: -1.0,
    };
    with_engine(id, |engine| {
        engine
            .pages()
            .into_iter()
            .find(|item| item.index == page)
            .map_or(none, |item| EbPageInfo {
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

/// 页内点 → 链接地址（`""` 没有）。指针在本线程有效。
#[no_mangle]
pub extern "C" fn eb_plain_link_at(id: i32, page: u32, x: f32, y: f32) -> *const c_char {
    with_engine(id, |engine| {
        scratch(engine.link_at(page, x, y).unwrap_or(""))
    })
    .unwrap_or(std::ptr::null())
}
