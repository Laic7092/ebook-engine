//! C ABI 的往返测试：走真正的 `extern "C"` 入口，包括缓冲区的分配与释放。

use ebook_abi::protocol::{Request, Response, Style, Viewport, Insets};
use ebook_abi::*;

fn demo() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/demo.txt")).unwrap()
}

fn call(engine: *mut EbEngine, request: &Request) -> Response {
    let bytes = postcard::to_allocvec(request).unwrap();
    let mut out_len = 0usize;
    unsafe {
        let ptr = eb_request(engine, bytes.as_ptr(), bytes.len(), &mut out_len);
        assert!(!ptr.is_null(), "eb_request 返回了空指针");
        let out = std::slice::from_raw_parts(ptr, out_len).to_vec();
        eb_free(ptr, out_len);
        postcard::from_bytes(&out).unwrap()
    }
}

fn open() -> *mut EbEngine {
    let bytes = demo();
    let name = c"demo.txt";
    unsafe {
        let engine = eb_open(bytes.as_ptr(), bytes.len(), name.as_ptr());
        assert!(!engine.is_null(), "eb_open 失败");
        engine
    }
}

fn viewport() -> Viewport {
    Viewport {
        width: 400.0,
        height: 700.0,
        insets: Insets {
            top: 24.0,
            right: 24.0,
            bottom: 24.0,
            left: 24.0,
        },
    }
}

fn style() -> Style {
    Style {
        font_scale: 1.0,
        line_scale: 1.0,
        letter_px: 0.0,
        align: 0,
    }
}

#[test]
fn pings() {
    let engine = open();
    assert!(matches!(call(engine, &Request::Ping), Response::Pong));
    unsafe { eb_close(engine) };
}

#[test]
fn opens_and_reports_the_book() {
    let engine = open();
    match call(engine, &Request::Book) {
        Response::Book(book) => {
            assert!(!book.spine.is_empty());
            assert!(book.title.is_some() || book.language.is_some());
        }
        other => panic!("期望 Book，拿到 {other:?}"),
    }
    match call(engine, &Request::Toc) {
        Response::Toc(entries) => {
            assert!(!entries.is_empty());
            assert!(entries.iter().all(|entry| entry.spine >= -1));
        }
        other => panic!("期望 Toc，拿到 {other:?}"),
    }
    unsafe { eb_close(engine) };
}

#[test]
fn layout_then_bitmap() {
    let engine = open();
    let pages = match call(
        engine,
        &Request::Layout {
            spine: 0,
            viewport: viewport(),
            style: style(),
        },
    ) {
        Response::Pages(pages) => pages,
        other => panic!("期望 Pages，拿到 {other:?}"),
    };
    assert!(!pages.is_empty());
    assert_eq!(pages[0].index, 0);
    assert!((pages[0].width - 352.0).abs() < 0.5);

    match call(
        engine,
        &Request::Bitmap {
            page: 0,
            scale: 2.0,
            ink: 0,
            paper: 0,
            bold: false,
        },
    ) {
        Response::Bitmap {
            width,
            height,
            rgba,
        } => {
            // `bitmap` 用的是整页几何（400×700），不是内容盒（352×652）。
            assert_eq!(width, 800);
            assert_eq!(height, 1400);
            assert_eq!(rgba.len(), (width * height * 4) as usize);
        }
        other => panic!("期望 Bitmap，拿到 {other:?}"),
    }
    unsafe { eb_close(engine) };
}

#[test]
fn hit_progress_and_selection() {
    let engine = open();
    let _ = call(
        engine,
        &Request::Layout {
            spine: 0,
            viewport: viewport(),
            style: style(),
        },
    );
    let hit = match call(engine, &Request::Hit { page: 0, x: 80.0, y: 100.0 }) {
        Response::Hit(hit) => hit.expect("正文区域应该命中"),
        other => panic!("期望 Hit，拿到 {other:?}"),
    };
    match call(engine, &Request::Progress { loc: hit }) {
        Response::Progress(progress) => {
            let progress = progress.expect("排过版就有进度");
            assert!((0.0..=1.0).contains(&progress));
        }
        other => panic!("期望 Progress，拿到 {other:?}"),
    }
    assert!(matches!(
        call(engine, &Request::Selection { a: hit, b: hit }),
        Response::Selection(Some(_))
    ));
    unsafe { eb_close(engine) };
}

#[test]
fn bad_request_returns_an_error_response() {
    let engine = open();
    let mut out_len = 0usize;
    let garbage = [0xffu8; 12];
    unsafe {
        let ptr = eb_request(engine, garbage.as_ptr(), garbage.len(), &mut out_len);
        let out = std::slice::from_raw_parts(ptr, out_len).to_vec();
        eb_free(ptr, out_len);
        let response: Response = postcard::from_bytes(&out).unwrap();
        assert!(matches!(response, Response::Error(_)));
        eb_close(engine);
    }
}

#[test]
fn opening_garbage_returns_null() {
    let bytes = b"not a book at all";
    unsafe {
        let engine = eb_open(bytes.as_ptr(), bytes.len(), std::ptr::null());
        // 纯文本总是能打开（按文本解码），所以这里不是 null；空指针才是失败路径。
        if !engine.is_null() {
            eb_close(engine);
        }
        let engine = eb_open(std::ptr::null(), 0, std::ptr::null());
        assert!(engine.is_null());
        let mut len = 0usize;
        let message = eb_last_error(&mut len);
        assert!(len > 0, "应该留下错误信息");
        assert!(!message.is_null());
    }
}
