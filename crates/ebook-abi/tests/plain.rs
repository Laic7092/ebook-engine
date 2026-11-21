//! 标量 C 面的往返测试：真实 `extern "C"` 入口，位图落到临时文件。

use std::ffi::CString;

use ebook_abi::plain::*;

fn demo() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.txt"
    ))
    .unwrap()
}

/// 本机有 CJK 字形的系统字体；没有的话测试跳过。
fn font() -> Vec<u8> {
    for path in [
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            return bytes;
        }
    }
    Vec::new()
}

/// 打开示例书，返回句柄和「注册上字体了吗」：CI 的机器上可能没有中文字体，
/// 没有字体时页照样出，位图上没有字。
fn open_book() -> (i32, bool) {
    let bytes = demo();
    let name = c"demo.txt";
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), name.as_ptr()) };
    assert!(engine >= 0, "eb_plain_open 失败");
    let font = font();
    let mut has_font = false;
    if !font.is_empty() {
        let id = unsafe { eb_plain_register_font(engine, font.as_ptr(), font.len()) };
        assert!(id >= 0, "注册字体失败");
        has_font = true;
    }
    (engine, has_font)
}

#[test]
fn lays_out_and_writes_a_raw_page() {
    let (engine, has_font) = open_book();
    assert!(eb_plain_spine_count(engine) > 0);
    assert!(eb_plain_page_count(engine) == 0, "排版前没有页");
    let rc = eb_plain_layout(
        engine, 0, 400.0, 700.0, 24.0, 24.0, 24.0, 24.0, 1.0, 1.0, 0.0, 0.0, 0,
    );
    assert_eq!(rc, 0, "排版失败");
    assert!(eb_plain_page_count(engine) > 0, "排出来没有页");

    let info = eb_plain_page_info(engine, 0);
    assert_eq!(info.ok, 1);
    // 页框是内容盒：视口减去四边留白。
    assert!((info.width - 352.0).abs() < 1.0, "页宽 {}", info.width);
    assert!((info.height - 652.0).abs() < 1.0, "页高 {}", info.height);

    let path = std::env::temp_dir().join("eb-plain-page0.raw");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    let rc = eb_plain_page_raw(engine, 0, 2.0, 0, 0, false, c_path.as_ptr());
    assert_eq!(rc, 0, "光栅化失败");

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"EQP1");
    let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    // 位图覆盖整个视口（含四边留白），不是内容盒。
    assert_eq!(width, 800);
    assert_eq!(height, 1400);
    assert_eq!(bytes.len(), 12 + (width * height * 4) as usize);

    // 有字体时页上一定不是全白：有字。
    let ink = bytes[12..].as_chunks::<4>().0.iter().any(|px| px[0] < 200);
    if has_font {
        assert!(ink, "位图是空的");
    } else {
        eprintln!("no font on this machine: the page is blank, as it should be");
    }

    let hit = eb_plain_hit(engine, 0, 40.0, 40.0);
    assert!(hit.page >= 0, "点在页上没有位置");
    assert!(hit.progress >= 0.0 && hit.progress <= 1.0);

    eb_plain_close(engine);
}

#[test]
fn no_font_makes_blank_pages() {
    // 没有注册字体时排版仍然出页（StubMeasurer），但位图上一个字也没有：
    // 这正是 iOS 上「有页码、无正文」的样子。
    let bytes = demo();
    let name = c"demo.txt";
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), name.as_ptr()) };
    assert!(engine >= 0);
    let _ = eb_plain_layout(
        engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, 0.0, 0,
    );
    let pages = eb_plain_page_count(engine);
    let path = std::env::temp_dir().join("eb-nofont.eqp");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_page_raw(engine, 0, 2.0, 0, 0, false, c_path.as_ptr()),
        0
    );
    let bytes = std::fs::read(&path).unwrap();
    let ink = bytes[12..].as_chunks::<4>().0.iter().any(|px| px[0] < 200);
    eprintln!("no font: {pages} pages, ink drawn: {ink}");
    assert!(pages > 0);
    assert!(!ink, "没有字体时不该有字");
    eb_plain_close(engine);
}

#[test]
fn the_ink_colors_the_glyphs() {
    // 夜纸：黑底浅字。排版时写进 IR 的是黑色，墨色在这一层覆盖它——不然
    // 夜里就是黑字画在黑纸上。
    let (engine, has_font) = open_book();
    if !has_font {
        eprintln!("no font on this machine: nothing to colour");
        eb_plain_close(engine);
        return;
    }
    let _ = eb_plain_layout(
        engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, 0.0, 0,
    );
    let path = std::env::temp_dir().join("eb-night.eqp");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_page_raw(
            engine,
            0,
            2.0,
            0xd8d6_cfff,
            0x1616_1aff,
            false,
            c_path.as_ptr()
        ),
        0
    );
    let bytes = std::fs::read(&path).unwrap();
    let pixels = bytes[12..].as_chunks::<4>().0;
    let ink = pixels.iter().filter(|px| px[0] > 0xc0).count();
    let paper = pixels
        .iter()
        .filter(|px| px[0] == 0x16 && px[1] == 0x16 && px[2] == 0x1a)
        .count();
    assert!(
        paper > pixels.len() / 2,
        "纸面不是夜纸: {paper} of {}",
        pixels.len()
    );
    assert!(ink > 500, "没有浅色的字: {ink} 个像素");
    eb_plain_close(engine);
}

#[test]
fn word_spacing_reaches_the_layout() {
    // 词距是西文的事：同样的字，词与词之间放宽一点，一行就装不下了。
    // 它走的是 `Style::word_px` → 节点的 `word-spacing`，不经过任何 UI。
    let text = "the quick brown fox jumps over the lazy dog. ".repeat(400);
    let bytes = text.into_bytes();
    let name = c"english.txt";
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), name.as_ptr()) };
    assert!(engine >= 0);
    let pages = |word: f32| {
        let _ = eb_plain_layout(
            engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, word, 0,
        );
        eb_plain_page_count(engine)
    };
    let tight = pages(0.0);
    let wide = pages(12.0);
    assert!(tight > 0);
    assert!(wide > tight, "词距没进排版: {tight} 页 -> {wide} 页");
    eb_plain_close(engine);
}

#[test]
fn reports_book_and_toc() {
    let (engine, _) = open_book();
    let title = eb_plain_title(engine);
    assert!(!title.is_null());
    let href = eb_plain_spine_href(engine, 0);
    assert!(!href.is_null());
    assert!(eb_plain_toc_count(engine) > 0);
    assert!(eb_plain_toc_spine(engine, 0) >= 0);
    eb_plain_close(engine);
}

#[test]
fn timings() {
    let (engine, _) = open_book();
    let _ = eb_plain_layout(
        engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, 0.0, 0,
    );
    let pages = eb_plain_page_count(engine);
    let run = pages.min(5);
    let start = std::time::Instant::now();
    for page in 0..run {
        let path = std::env::temp_dir().join(format!("eb-bench-{page}.eqp"));
        let c = CString::new(path.to_str().unwrap()).unwrap();
        assert_eq!(
            eb_plain_page_raw(engine, page, 3.0, 0, 0, false, c.as_ptr()),
            0
        );
    }
    eprintln!(
        "{run} pages at 3x: {:?} ({} ms a page)",
        start.elapsed(),
        start.elapsed().as_millis() / u128::from(run)
    );
    let start = std::time::Instant::now();
    let _ = eb_plain_layout(
        engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, 0.0, 0,
    );
    eprintln!("one chapter layout: {:?}", start.elapsed());
    eb_plain_close(engine);
}
