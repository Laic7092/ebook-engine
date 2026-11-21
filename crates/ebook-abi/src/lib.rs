//! C ABI：给 cgo / Swift / JNI 用的最小面。
//!
//! 设计原则：
//! - **有状态但只关于书**：`eb_open` 返回一个不透明句柄，句柄内部是
//!   `Mutex<Engine>`；viewport/style 这类宿主设置不进 ABI，跟着请求走。
//! - **一条通用入口**：`eb_request` 收发 postcard 编码的 `Request`/`Response`。
//!   新增能力只改 `protocol.rs`，不用再往 C 面上加函数。
//! - **缓冲区归调用方**：`eb_request` 返回的指针必须用 `eb_free_bytes` 释放。
//! - **panic 不出边界**：所有入口都 `catch_unwind`，panic 变成 `Error` 响应，
//!   而不是让整个 App abort。这条依赖 native profile 里的 `panic = "unwind"`。

pub mod plain;
pub mod protocol;

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use ebook_reader::{Engine, Limits, Location, SelectionRect as CoreRect, Style, Viewport};
use ebook_reader::EdgesF;

use protocol::{
    Book, FontFace, Insets, Link, Loc, Page, Request, Response, Selection, SelectionRect,
    SpineItem, TocEntry,
};

const VERSION: &CStr = c"ebook-abi 0.1.0";

/// 不透明句柄。内部加锁，因为 Go/Swift 侧不保证所有调用都在同一个线程上。
pub struct EbEngine {
    inner: Mutex<Engine>,
}

thread_local! {
    /// 最后一个错误的细节，只在 `eb_open` 这类拿不到 `Response` 的入口用。
    pub(crate) static LAST_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn set_last_error(message: impl Into<String>) {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(message.into()));
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

fn into_raw(bytes: Vec<u8>) -> (*mut u8, usize) {
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
    VERSION.as_ptr()
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

/// 上次 `eb_open` 失败的细节（UTF-8，不带结尾 0）。空串表示没有错误。
#[no_mangle]
pub extern "C" fn eb_last_error(len: *mut usize) -> *const u8 {
    LAST_ERROR.with(|slot| {
        let message = slot.borrow().clone().unwrap_or_default();
        let mut bytes = message.into_bytes();
        bytes.shrink_to_fit();
        let len_value = bytes.len();
        // 调用方拿到指针后马上拷贝；下一次 `eb_open` 会覆盖它。
        let ptr = bytes.as_ptr();
        std::mem::forget(bytes);
        if !len.is_null() {
            unsafe { *len = len_value };
        }
        ptr
    })
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
            let viewport = Viewport {
                width: viewport.width,
                height: viewport.height,
                insets: EdgesF {
                    top: viewport.insets.top,
                    right: viewport.insets.right,
                    bottom: viewport.insets.bottom,
                    left: viewport.insets.left,
                },
            };
            let style = Style {
                font_scale: style.font_scale,
                line_scale: style.line_scale,
                letter_px: style.letter_px,
                align: style.align,
            };
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
        Request::Hit { page, x, y } => {
            Response::Hit(engine.hit(page, x, y).map(loc_wire))
        }
        Request::LinkAt { page, x, y } => {
            Response::Link(engine.link_at(page, x, y).map(str::to_string))
        }
        Request::Selection { a, b } => Response::Selection(
            engine.selection(loc_core(a), loc_core(b)).map(|sel| Selection {
                start: loc_wire(sel.start),
                end: loc_wire(sel.end),
                rects: sel.rects.iter().map(rect_wire).collect(),
                text: sel.text,
            }),
        ),
        Request::Progress { loc } => Response::Progress(engine.progress(loc_core(loc))),
        Request::FragmentLine { fragment } => {
            Response::Line(engine.fragment_line(&fragment))
        }
        Request::ResolveLink { href } => Response::Target(engine.resolve_link(&href).map(|link| {
            Link {
                spine: link.spine as i32,
                fragment: link.fragment,
                external: link.external,
            }
        })),
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
