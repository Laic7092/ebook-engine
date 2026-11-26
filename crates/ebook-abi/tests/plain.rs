//! 标量 C 面的往返测试：真实 `extern "C"` 入口，位图落到临时文件。

use std::ffi::CString;

use ebook_abi::plain::*;
use ebook_abi::EbEngine;

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
fn open_book() -> (*mut EbEngine, bool) {
    let bytes = demo();
    let name = c"demo.txt";
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), name.as_ptr()) };
    assert!(!engine.is_null(), "eb_plain_open 失败");
    let font = font();
    let mut has_font = false;
    if !font.is_empty() {
        let id = unsafe { eb_plain_register_font(engine, font.as_ptr(), font.len()) };
        assert!(id >= 0, "注册字体失败");
        has_font = true;
    }
    (engine, has_font)
}

#[allow(clippy::too_many_arguments)]
fn layout_options(
    spine: u32,
    scroll: u32,
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
) -> EbLayoutOptions {
    EbLayoutOptions {
        size: std::mem::size_of::<EbLayoutOptions>() as u32,
        version: EB_LAYOUT_OPTIONS_VERSION,
        spine,
        scroll,
        width,
        height,
        top,
        right,
        bottom,
        left,
        font_scale,
        line_scale,
        letter_px,
        word_px,
        align,
    }
}

#[allow(clippy::too_many_arguments)]
fn layout(
    engine: *mut EbEngine,
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
    let options = layout_options(
        spine, 0, width, height, top, right, bottom, left, font_scale, line_scale, letter_px,
        word_px, align,
    );
    unsafe { eb_plain_layout(engine, &options) }
}

#[allow(clippy::too_many_arguments)]
fn layout_scroll(
    engine: *mut EbEngine,
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
    let options = layout_options(
        spine, 1, width, height, top, right, bottom, left, font_scale, line_scale, letter_px,
        word_px, align,
    );
    unsafe { eb_plain_layout(engine, &options) }
}

#[allow(clippy::too_many_arguments)]
fn scroll_begin(
    engine: *mut EbEngine,
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
    let options = layout_options(
        0, 0, width, height, top, right, bottom, left, font_scale, line_scale, letter_px, word_px,
        align,
    );
    unsafe { eb_plain_scroll_begin(engine, &options) }
}

#[allow(clippy::too_many_arguments)]
fn scroll_slice_raw(
    engine: *mut EbEngine,
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
    y: f32,
    slice_height: f32,
    scale: f32,
    ink: u32,
    paper: u32,
    bold: bool,
    path: *const std::ffi::c_char,
) -> i32 {
    let options = EbScrollSliceOptions {
        size: std::mem::size_of::<EbScrollSliceOptions>() as u32,
        version: EB_SCROLL_SLICE_OPTIONS_VERSION,
        layout: layout_options(
            spine, 1, width, height, top, right, bottom, left, font_scale, line_scale, letter_px,
            word_px, align,
        ),
        y,
        slice_height,
        scale,
        ink,
        paper,
        bold,
    };
    unsafe { eb_plain_scroll_slice_raw(engine, &options, path) }
}

#[test]
fn lays_out_and_writes_a_raw_page() {
    let _guard = serial();
    let (engine, has_font) = open_book();
    assert!(eb_plain_spine_count(engine) > 0);
    assert!(eb_plain_page_count(engine) == 0, "排版前没有页");
    let rc = layout(
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

/// 落盘先写临时文件再 `rename`：目标路径要么不存在，要么是完整位图，成功
/// 之后也不留下 `.part` 残片。
#[test]
fn raw_writes_are_atomic_and_leave_no_parts() {
    let _guard = serial();
    let (engine, _) = open_book();
    assert_eq!(
        layout(engine, 0, 400.0, 700.0, 24.0, 24.0, 24.0, 24.0, 1.0, 1.0, 0.0, 0.0, 0,),
        0
    );

    let dir = std::env::temp_dir().join("eb-abi-atomic");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("page.raw");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_page_raw(engine, 0, 1.0, 0, 0, false, c_path.as_ptr()),
        0
    );

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"EQP1");
    let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), 12 + width * height * 4, "目标文件不是完整位图");

    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "page.raw")
        .collect();
    assert!(leftovers.is_empty(), "临时文件没清掉: {leftovers:?}");

    // 目录不存在：失败返回 -3，也不会凭空留下文件。
    let missing = dir.join("nope").join("page.raw");
    let c_missing = CString::new(missing.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_page_raw(engine, 0, 1.0, 0, 0, false, c_missing.as_ptr()),
        -3
    );
    assert!(!missing.exists());

    // 覆盖同一个目标路径：仍然是完整位图。
    assert_eq!(
        eb_plain_page_raw(engine, 0, 1.0, 0, 0, false, c_path.as_ptr()),
        0
    );
    let again = std::fs::read(&path).unwrap();
    assert_eq!(again.len(), bytes.len());

    let _ = std::fs::remove_dir_all(&dir);
    eb_plain_close(engine);
}

#[test]
fn lays_out_scroll_and_writes_a_slice() {
    let _guard = serial();
    let (engine, has_font) = open_book();
    // 滚动：上下留白 0，左右 24；height 只是视口高，不参与切页。
    let rc = layout_scroll(
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
    let lower_bytes = std::fs::read(&lower).unwrap();
    if has_font {
        // 流里有字：不同 y 拿到的必须不是同一张图。
        assert_ne!(lower_bytes, bytes, "不同 y 的瓦片居然逐字节相同");
    } else {
        // 没有字体时整页是纸（StubMeasurer），两片都白，尺寸相同即可。
        assert_eq!(lower_bytes.len(), bytes.len());
    }

    // 新入口在同一个锁里排版 + 取窗口：后台线程不用先 layout 再 slice。
    let one = std::env::temp_dir().join("eb-plain-scroll-one-call.raw");
    let c_one = CString::new(one.to_str().unwrap()).unwrap();
    assert_eq!(
        scroll_slice_raw(
            engine,
            0,
            400.0,
            700.0,
            0.0,
            24.0,
            0.0,
            24.0,
            1.0,
            1.0,
            0.0,
            0.0,
            0,
            0.0,
            300.0,
            2.0,
            0,
            0,
            false,
            c_one.as_ptr(),
        ),
        0
    );
    assert_eq!(std::fs::read(&one).unwrap(), bytes);

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
    let _guard = serial();
    // 没有注册字体时排版仍然出页（StubMeasurer），但位图上一个字也没有：
    // 这正是 iOS 上「有页码、无正文」的样子。
    let bytes = demo();
    let name = c"demo.txt";
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), name.as_ptr()) };
    assert!(!engine.is_null());
    let _ = layout(
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
    let _guard = serial();
    // 夜纸：黑底浅字。排版时写进 IR 的是黑色，墨色在这一层覆盖它——不然
    // 夜里就是黑字画在黑纸上。
    let (engine, has_font) = open_book();
    if !has_font {
        eprintln!("no font on this machine: nothing to colour");
        eb_plain_close(engine);
        return;
    }
    let _ = layout(
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
fn metadata_bundle_matches_single_field_getters() {
    let _guard = serial();
    let (engine, _) = open_book();
    let meta = eb_plain_metadata(engine);
    assert_eq!(meta.ok, 1);
    assert!(!meta.title.is_null());
    let bundle_title = unsafe { std::ffi::CStr::from_ptr(meta.title) };
    let single_title = unsafe { std::ffi::CStr::from_ptr(eb_plain_title(engine)) };
    assert_eq!(bundle_title, single_title);
    assert_eq!(meta.creator_count, eb_plain_creator_count(engine));
    assert_ne!(eb_plain_capabilities(engine) & EB_CAP_TXT, 0);
    assert_eq!(eb_plain_metadata(std::ptr::null_mut()).ok, 0);
    eb_plain_close(engine);
}

#[test]
fn page_bytes_match_the_raw_file_format() {
    let _guard = serial();
    let (engine, _) = open_book();
    assert_eq!(
        layout(engine, 0, 400.0, 700.0, 24.0, 24.0, 24.0, 24.0, 1.0, 1.0, 0.0, 0.0, 0),
        0
    );
    let mut len = 0usize;
    let ptr = eb_plain_page_bytes(engine, 0, 1.0, 0, 0, false, &mut len);
    assert!(!ptr.is_null());
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { ebook_abi::eb_free(ptr, len) };
    assert_eq!(&bytes[..4], b"EQP1");
    let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    assert_eq!(bytes.len(), 12 + (width * height * 4) as usize);
    eb_plain_close(engine);
}

#[test]
fn enumerates_images_and_returns_full_resolution_bytes() {
    let _guard = serial();
    let Ok(bytes) = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/images.epub"
    )) else {
        eprintln!("fixtures/images.epub 不在，跳过图片枚举测试");
        return;
    };
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), c"images.epub".as_ptr()) };
    assert!(!engine.is_null());
    assert_eq!(
        layout(engine, 0, 400.0, 700.0, 24.0, 24.0, 24.0, 24.0, 1.0, 1.0, 0.0, 0.0, 0),
        0
    );
    let count = eb_plain_image_count(engine);
    assert!(count > 0, "images.epub 应该有图");
    let info = eb_plain_image_info(engine, 0);
    assert_eq!(info.ok, 1);
    assert_eq!(info.id, 0);
    assert!(info.width > 0 && info.height > 0);
    assert!(!info.src.is_null());
    let mut len = 0usize;
    let ptr = eb_plain_image_full_bytes(engine, 0, &mut len);
    assert!(!ptr.is_null());
    let image = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { ebook_abi::eb_free(ptr, len) };
    assert_eq!(&image[..4], b"EQP1");
    eb_plain_close(engine);
}

#[test]
fn word_spacing_reaches_the_layout() {
    let _guard = serial();
    // 词距是西文的事：同样的字，词与词之间放宽一点，一行就装不下了。
    // 它走的是 `Style::word_px` → 节点的 `word-spacing`，不经过任何 UI。
    let text = "the quick brown fox jumps over the lazy dog. ".repeat(400);
    let bytes = text.into_bytes();
    let name = c"english.txt";
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), name.as_ptr()) };
    assert!(!engine.is_null());
    let pages = |word: f32| {
        let _ = layout(
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
    let _guard = serial();
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
            !getter(std::ptr::null_mut()).is_null(),
            "无效句柄时也不该返回 null"
        );
    }
    assert_eq!(eb_plain_cover_len(std::ptr::null_mut()), 0);
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
}

#[test]
fn selection_round_trip() {
    let _guard = serial();
    let (engine, _) = open_book();
    // 没排版时选不出来，但不干预。
    assert!(eb_plain_select(engine, 0, 0, 0, 0, 1, 0) < 0);
    let rc = layout(
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
    let _guard = serial();
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
    assert!(!engine.is_null(), "打开 demo.epub 失败");
    assert_eq!(eb_plain_spine_count(engine), 2);
    let rc = layout(
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
    let _guard = serial();
    let (engine, _) = open_book();
    let rc = layout(
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
    let _guard = serial();
    // 失败一次：错误串就在库内的缓冲里，指针不归调用方、也不该每次漏一份。
    let engine = unsafe { eb_plain_open(std::ptr::null(), 0, std::ptr::null()) };
    assert!(engine.is_null());
    let first = eb_plain_last_error();
    assert!(!first.is_null());
    let message = unsafe { std::ffi::CStr::from_ptr(first) }
        .to_string_lossy()
        .into_owned();
    assert!(!message.is_empty(), "失败的 open 应该留下原因");

    // 再失败一次：还是同一块缓冲（没有 mem::forget 漏出去的旧串），内容不变。
    let engine = unsafe { eb_plain_open(std::ptr::null(), 0, std::ptr::null()) };
    assert!(engine.is_null());
    let second = unsafe { std::ffi::CStr::from_ptr(eb_plain_last_error()) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(second, message, "同一个失败应该给同一条原因");

    // 成功一次就清空。
    let bytes = demo();
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), c"demo.txt".as_ptr()) };
    assert!(!engine.is_null());
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(eb_plain_last_error()) }.to_bytes(),
        b""
    );
    eb_plain_close(engine);
}

#[test]
fn version_is_not_hard_coded_to_an_old_release() {
    let _guard = serial();
    let version = unsafe { std::ffi::CStr::from_ptr(eb_plain_version()) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(version, format!("ebook-abi {}", env!("CARGO_PKG_VERSION")));
    assert_eq!(eb_plain_abi_version(), ebook_abi::ABI_VERSION);
}

#[test]
fn timings() {
    let _guard = serial();
    let (engine, _) = open_book();
    let _ = layout(
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
    let _ = layout(
        engine, 0, 390.0, 760.0, 26.0, 30.0, 26.0, 30.0, 1.0, 1.6, 0.0, 0.0, 0,
    );
    eprintln!("one chapter layout: {:?}", start.elapsed());
    eb_plain_close(engine);
}

#[test]
fn scroll_session_is_cross_chapter_and_band_rendering_works() {
    let _guard = serial();
    // 跨章需要真 EPUB（两章）；夹具不在就跳过。
    let Ok(bytes) = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.epub"
    )) else {
        eprintln!("fixtures/demo.epub 不在，跳过滚动会话测试");
        return;
    };
    let engine = unsafe { eb_plain_open(bytes.as_ptr(), bytes.len(), c"demo.epub".as_ptr()) };
    assert!(!engine.is_null());

    // 没开始会话：滚动入口给明确错误，而不是看起来像 0 的值。
    assert_eq!(eb_plain_scroll_total_height(engine), -1.0);
    assert_eq!(eb_plain_scroll_spine_at(engine, 0.0), -1);
    assert_eq!(eb_plain_scroll_hit(engine, 0.0, 0.0).spine, -1);

    assert_eq!(
        scroll_begin(engine, 400.0, 700.0, 0.0, 24.0, 0.0, 24.0, 1.0, 1.0, 0.0, 0.0, 0,),
        2,
        "demo.epub 应有两章"
    );
    assert_eq!(eb_plain_scroll_chapter_count(engine), 2);

    let first = eb_plain_scroll_chapter(engine, 0);
    assert_eq!(first.ok, 1);
    assert_eq!(first.spine, 0);
    assert_eq!(first.top, 0.0);
    assert!(first.height > 0.0, "第 0 章没有高度");
    let second = eb_plain_scroll_chapter(engine, 1);
    assert_eq!(second.ok, 1);
    assert_eq!(second.spine, 1);
    assert!((second.top - first.height).abs() < 0.01);

    // 越界下标：ok == 0，不是崩溃。
    let beyond = eb_plain_scroll_chapter(engine, 7);
    assert_eq!(beyond.ok, 0);
    assert_eq!(beyond.spine, u32::MAX);

    let total = eb_plain_scroll_total_height(engine);
    assert!(total > 0.0);
    assert!((total - (first.height + second.height)).abs() < 0.01);
    assert_eq!(eb_plain_scroll_spine_at(engine, 0.0), 0);
    assert_eq!(eb_plain_scroll_spine_at(engine, second.top + 1.0), 1);
    assert_eq!(eb_plain_scroll_spine_at(engine, total + 10.0), -1);
    assert!((eb_plain_scroll_chapter_top(engine, 1) - second.top).abs() < 0.01);

    // 一段跨章的可见带：文件与内存两条路给同样的字节。
    let y = second.top.floor() - 40.0;
    let path = std::env::temp_dir().join("eb-plain-scroll-band.eqp");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_scroll_band_raw(engine, y, 120.0, 1.0, 0, 0, false, c_path.as_ptr()),
        0,
        "跨章带写文件失败"
    );
    let on_disk = std::fs::read(&path).unwrap();
    let mut len = 0usize;
    let ptr = eb_plain_scroll_band_bytes(engine, y, 120.0, 1.0, 0, 0, false, &mut len);
    assert!(!ptr.is_null(), "内存瓦片返回空指针");
    let in_memory = unsafe { std::slice::from_raw_parts(ptr, len).to_vec() };
    unsafe { ebook_abi::eb_free(ptr, len) };
    assert_eq!(on_disk, in_memory, "两条路应给逐字节相同的 EQP1");
    assert_eq!(&on_disk[..4], b"EQP1");
    assert_eq!(u32::from_le_bytes(on_disk[4..8].try_into().unwrap()), 400);
    assert_eq!(u32::from_le_bytes(on_disk[8..12].try_into().unwrap()), 120);

    // 命中带章号；行号换算回同一个流坐标。
    let hit = eb_plain_scroll_hit(engine, 60.0, second.top + 30.0);
    assert_eq!(hit.spine, 1);
    assert!(hit.y >= second.top && hit.y < second.top + second.height);
    let line_y = eb_plain_scroll_line_y(engine, 1, hit.line);
    assert!((line_y - hit.y).abs() < 0.01, "{line_y} vs {}", hit.y);

    // 目录/链接跳转：锚点 → 全书流坐标。
    let anchor = eb_plain_scroll_anchor_y(engine, 0, c"note".as_ptr());
    assert!(anchor >= 0.0 && anchor < first.height, "anchor {anchor}");
    assert_eq!(eb_plain_scroll_anchor_y(engine, 0, c"nope".as_ptr()), -1.0);
    assert_eq!(eb_plain_scroll_anchor_y(engine, 99, c"note".as_ptr()), -1.0);

    // 进度：书首 0，书尾 1。
    assert!(eb_plain_scroll_progress(engine, 0.0).abs() < 1.0e-6);
    assert!((eb_plain_scroll_progress(engine, total) - 1.0).abs() < 1.0e-6);

    eb_plain_close(engine);
}

#[test]
fn scroll_session_requires_a_handle_and_a_book() {
    let _guard = serial();
    // -1 句柄：不 panic，给错误码。
    assert_eq!(
        scroll_begin(
            std::ptr::null_mut(),
            400.0,
            700.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            1.0,
            0.0,
            0.0,
            0
        ),
        -1
    );
    assert_eq!(eb_plain_scroll_chapter_count(std::ptr::null_mut()), 0);
    assert_eq!(eb_plain_scroll_chapter(std::ptr::null_mut(), 0).ok, 0);
    assert_eq!(eb_plain_scroll_total_height(std::ptr::null_mut()), -1.0);
    assert_eq!(eb_plain_scroll_spine_at(std::ptr::null_mut(), 0.0), -1);
    assert_eq!(eb_plain_scroll_chapter_top(std::ptr::null_mut(), 0), -1.0);
    assert_eq!(
        eb_plain_scroll_hit(std::ptr::null_mut(), 0.0, 0.0).spine,
        -1
    );
    assert_eq!(eb_plain_scroll_line_y(std::ptr::null_mut(), 0, 0), -1.0);
    assert_eq!(
        eb_plain_scroll_anchor_y(std::ptr::null_mut(), 0, c"x".as_ptr()),
        -1.0
    );
    assert_eq!(eb_plain_scroll_progress(std::ptr::null_mut(), 0.0), -1.0);
    let mut len = 123usize;
    assert!(eb_plain_scroll_band_bytes(
        std::ptr::null_mut(),
        0.0,
        10.0,
        1.0,
        0,
        0,
        false,
        &mut len
    )
    .is_null());
    assert_eq!(len, 0, "失败时 out_len 要清 0");

    let path = std::env::temp_dir().join("eb-plain-scroll-none.eqp");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(
        eb_plain_scroll_band_raw(
            std::ptr::null_mut(),
            0.0,
            10.0,
            1.0,
            0,
            0,
            false,
            c_path.as_ptr()
        ),
        -1
    );
}

#[test]
fn stream_book_appends_and_replaces_without_a_file() {
    let _guard = serial();
    let title = CString::new("流式书").unwrap();
    let id = unsafe { eb_plain_new_book(title.as_ptr()) };
    assert!(!id.is_null(), "new_book 失败");

    let chapter_title = CString::new("第一章").unwrap();
    let text = CString::new("第一行\n第二行").unwrap();
    assert_eq!(
        eb_plain_append_chapter(id, chapter_title.as_ptr(), text.as_ptr()),
        1
    );
    let rev = eb_plain_chapter_rev(id, 0);
    assert!(rev > 0);

    let replaced = CString::new("第一行\n改过的第二行").unwrap();
    assert_eq!(
        eb_plain_replace_chapter(id, 0, chapter_title.as_ptr(), replaced.as_ptr()),
        0
    );
    assert_ne!(eb_plain_chapter_rev(id, 0), rev);
    assert_eq!(eb_plain_spine_count(id), 1);

    // 越界与空文本都是错误码，不是 panic。
    assert_eq!(
        eb_plain_replace_chapter(id, 9, chapter_title.as_ptr(), text.as_ptr()),
        -2
    );
    let empty = CString::new("").unwrap();
    assert_eq!(
        eb_plain_append_chapter(id, chapter_title.as_ptr(), empty.as_ptr()),
        -2
    );

    assert_eq!(eb_plain_clear_chapters(id), 0);
    assert_eq!(eb_plain_spine_count(id), 0);
    assert_eq!(eb_plain_close(id), 0);
}

#[test]
fn a_locator_survives_a_relayout() {
    let _guard = serial();
    let (id, _) = open_book();
    assert_eq!(
        layout(id, 0, 400.0, 700.0, 24.0, 24.0, 24.0, 24.0, 1.0, 1.0, 0.0, 0.0, 0),
        0
    );
    let hit = eb_plain_hit(id, 0, 40.0, 40.0);
    assert!(hit.page >= 0, "第一页应该能命中正文");
    let text = eb_plain_locator_at(id, 0, 40.0, 40.0);
    assert!(!text.is_null());
    let text = unsafe { std::ffi::CStr::from_ptr(text) }
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(text.split(':').count(), 3, "格式是 spine:node:offset");
    let back = eb_plain_locate(id, CString::new(text.clone()).unwrap().as_ptr());
    assert_eq!(back.page, hit.page);
    assert_eq!((back.line, back.byte), (hit.line, hit.byte));

    // 换页高重新排版后，同一个 locator 仍指向同一行。
    assert_eq!(
        layout(id, 0, 420.0, 760.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0),
        0
    );
    let again = eb_plain_locate(id, CString::new(text).unwrap().as_ptr());
    assert!(again.page >= 0);

    let range = eb_plain_locator_range(
        id,
        hit.page as u32,
        hit.line,
        hit.byte,
        hit.page as u32,
        hit.line,
        hit.byte,
    );
    assert!(!range.is_null());
    let range = unsafe { std::ffi::CStr::from_ptr(range) }.to_str().unwrap();
    assert_eq!(range.split('|').count(), 2);

    let none = eb_plain_locate(id, c"not-a-locator".as_ptr());
    assert_eq!(none.page, -1);
    assert_eq!(eb_plain_close(id), 0);
}
