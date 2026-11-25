//! 稳定 Locator（Anchor / AnchorRange）与块级窗口排版。

use ebook_reader::{Engine, Style, Viewport};

fn viewport() -> Viewport {
    Viewport::new(400.0, 700.0).with_margin(24.0)
}

fn book() -> Engine {
    let mut engine = Engine::new_book("块窗口");
    let text = (0..30)
        .map(|i| format!("第 {i} 段。这一段用来确认窗口只排自己，不排前面的内容。"))
        .collect::<Vec<_>>()
        .join("\n");
    engine.append_chapter("第一章", &text).unwrap();
    engine
}

#[test]
fn anchor_round_trips_and_covers_a_range() {
    let mut engine = book();
    engine.layout(0, viewport(), Style::default()).unwrap();
    let ir = engine.ir().unwrap();
    let page = &ir.pages[0];
    let line = &ir.runs.lines[3].rect;
    let point = |line: &ebook_ir::RectF| {
        (
            line.x + line.w * 0.5 - page.origin.x + page.inset.left,
            line.y + 2.0 - page.origin.y + page.inset.top,
        )
    };
    let (x, y) = point(line);
    let anchor = engine.anchor_at(0, x, y).expect("anchor");
    assert_eq!(anchor.spine, 0);

    let location = engine.locate(&anchor).expect("location");
    assert_eq!(location.page, 0);
    assert_eq!(location.line, 3);
    assert!(location.byte <= ir.runs.lines[3].src.len);

    let end = &ir.runs.lines[5].rect;
    let (x, y) = point(end);
    let end_anchor = engine.anchor_at(0, x, y).expect("end anchor");
    let end_location = engine.locate(&end_anchor).unwrap();
    let range = engine.anchor_range(location, end_location).expect("range");
    assert_eq!(range.start, anchor);
    assert_eq!(range.end, end_anchor);
    assert_ne!(range.start, range.end);

    // 宿主实际持久化的是 Locator（章 + 节点 + 字节），不是页/行。
    let (sx, sy) = point(line);
    let locator = engine.locator_at(0, sx, sy).expect("locator");
    assert_eq!(engine.locate_locator(&locator), Some(location));
    assert!(engine.block_of_locator(&locator).is_some());
    let stable = engine
        .locator_range(location, end_location)
        .expect("locator range");
    assert_eq!(stable.start, locator);
}

#[test]
fn a_block_window_only_contains_the_requested_block() {
    let mut engine = book();
    engine.layout(0, viewport(), Style::default()).unwrap();
    let full_lines = engine.ir().unwrap().runs.lines.len();
    let ir = engine.ir().unwrap();
    let page = &ir.pages[0];
    let line = &ir.runs.lines[20].rect;
    let (x, y) = (
        line.x + line.w * 0.5 - page.origin.x + page.inset.left,
        line.y + 2.0 - page.origin.y + page.inset.top,
    );
    let anchor = engine.anchor_at(0, x, y).expect("anchor");
    let block = engine.block_of_anchor(&anchor).expect("block");

    engine
        .layout_block_window(0, block, block, viewport(), Style::default())
        .unwrap();
    let win = engine.ir().unwrap();
    assert!(!win.runs.lines.is_empty());
    assert!(win.runs.lines.len() < full_lines);
    assert!(win.blocks.iter().any(|b| b.node == block));
    assert!(win.pages[0].origin.y.abs() < 0.01);
    assert!(!engine.blocks().is_empty());

    let page = &win.pages[0];
    let line = &win.runs.lines[0].rect;
    let back = engine
        .anchor_at(
            0,
            line.x + line.w * 0.5 - page.origin.x + page.inset.left,
            line.y + 1.0 - page.origin.y + page.inset.top,
        )
        .expect("window anchor");
    assert!(engine.locate(&back).is_some());
}

#[test]
fn block_window_rejects_a_spine_mismatch() {
    let mut engine = book();
    engine.layout(0, viewport(), Style::default()).unwrap();
    let anchor = engine.anchor_at(0, 30.0, 30.0).unwrap_or_default();
    let mut other = anchor.clone();
    other.spine = 9;
    assert!(engine.locate(&other).is_none());
    assert!(engine.block_of_anchor(&other).is_none());
}
