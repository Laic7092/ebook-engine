//! `page_meta` / `fragment_line` 的对外契约：宿主不解析 IR 也能翻页与跳锚点。
//!
//! 与 `page_bitmap.rs` 一样共享 `BOOK` / `FONT` / `LAST`，放在同一个 `#[test]`
//! 里顺序跑。

#![cfg(feature = "epub")]

use ebook_web::{fragment_line, layout_cached, open_book, page_meta};

// 页面故意小一些：demo 第一章要落成多页，行区间与进度才有可测的跨度。
const WIDTH: f32 = 260.0;
const HEIGHT: f32 = 220.0;
const MARGIN: f32 = 20.0;
/// 每页 9 个 f32：w, h, left, right, top, bottom, line_start, line_len, progress。
const STRIDE: usize = 9;

fn demo() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.epub"
    ))
    .expect("fixtures/demo.epub")
}

fn unpack(bytes: &[u8]) -> Vec<[f32; STRIDE]> {
    assert!(bytes.len() >= 4, "meta header");
    let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), 4 + count * STRIDE * 4, "meta length");
    (0..count)
        .map(|index| {
            let mut page = [0.0_f32; STRIDE];
            for (slot, value) in page.iter_mut().enumerate() {
                let at = 4 + (index * STRIDE + slot) * 4;
                *value = f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            }
            page
        })
        .collect()
}

#[test]
fn page_meta_and_fragment_line_describe_the_current_chapter() {
    let info = open_book(&demo());
    assert!(!info.contains("error"), "{info}");
    assert!(
        !layout_cached(0, WIDTH, HEIGHT, MARGIN).is_empty(),
        "layout"
    );
    let pages = unpack(&page_meta());
    assert!(!pages.is_empty(), "pages");

    for (index, page) in pages.iter().enumerate() {
        let (w, h, left, right, top, bottom) =
            (page[0], page[1], page[2], page[3], page[4], page[5]);
        // 页面盒子就是排版入参：宽高与页边距必须原样回来。
        assert!((w + left + right - WIDTH).abs() < 0.5, "page {index} width");
        assert!(
            (h + top + bottom - HEIGHT).abs() < 0.5,
            "page {index} height"
        );
        assert!(left >= 0.0 && top >= 0.0, "page {index} insets");
        // 行区间连续、有序，最后一个页码正好覆盖全章所有行。
        assert!(page[7] > 0.0, "page {index} has no lines");
        assert!((0.0..=1.0).contains(&page[8]), "page {index} progress");
        if index > 0 {
            let previous = pages[index - 1];
            assert_eq!(previous[6] + previous[7], page[6], "page {index} line gap");
            assert!(page[8] >= previous[8], "page {index} progress went back");
        }
    }
    // 首尾进度夹在 0..=1 之间，首页从 0 开始。
    assert!(pages[0][8] <= 0.05, "first page progress {}", pages[0][8]);
    assert!(
        pages.len() > 1,
        "test page size should split the chapter into pages"
    );
    assert!(pages[pages.len() - 1][8] > 0.0, "last page progress");

    // demo.epub 第一章带 id="n" 的锚点（与 contract.mjs 用的是同一个）。
    let line = fragment_line("note");
    assert_ne!(line, u32::MAX, "anchor note");
    let pages = unpack(&page_meta());
    assert!(
        pages
            .iter()
            .any(|page| line as f32 >= page[6] && (line as f32) < page[6] + page[7]),
        "line {line} is inside the chapter"
    );

    assert_eq!(fragment_line("no-such-anchor"), u32::MAX);
    // 空 fragment 不是锚点：查不到就是查不到，不 panic。
    assert_eq!(fragment_line(""), u32::MAX);

    // 换书：交互状态跟着上一本书，打开新书后必须先清空，等新排版。
    open_book(&demo());
    // 新的书打开后还没排版：元数据必须是空数组，而不是上一章的残留。
    let bytes = page_meta();
    assert_eq!(bytes.len(), 4, "no pages yet");
    assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 0);
    assert_eq!(fragment_line("note"), u32::MAX);
}
