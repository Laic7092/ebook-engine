//! C ABI：给 cgo / Swift / JNI 用的最小面。
//!
//! 头文件是 `include/ebook_abi.h`（`tests/header.rs` 保证它与这里的
//! `pub extern "C" fn` 及 `repr(C)` 结构逐一对得上）。
//!
//! 设计原则：
//! - **有状态但只关于书**：`eb_open` 返回一个不透明句柄，句柄内部是
//!   `Mutex<Engine>`；viewport/style 这类宿主设置不进 ABI，跟着请求走。
//! - **一条通用入口**：`eb_request` 收发 postcard 编码的 `Request`/`Response`。
//!   新增能力只改 `protocol.rs`，不用再往 C 面上加函数。
//! - **缓冲区归调用方**：`eb_request` 返回的指针必须用 `eb_free` 释放。
//! - **panic 不出边界**：所有入口都 `catch_unwind`，panic 变成 `Error` 响应，
//!   而不是让整个 App abort。这条依赖 native profile 里的 `panic = "unwind"`。

pub mod plain;
pub mod protocol;

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{LazyLock, Mutex};

use ebook_reader::EdgesF;
use ebook_reader::{Engine, Limits, Location, SelectionRect as CoreRect, Style, Viewport};

use protocol::{
    Book, FontFace, Insets, Link, Loc, Page, Request, Response, Selection, SelectionRect,
    SpineItem, TocEntry,
};

/// `repr(C)` 结构体与函数签名的布局版本。
///
/// 改任何 `#[repr(C)]` 结构、返回值形状或既有函数参数就 +1；头文件里的
/// `EB_ABI_VERSION` 必须与之一致。宿主在启动时比对 `eb_abi_version()`，
/// 不一致就别调，而不是把字段读串。
///
/// 只加新函数、新枚举变体不算破坏（句柄、缓冲区契约不变），但结构体加字段
/// 一定算：C 是按偏移读的。
pub const ABI_VERSION: u32 = 1;

/// 不透明句柄。内部加锁，因为 Go/Swift 侧不保证所有调用都在同一个线程上。
pub struct EbEngine {
    inner: Mutex<Engine>,
}

/// 最后一个错误的细节。进程级、UTF-8、以结尾 0 收尾的 `Vec<u8>`。
///
/// 用 `Vec` 而不是 `CString`：`set_last_error` 复用容量，连续失败不会每次
/// 换一块内存（早期版本对 `Vec` 做 `mem::forget`，每次失败漏一条字符串）。
/// 指针在**下一次失败的调用**前有效；不归调用方释放。
static LAST_ERROR: LazyLock<Mutex<Vec<u8>>> = LazyLock::new(|| Mutex::new(vec![0]));

pub(crate) fn set_last_error(message: impl AsRef<str>) {
    let text = message.as_ref();
    let mut buffer = LAST_ERROR
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    buffer.clear();
    buffer.extend_from_slice(text.replace('\0', " ").as_bytes());
    buffer.push(0);
}

/// 错误串的裸指针（含结尾 0）。原地复用缓冲，所以可以随便返回。
pub(crate) fn last_error_ptr() -> *const c_char {
    let buffer = LAST_ERROR
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    buffer.as_ptr() as *const c_char
}

/// 同上，外加不含结尾 0 的长度，一次加锁取齐（长度与指针必须同源）。
pub(crate) fn last_error_with_len() -> (*const c_char, usize) {
    let buffer = LAST_ERROR
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    (
        buffer.as_ptr() as *const c_char,
        buffer.len().saturating_sub(1),
    )
}

// ---------------------------------------------------------------- 缓冲区

/// 分配 `len` 字节交给宿主写入请求。
#[no_mangle]
pub extern "C" fn eb_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    let boxed = vec![0u8; len].into_boxed_slice();
    Box::into_raw(boxed) as *mut u8
}

/// 释放 `eb_alloc` / `eb_request` 返回的缓冲区。
///
/// # Safety
/// `ptr` 必须来自本库的 `eb_alloc` 或 `eb_request`，且 `len` 与当初一致。
#[no_mangle]
pub unsafe extern "C" fn eb_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
}

pub(crate) fn into_raw(bytes: Vec<u8>) -> (*mut u8, usize) {
    if bytes.is_empty() {
        return (std::ptr::null_mut(), 0);
    }
    let len = bytes.len();
    let boxed = bytes.into_boxed_slice();
    (Box::into_raw(boxed) as *mut u8, len)
}

// ---------------------------------------------------------------- 生命周期

#[no_mangle]
pub extern "C" fn eb_version() -> *const c_char {
    // 静态、含结尾 0、进程生命周期内有效。
    const BYTES: &[u8] = concat!("ebook-abi ", env!("CARGO_PKG_VERSION"), "\0").as_bytes();
    BYTES.as_ptr() as *const c_char
}

/// 结构布局版本，见 [`ABI_VERSION`]。宿主应与头文件里的 `EB_ABI_VERSION` 比对。
#[no_mangle]
pub extern "C" fn eb_abi_version() -> u32 {
    ABI_VERSION
}

/// `repr(C)` 结构体的字节数，供宿主校验头文件与本库同源。`kind` 见头文件
/// （1 `EbPageInfo`、2 `EbLoc`、3 `EbSelectionRect`、4 `EbLink`）；未知返回 0。
#[no_mangle]
pub extern "C" fn eb_struct_size(kind: u32) -> usize {
    match kind {
        plain::EB_STRUCT_PAGE_INFO => std::mem::size_of::<plain::EbPageInfo>(),
        plain::EB_STRUCT_LOC => std::mem::size_of::<plain::EbLoc>(),
        plain::EB_STRUCT_SELECTION_RECT => std::mem::size_of::<plain::EbSelectionRect>(),
        plain::EB_STRUCT_LINK => std::mem::size_of::<plain::EbLink>(),
        plain::EB_STRUCT_SCROLL_CHAPTER => std::mem::size_of::<plain::EbScrollChapter>(),
        plain::EB_STRUCT_SCROLL_LOC => std::mem::size_of::<plain::EbScrollLoc>(),
        _ => 0,
    }
}

/// 打开一本书（EPUB 或文本，按魔数识别）。失败返回 null，细节见 `eb_last_error`。
///
/// # Safety
/// `bytes` 必须指向 `len` 字节可读内存；`name` 可以为 null。
#[no_mangle]
pub unsafe extern "C" fn eb_open(
    bytes: *const u8,
    len: usize,
    name: *const c_char,
) -> *mut EbEngine {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if bytes.is_null() || len == 0 {
            return Err("空的书籍数据".to_string());
        }
        let data = std::slice::from_raw_parts(bytes, len).to_vec();
        let name = if name.is_null() {
            String::new()
        } else {
            CStr::from_ptr(name).to_string_lossy().into_owned()
        };
        Engine::open(data, &name).map_err(|error| format!("{error:?}"))
    }));

    match result {
        Ok(Ok(engine)) => {
            set_last_error("");
            Box::into_raw(Box::new(EbEngine {
                inner: Mutex::new(engine),
            }))
        }
        Ok(Err(message)) => {
            set_last_error(message);
            std::ptr::null_mut()
        }
        Err(_) => {
            set_last_error("eb_open 内部 panic");
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `engine` 必须是 `eb_open` 的返回值，且只能关闭一次。
#[no_mangle]
pub unsafe extern "C" fn eb_close(engine: *mut EbEngine) {
    if engine.is_null() {
        return;
    }
    drop(Box::from_raw(engine));
}

/// 上次失败（`eb_open` 等拿不到 `Response` 的入口）的细节，UTF-8、不带结尾 0。
/// 空串表示没有错误。
///
/// `len` 由 C 调用方给出：本函数写它，调用方保证可写（clippy 只能看到裸指针）。
/// 指针指向库内的错误缓冲，**下一次失败的调用**前有效；不归调用方释放。
/// 需要 `C string` 的宿主用 `eb_plain_last_error`（同一块内存，带结尾 0）。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn eb_last_error(len: *mut usize) -> *const u8 {
    let (ptr, size) = last_error_with_len();
    if !len.is_null() {
        // 不含结尾 0：`size` 已经把 NUL 减掉了。
        unsafe { *len = size };
    }
    ptr as *const u8
}

// ---------------------------------------------------------------- 调用

/// 收一个 postcard `Request`，回一个 postcard `Response`。
///
/// 返回值用 `eb_free` 释放；`out_len` 为 0 时返回 null（只有请求本身解码失败
/// 才会发生，因为错误也会编码成 `Response::Error`）。
///
/// # Safety
/// `engine` 必须是 `eb_open` 的返回值；`req` 必须可读 `len` 字节；
/// `out_len` 不能为 null。
#[no_mangle]
pub unsafe extern "C" fn eb_request(
    engine: *mut EbEngine,
    req: *const u8,
    len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    *out_len = 0;
    if engine.is_null() {
        return std::ptr::null_mut();
    }

    let response = catch_unwind(AssertUnwindSafe(|| {
        let bytes = std::slice::from_raw_parts(req, len);
        let request: Request = match postcard::from_bytes(bytes) {
            Ok(request) => request,
            Err(error) => return Response::Error(format!("解码请求失败: {error}")),
        };
        let engine = &*engine;
        let mut guard = engine
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        dispatch(&mut guard, request)
    }))
    .unwrap_or_else(|_| Response::Error("引擎内部 panic".to_string()));

    let encoded = postcard::to_allocvec(&response).unwrap_or_default();
    let (ptr, size) = into_raw(encoded);
    *out_len = size;
    ptr
}

fn dispatch(engine: &mut Engine, request: Request) -> Response {
    match request {
        Request::Layout {
            spine,
            viewport,
            style,
        } => {
            let viewport = viewport_core(viewport);
            let style = style_core(style);
            match engine.layout(spine as usize, viewport, style) {
                Ok(()) => Response::Pages(engine.pages().into_iter().map(page_wire).collect()),
                Err(error) => Response::Error(error.to_string()),
            }
        }
        Request::Bitmap {
            page,
            scale,
            ink,
            paper,
            bold,
        } => match engine.bitmap(page, scale, ink, paper, bold) {
            Ok(bitmap) => Response::Bitmap {
                width: bitmap.width,
                height: bitmap.height,
                rgba: bitmap.rgba,
            },
            Err(error) => Response::Error(error.to_string()),
        },
        Request::Hit { page, x, y } => Response::Hit(engine.hit(page, x, y).map(loc_wire)),
        Request::LinkAt { page, x, y } => {
            Response::Link(engine.link_at(page, x, y).map(str::to_string))
        }
        Request::Selection { a, b } => Response::Selection(
            engine
                .selection(loc_core(a), loc_core(b))
                .map(|sel| Selection {
                    start: loc_wire(sel.start),
                    end: loc_wire(sel.end),
                    rects: sel.rects.iter().map(rect_wire).collect(),
                    text: sel.text,
                }),
        ),
        Request::Progress { loc } => Response::Progress(engine.progress(loc_core(loc))),
        Request::FragmentLine { fragment } => Response::Line(engine.fragment_line(&fragment)),
        Request::ResolveLink { href } => {
            Response::Target(engine.resolve_link(&href).map(|link| Link {
                spine: link.spine as i32,
                fragment: link.fragment,
                external: link.external,
            }))
        }
        Request::Scroll { viewport, style } => {
            let viewport = viewport_core(viewport);
            let style = style_core(style);
            match engine.scroll_begin(viewport, style) {
                Ok(_) => match engine.scroll_chapters() {
                    Ok(chapters) => Response::ScrollChapters(
                        chapters.into_iter().map(scroll_chapter_wire).collect(),
                    ),
                    Err(error) => Response::Error(error.to_string()),
                },
                Err(error) => Response::Error(error.to_string()),
            }
        }
        Request::ScrollChapters => match engine.scroll_chapters() {
            Ok(chapters) => {
                Response::ScrollChapters(chapters.into_iter().map(scroll_chapter_wire).collect())
            }
            Err(error) => Response::Error(error.to_string()),
        },
        Request::ScrollChapter { index } => match engine.scroll_chapter(index as usize) {
            Ok(chapter) => Response::ScrollChapter(Some(scroll_chapter_wire(chapter))),
            Err(ebook_reader::EngineError::ChapterOutOfRange(_)) => Response::ScrollChapter(None),
            Err(error) => Response::Error(error.to_string()),
        },
        Request::ScrollHeight => match engine.scroll_total_height() {
            Ok(height) => Response::Number(height),
            Err(error) => Response::Error(error.to_string()),
        },
        Request::ScrollSpineAt { y } => match engine.scroll_spine_at(y) {
            Ok(found) => Response::ScrollSpine(found.map(|(spine, _)| spine as u32)),
            Err(error) => Response::Error(error.to_string()),
        },
        Request::ScrollBand {
            y,
            height,
            scale,
            ink,
            paper,
            bold,
        } => match engine.scroll_slice(y, height, scale, ink, paper, bold) {
            Ok(bitmap) => Response::Bitmap {
                width: bitmap.width,
                height: bitmap.height,
                rgba: bitmap.rgba,
            },
            Err(error) => Response::Error(error.to_string()),
        },
        Request::ScrollHit { x, y } => match engine.scroll_hit(x, y) {
            Ok(hit) => Response::ScrollHit(hit.map(scroll_loc_wire)),
            Err(error) => Response::Error(error.to_string()),
        },
        Request::ScrollLineY { spine, line } => match engine.scroll_line_y(spine as usize, line) {
            Ok(y) => Response::Position(y),
            Err(error) => Response::Error(error.to_string()),
        },
        Request::ScrollAnchorY { spine, fragment } => {
            match engine.scroll_anchor_y(spine as usize, &fragment) {
                Ok(y) => Response::Position(y),
                Err(error) => Response::Error(error.to_string()),
            }
        }
        Request::ScrollProgress { y } => match engine.scroll_progress(y) {
            Ok(progress) => Response::Number(progress),
            Err(error) => Response::Error(error.to_string()),
        },
        Request::RegisterFont { bytes } => match engine.register_font(bytes) {
            Ok(id) => Response::Font(id.0 as u32),
            Err(message) => Response::Error(message),
        },
        Request::RegisterFontFace { index } => match engine.register_font_face(index as usize) {
            Ok(id) => Response::Font(id.0 as u32),
            Err(message) => Response::Error(message),
        },
        Request::BaseFontSize => Response::Number(engine.base_font_size()),
        Request::Book => Response::Book(book_wire(engine)),
        Request::Toc => Response::Toc(engine.toc().iter().map(toc_wire).collect()),
        Request::Pages => {
            // 排版结果没变时，宿主随时可以重新要页列表（例如换了窗口大小前的兜底）。
            Response::Pages(engine.pages().into_iter().map(page_wire).collect())
        }
        Request::Limits {
            max_page_pixels,
            image_cache_bytes,
        } => {
            engine.set_limits(Limits {
                max_page_pixels,
                image_cache_bytes: image_cache_bytes as usize,
            });
            Response::Ok
        }
        Request::Ping => Response::Pong,
    }
}

fn viewport_core(viewport: protocol::Viewport) -> Viewport {
    Viewport {
        width: viewport.width,
        height: viewport.height,
        insets: EdgesF {
            top: viewport.insets.top,
            right: viewport.insets.right,
            bottom: viewport.insets.bottom,
            left: viewport.insets.left,
        },
    }
}

fn style_core(style: protocol::Style) -> Style {
    Style {
        font_scale: style.font_scale,
        line_scale: style.line_scale,
        letter_px: style.letter_px,
        word_px: style.word_px,
        align: style.align,
    }
}

fn scroll_chapter_wire(chapter: ebook_reader::ScrollChapter) -> protocol::ScrollChapter {
    protocol::ScrollChapter {
        spine: chapter.spine as u32,
        top: chapter.top,
        height: chapter.height,
        ok: chapter.ok,
    }
}

fn scroll_loc_wire(hit: ebook_reader::ScrollLoc) -> protocol::ScrollLoc {
    protocol::ScrollLoc {
        spine: hit.spine as i32,
        line: hit.line,
        byte: hit.byte,
        y: hit.y,
    }
}

fn loc_core(loc: Loc) -> Location {
    Location {
        page: loc.page,
        line: loc.line,
        byte: loc.byte,
    }
}

fn loc_wire(loc: Location) -> Loc {
    Loc {
        page: loc.page,
        line: loc.line,
        byte: loc.byte,
    }
}

fn page_wire(page: ebook_reader::PageBox) -> Page {
    Page {
        index: page.index,
        width: page.width,
        height: page.height,
        insets: Insets {
            top: page.inset.top,
            right: page.inset.right,
            bottom: page.inset.bottom,
            left: page.inset.left,
        },
        line_from: page.line_from,
        line_len: page.line_len,
        progress: page.progress,
    }
}

fn rect_wire(rect: &CoreRect) -> SelectionRect {
    SelectionRect {
        page: rect.page,
        x: rect.rect.x,
        y: rect.rect.y,
        w: rect.rect.w,
        h: rect.rect.h,
    }
}

fn toc_wire(entry: &ebook_reader::TocEntry) -> TocEntry {
    TocEntry {
        label: entry.label.clone(),
        spine: if entry.spine == ebook_reader::TocEntry::NO_SPINE {
            -1
        } else {
            entry.spine as i32
        },
        fragment: entry.fragment.clone(),
        depth: entry.depth,
    }
}

fn book_wire(engine: &Engine) -> Book {
    let book = engine.book();
    Book {
        title: book.title.clone(),
        language: book.lang.clone(),
        creator: book.creator.clone(),
        publisher: book.publisher.clone(),
        identifier: book.identifier.clone(),
        date: book.date.clone(),
        description: book.description.clone(),
        cover: book.cover_href.clone(),
        spine: book
            .spine
            .iter()
            .enumerate()
            .map(|(index, item)| SpineItem {
                index: index as u32,
                id: item.id.clone(),
                href: item.href.clone(),
                media_type: item.media_type.clone(),
                linear: item.linear,
            })
            .collect(),
        fonts: book
            .fonts()
            .iter()
            .map(|face| FontFace {
                family: face.family.clone(),
                weight: face.weight,
                href: face.href.clone(),
            })
            .collect(),
    }
}
