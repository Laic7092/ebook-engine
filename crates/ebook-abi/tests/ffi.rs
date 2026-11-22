//! C ABI 的往返测试：走真正的 `extern "C"` 入口，包括缓冲区的分配与释放。

use ebook_abi::protocol::{Insets, Request, Response, Style, Viewport};
use ebook_abi::*;

/// `eb_last_error` / `eb_plain_last_error` 是进程级缓冲：成功开书会清空它。
/// 测试默认并行，任何一次成功的 `open` 都能把另一个测试刚设的错误擦掉。
/// 这个互斥把每个 ABI 测试串起来；整套只有几百毫秒，稳定性比并行更值钱。
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn demo() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.txt"
    ))
    .unwrap()
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
        word_px: 0.0,
        align: 0,
    }
}

#[test]
fn pings() {
    let _guard = serial();
    let engine = open();
    assert!(matches!(call(engine, &Request::Ping), Response::Pong));
    unsafe { eb_close(engine) };
}

#[test]
fn opens_and_reports_the_book() {
    let _guard = serial();
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
    let _guard = serial();
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
    let _guard = serial();
    let engine = open();
    let _ = call(
        engine,
        &Request::Layout {
            spine: 0,
            viewport: viewport(),
            style: style(),
        },
    );
    let hit = match call(
        engine,
        &Request::Hit {
            page: 0,
            x: 80.0,
            y: 100.0,
        },
    ) {
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
    let _guard = serial();
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
    let _guard = serial();
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

#[test]
fn version_and_the_error_buffer_contract() {
    let _guard = serial();
    // 版本不再手写：跟 CARGO_PKG_VERSION 一致，结构布局版本与头文件对齐。
    let version = unsafe { std::ffi::CStr::from_ptr(eb_version()) };
    assert_eq!(
        version.to_str().unwrap(),
        concat!("ebook-abi ", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(eb_abi_version(), ebook_abi::ABI_VERSION);

    // 失败后：指针 + len 指向库内缓冲（不在调用方，不需要 eb_free），
    // 紧跟的字节是结尾 0。连调两次不会换地址——没有 mem::forget 漏出去的旧串。
    let mut first = 0usize;
    let mut second = 0usize;
    let first_ptr = unsafe {
        assert!(eb_open(std::ptr::null(), 0, std::ptr::null()).is_null());
        eb_last_error(&mut first)
    };
    let second_ptr = unsafe {
        assert!(eb_open(std::ptr::null(), 0, std::ptr::null()).is_null());
        eb_last_error(&mut second)
    };
    assert_eq!(first, second);
    assert_eq!(first_ptr, second_ptr);
    assert!(!first_ptr.is_null());
    assert!(first > 0, "失败应该留下原因");
    let bytes = unsafe { std::slice::from_raw_parts(first_ptr, first) };
    assert!(std::str::from_utf8(bytes).is_ok(), "错误串应是 UTF-8");
    assert_eq!(unsafe { *first_ptr.add(first) }, 0, "len 不含结尾 0");
}

#[test]
fn scroll_requests_round_trip() {
    let _guard = serial();
    let engine = open();

    // 开始会话，一次拿到章节表（协议面可以返回数组）。
    let chapters = match call(
        engine,
        &Request::Scroll {
            viewport: viewport(),
            style: style(),
        },
    ) {
        Response::ScrollChapters(chapters) => chapters,
        other => panic!("Scroll 应回章节表，实际 {other:?}"),
    };
    assert!(!chapters.is_empty(), "demo.txt 应至少一章");
    assert_eq!(chapters[0].spine, 0);
    assert_eq!(chapters[0].top, 0.0);
    assert!(chapters[0].height > 0.0);
    for pair in chapters.windows(2) {
        assert!((pair[1].top - (pair[0].top + pair[0].height)).abs() < 0.01);
    }

    let total = match call(engine, &Request::ScrollHeight) {
        Response::Number(total) => total,
        other => panic!("ScrollHeight 应回数字，实际 {other:?}"),
    };
    let last = chapters.last().unwrap();
    assert!((total - (last.top + last.height)).abs() < 0.01);

    // 流坐标 → 章 → 章首。
    assert!(matches!(
        call(engine, &Request::ScrollSpineAt { y: 0.0 }),
        Response::ScrollSpine(Some(0))
    ));
    assert!(matches!(
        call(engine, &Request::ScrollSpineAt { y: total + 500.0 }),
        Response::ScrollSpine(None)
    ));
    assert!(matches!(
        call(engine, &Request::ScrollChapter { index: 9999 }),
        Response::ScrollChapter(None)
    ));

    // 可见带：直接回像素，不用落盘。
    match call(
        engine,
        &Request::ScrollBand {
            y: 0.0,
            height: 100.0,
            scale: 1.0,
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
            assert_eq!(width, 400);
            assert_eq!(height, 100);
            assert_eq!(rgba.len(), 400 * 100 * 4);
        }
        other => panic!("ScrollBand 应回位图，实际 {other:?}"),
    }

    // 命中带章号，行号能换回同一个流坐标。
    let hit = match call(engine, &Request::ScrollHit { x: 40.0, y: 20.0 }) {
        Response::ScrollHit(Some(hit)) => hit,
        other => panic!("ScrollHit 应有命中，实际 {other:?}"),
    };
    assert_eq!(hit.spine, 0);
    match call(
        engine,
        &Request::ScrollLineY {
            spine: 0,
            line: hit.line,
        },
    ) {
        Response::Position(Some(y)) => assert!((y - hit.y).abs() < 0.01),
        other => panic!("ScrollLineY 应回位置，实际 {other:?}"),
    }
    // 不存在的锚点：`None`，不是错误。
    assert!(matches!(
        call(
            engine,
            &Request::ScrollAnchorY {
                spine: 0,
                fragment: "not-here".to_string(),
            },
        ),
        Response::Position(None)
    ));

    // 进度：书尾是 1。会话没开始时是 Error。
    match call(engine, &Request::ScrollProgress { y: total }) {
        Response::Number(progress) => assert!((progress - 1.0).abs() < 1.0e-6),
        other => panic!("ScrollProgress 应回数字，实际 {other:?}"),
    }
    unsafe { eb_close(engine) };
    let bare = open();
    assert!(matches!(
        call(bare, &Request::ScrollHeight),
        Response::Error(_)
    ));
    unsafe { eb_close(bare) };
}
