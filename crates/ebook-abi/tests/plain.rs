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
fn lays_out_scroll_and_writes_a_slice() {
    let (engine, has_font) = open_book();
    // 滚动：上下留白 0，左右 24；height 只是视口高，不参与切页。
    let rc = eb_plain_layout_scroll(
        engine, 0, 400.0, 700.0, 0.0, 24.0, 0.0, 24.0, 1.0, 1.0, 0.0, 0.0, 0,
    );
    assert_eq!(rc, 0, "滚动排版失败");
    assert_eq!(eb_plain_page_count(engine), 1, "整章一页");

    let height = eb_plain_content_height(engine);
    assert!(height > 0.0, "content height {height}");
    let info = eb_plain_page_info(engine, 0);
    assert_eq!(info.ok, 1);
    assert!((info.height - height).abs() < 0.01);
    assert_eq!(info.width, 352.0);

    let path = std::env::temp_dir().join("eb-plain-scroll-slice.raw");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    let rc = eb_plain_page_slice_raw(engine, 0, 0.0, 300.0, 2.0, 0, 0, false, c_path.as_ptr());
    assert_eq!(rc, 0, "瓦片光栅化失败");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"EQP1");
    let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let slice_height = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    assert_eq!(width, 800);
    assert_eq!(slice_height, 600);
    assert_eq!(bytes.len(), 12 + (width * slice_height * 4) as usize);

    // 同一 y 再来一次逐字节相同；不同 y 是不同的一片。
    let again = std::env::temp_dir().join("eb-plain-scroll-slice-again.raw");
    let c_again = CString::new(again.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_page_slice_raw(engine, 0, 0.0, 300.0, 2.0, 0, 0, false, c_again.as_ptr(),),
        0
    );
    assert_eq!(std::fs::read(&again).unwrap(), bytes);

    let lower = std::env::temp_dir().join("eb-plain-scroll-lower.raw");
    let c_lower = CString::new(lower.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_page_slice_raw(
            engine,
            0,
            height / 2.0,
            300.0,
            2.0,
            0,
            0,
            false,
            c_lower.as_ptr(),
        ),
        0
    );
    assert_ne!(std::fs::read(&lower).unwrap(), bytes);

    let hit = eb_plain_hit(engine, 0, 40.0, 40.0);
    assert!(hit.page >= 0, "滚动页上应该有位置");
    assert!((0.0..=1.0).contains(&hit.progress));

    if !has_font {
        eprintln!("no font on this machine: slices are paper, as they should be");
    }
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

    // 新增的书目/spine/TOC 字段：越界一律给空串或 0，不是 null、不 panic。
    for getter in [
        eb_plain_language,
        eb_plain_creator,
        eb_plain_publisher,
        eb_plain_identifier,
        eb_plain_date,
        eb_plain_description,
        eb_plain_cover_href,
        eb_plain_cover_mime,
    ] {
        assert!(!getter(engine).is_null(), "句柄有效时不该返回 null");
        assert!(
            !getter(engine + 1000).is_null(),
            "无效句柄时也不该返回 null"
        );
    }
    assert_eq!(eb_plain_cover_len(engine + 1000), 0);
    // TXT 没有封面：-1 而不是写一个空文件。
    let path = std::env::temp_dir().join("eb-no-cover.png");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(eb_plain_cover_write(engine, c_path.as_ptr()), -1);
    assert!(!path.exists());

    assert!(!eb_plain_spine_id(engine, 0).is_null());
    assert!(!eb_plain_spine_media_type(engine, 0).is_null());
    assert_eq!(eb_plain_spine_linear(engine, 0), 1);
    assert_eq!(eb_plain_spine_linear(engine, 9999), 0);
    assert!(!eb_plain_spine_href(engine, 9999).is_null());

    assert!(!eb_plain_toc_href(engine, 0).is_null());
    // `eb_plain_toc_fragment` 没有 fragment 时是空串，不是 null。
    assert!(!eb_plain_toc_fragment(engine, 0).is_null());

    eb_plain_close(engine);

    // 关掉以后：句柄不再有效，但不 panic。
    assert_eq!(eb_plain_spine_count(engine), 0);
    assert_eq!(eb_plain_page_count(engine), 0);
    assert_eq!(eb_plain_set_limits(engine, 1024, 1024), -1);
    assert_eq!(eb_plain_select(engine, 0, 0, 0, 0, 1, 0), -1);
    assert_eq!(eb_plain_selection_rect_count(engine), 0);
}

#[test]
fn selection_round_trip() {
    let (engine, _) = open_book();
    // 没排版时选不出来，但不干预。
    assert!(eb_plain_select(engine, 0, 0, 0, 0, 1, 0) < 0);
    let rc = eb_plain_layout(
        engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, 0.0, 0,
    );
    assert_eq!(rc, 0);

    let count = eb_plain_select(engine, 0, 0, 0, 0, 2, 0);
    assert!(count > 0, "跨行选区应该有矩形块: {count}");
    assert_eq!(eb_plain_selection_rect_count(engine), count as u32);

    let text = eb_plain_selection_text(engine);
    assert!(!text.is_null());
    assert!(!unsafe { std::ffi::CStr::from_ptr(text) }
        .to_bytes()
        .is_empty());

    for index in 0..count as u32 {
        let rect = eb_plain_selection_rect(engine, index);
        assert_eq!(rect.ok, 1, "第 {index} 块");
        assert!(rect.w > 0.0 && rect.h > 0.0, "第 {index} 块是空矩形");
    }
    let out = eb_plain_selection_rect(engine, count as u32 + 5);
    assert_eq!(out.ok, 0, "越界下标应给 ok == 0");

    let start = eb_plain_selection_start(engine);
    let end = eb_plain_selection_end(engine);
    assert_eq!(start.page, 0, "起点应在第 0 页");
    assert!(end.page >= 0);
    assert!((0.0..=1.0).contains(&start.progress));

    // 端点越界：-2，且把上一份选区清掉（不留下一半新一半旧的结果）。
    assert_eq!(eb_plain_select(engine, 0, 0, 0, 99, 0, 0), -2);
    assert_eq!(eb_plain_selection_rect_count(engine), 0);
    assert_eq!(eb_plain_selection_start(engine).page, -1);
    eb_plain_close(engine);
}

#[test]
fn resolves_links_and_fragments() {
    // 用真 EPUB：书内链接是相对当前章解析的，TXT 没有这套路径语义。
    let Ok(bytes) = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.epub"
    )) else {
        // 夹具是生成的（`scripts/generate.sh`），干净 checkout 上没有就跳过，
        // 与「本机没字体就跳过」同一个态度。
        eprintln!("fixtures/demo.epub 不在，跳过链接解析测试");
        return;
    };
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), c"demo.epub".as_ptr()) };
    assert!(engine >= 0, "打开 demo.epub 失败");
    assert_eq!(eb_plain_spine_count(engine), 2);
    let rc = eb_plain_layout(
        engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, 0.0, 0,
    );
    assert_eq!(rc, 0);

    // 从第 0 章解 `ch2.xhtml`：落到 spine 1。
    let link = eb_plain_resolve_link(engine, c"ch2.xhtml".as_ptr());
    assert_eq!(link.ok, 1, "相对链接应该能解开");
    assert_eq!(link.spine, 1);
    assert_eq!(link.external, 0);
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(eb_plain_resolved_fragment(engine)) }.to_bytes(),
        b""
    );

    // 带 fragment 的书内链接。
    let link = eb_plain_resolve_link(engine, c"ch2.xhtml#snow".as_ptr());
    assert_eq!(link.ok, 1);
    assert_eq!(link.spine, 1);
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(eb_plain_resolved_fragment(engine)) }.to_bytes(),
        b"snow"
    );

    // 本书内 `#id`：指向当前章。
    let link = eb_plain_resolve_link(engine, c"#snow".as_ptr());
    assert_eq!(link.ok, 1);
    assert_eq!(link.spine, 0);

    // 外链：ok，external == 1，spine == -1。
    let link = eb_plain_resolve_link(engine, c"https://example.com/x#y".as_ptr());
    assert_eq!(link.ok, 1);
    assert_eq!(link.external, 1);
    assert_eq!(link.spine, -1);

    // 解不出的 href：ok == 0，且不留下上一次的 fragment。
    assert_eq!(
        eb_plain_resolve_link(engine, c"nope.xhtml#x".as_ptr()).ok,
        0
    );
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(eb_plain_resolved_fragment(engine)) }.to_bytes(),
        b""
    );

    // 锚点行号：本章没有这个 fragment 就是 -1。
    assert_eq!(
        eb_plain_fragment_line(engine, c"definitely-not-here".as_ptr()),
        -1
    );
    assert_eq!(eb_plain_fragment_line(engine, std::ptr::null()), -1);
    eb_plain_close(engine);
}

#[test]
fn limits_are_enforced() {
    let (engine, _) = open_book();
    let rc = eb_plain_layout(
        engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, 0.0, 0,
    );
    assert_eq!(rc, 0);

    // 一个很小的单页上限：1x（390×760 = 296k）能过，3x（2.67M）过不去。
    assert_eq!(eb_plain_set_limits(engine, 400_000, 1024 * 1024), 0);
    let small = std::env::temp_dir().join("eb-limit-small.eqp");
    let c_small = CString::new(small.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_page_raw(engine, 0, 1.0, 0, 0, false, c_small.as_ptr()),
        0
    );
    let big = std::env::temp_dir().join("eb-limit-big.eqp");
    let c_big = CString::new(big.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_page_raw(engine, 0, 3.0, 0, 0, false, c_big.as_ptr()),
        -2,
        "超过 max_page_pixels 应拒绝"
    );
    eb_plain_close(engine);
}

#[test]
fn the_error_string_is_a_pointer_not_a_leak() {
    // 失败一次：错误串就在库内的缓冲里，指针不归调用方、也不该每次漏一份。
    let engine = unsafe { eb_plain_open(std::ptr::null(), 0, std::ptr::null()) };
    assert_eq!(engine, -1);
    let first = eb_plain_last_error();
    assert!(!first.is_null());
    let message = unsafe { std::ffi::CStr::from_ptr(first) }
        .to_string_lossy()
        .into_owned();
    assert!(!message.is_empty(), "失败的 open 应该留下原因");

    // 再失败一次：还是同一块缓冲（没有 mem::forget 漏出去的旧串），内容不变。
    let engine = unsafe { eb_plain_open(std::ptr::null(), 0, std::ptr::null()) };
    assert_eq!(engine, -1);
    let second = unsafe { std::ffi::CStr::from_ptr(eb_plain_last_error()) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(second, message, "同一个失败应该给同一条原因");

    // 成功一次就清空。
    let bytes = demo();
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), c"demo.txt".as_ptr()) };
    assert!(engine >= 0);
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(eb_plain_last_error()) }.to_bytes(),
        b""
    );
    eb_plain_close(engine);
}

#[test]
fn version_is_not_hard_coded_to_an_old_release() {
    let version = unsafe { std::ffi::CStr::from_ptr(eb_plain_version()) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(version, format!("ebook-abi {}", env!("CARGO_PKG_VERSION")));
    assert_eq!(eb_plain_abi_version(), ebook_abi::ABI_VERSION);
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
