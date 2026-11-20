//! `page_bitmap` 的对外契约：8 字节头 + RGBA、尺度、越界与资源缺省的兜底。
//!
//! 这些用例共享 `BOOK` / `FONT` / `LAST` 三个进程级状态，所以放在同一个
//! `#[test]` 里按顺序跑，不用多线程并发。

#![cfg(feature = "epub")]

use ebook_web::{layout_cached, open_book, open_text, page_bitmap, register_font, reset_fonts};

const WIDTH: f32 = 400.0;
const HEIGHT: f32 = 600.0;
const MARGIN: f32 = 20.0;
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn demo() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.epub"
    ))
    .expect("fixtures/demo.epub")
}

fn latin() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/font/test.ttf"
    ))
    .expect("fixtures/font/test.ttf")
}

/// 解析头部，顺便检查「头 + 像素」的长度关系。
fn unpack(bitmap: Vec<u8>) -> (u32, u32, Vec<u8>) {
    assert!(bitmap.len() >= 8, "header");
    let w = u32::from_le_bytes(bitmap[0..4].try_into().unwrap());
    let h = u32::from_le_bytes(bitmap[4..8].try_into().unwrap());
    assert_eq!(bitmap.len(), 8 + w as usize * h as usize * 4, "pixel bytes");
    (w, h, bitmap[8..].to_vec())
}

#[test]
fn page_bitmap_contract() {
    let opened = open_book(&demo());
    assert!(!opened.starts_with("{\"error\""), "open: {opened}");

    // 1. 尺度：页面盒是排版尺寸 + 页边距，DPR 只乘像素。
    let ir = layout_cached(0, WIDTH, HEIGHT, MARGIN);
    assert!(!ir.is_empty(), "layout");
    let (w1, h1, _) = unpack(page_bitmap(0, 1.0, 0, 0, false));
    let (w2, h2, _) = unpack(page_bitmap(0, 2.0, 0, 0, false));
    // 排版尺寸是浮点，盒宽可能有 ±1 的取整差。
    assert!((w1 as i64 - WIDTH as i64).abs() <= 1, "w1 = {w1}");
    assert!((h1 as i64 - HEIGHT as i64).abs() <= 1, "h1 = {h1}");
    assert_eq!(w2, w1 * 2);
    assert_eq!(h2, h1 * 2);

    // 2. `scale` 非法值按 1 处理，不能变成 0 像素或 NaN。
    let (w0, h0, _) = unpack(page_bitmap(0, 0.0, 0, 0, false));
    assert_eq!((w0, h0), (w1, h1));
    let (nan_w, _, _) = unpack(page_bitmap(0, f32::NAN, 0, 0, false));
    assert_eq!(nan_w, w1);

    // 3. 没有装字体时也能出图：字形退回占位，但页面几何仍然成立。
    reset_fonts();
    layout_cached(0, WIDTH, HEIGHT, MARGIN);
    let (_, _, pixels) = unpack(page_bitmap(0, 1.0, 0, 0, false));
    assert_eq!(&pixels[0..4], &WHITE, "no-font page keeps the paper white");

    // 4. 装了字体之后页面上真的落了墨：不是整页空白。
    assert_ne!(register_font(&latin()), u32::MAX, "builtin font");
    layout_cached(0, WIDTH, HEIGHT, MARGIN);
    let (_, _, pixels) = unpack(page_bitmap(0, 1.0, 0, 0, false));
    let inked = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| **px != WHITE)
        .count();
    assert!(inked > 0, "font page has ink");

    // 5. 墨色/纸面是入参，宿主据此表达 sepia 之类，而不是靠滤镜猜。
    let (_, _, sepia) = unpack(page_bitmap(0, 1.0, 0x3C3022FF, 0xEFE4CFFF, false));
    assert_eq!(&sepia[0..4], &[0xef, 0xe4, 0xcf, 255], "paper color");
    let inked_sepia = sepia
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| **px != [0xef, 0xe4, 0xcf, 255])
        .count();
    assert!(inked_sepia > 0, "sepia page has ink");

    // 6. 全局加粗是绘制阶段的开关：同一份 IR，换 face 后墨量应该变多。
    //    换一本拉丁文的书：内置字体没有 CJK 字形，比对墨量没有意义。
    let opened = open_text("Bold sample text", "bold.txt");
    assert!(!opened.starts_with("{\"error\""), "open text: {opened}");
    layout_cached(0, WIDTH, HEIGHT, MARGIN);
    let (_, _, plain) = unpack(page_bitmap(0, 1.0, 0, 0, false));
    let (_, _, bold) = unpack(page_bitmap(0, 1.0, 0, 0, true));
    let plain_ink = plain
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| **px != WHITE)
        .count();
    let bold_ink = bold
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| **px != WHITE)
        .count();
    assert!(
        bold_ink > plain_ink,
        "bold = {bold_ink}, plain = {plain_ink}"
    );

    // 7. 越界页号、还没打开书都返回空数组。
    assert!(page_bitmap(9999, 1.0, 0, 0, false).is_empty());

    // 8. 像素上限：scale 被夹到 4，超大页面也不会把 wasm 内存一次吃干。
    let (clamped_w, _, _) = unpack(page_bitmap(0, 1000.0, 0, 0, false));
    assert_eq!(clamped_w, w1 * 4);
}
