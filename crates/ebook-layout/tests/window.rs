//! 块级窗口：只排目标块，坐标归一化到 0，窗口外不产出行/op。

use ebook_format::xhtml;
use ebook_ir::{NodeId, NodeKind, StyledDoc, Tag};
use ebook_layout::{
    layout, LayoutEnv, LayoutOptions, Mode, NoImageSizes, PageConstraint, StubMeasurer,
};

fn doc(paragraphs: usize) -> StyledDoc {
    let mut body = String::new();
    for i in 0..paragraphs {
        body.push_str(&format!("<p>第 {i} 段，窗口测试用的正文。</p>"));
    }
    let html = format!("<html><body>{body}</body></html>");
    xhtml::build(&html, 0, &mut xhtml::NoAssets).expect("xhtml")
}

fn paragraph(doc: &StyledDoc, nth: usize) -> NodeId {
    doc.nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| matches!(node.kind, NodeKind::Element(Tag::P)))
        .nth(nth)
        .map(|(id, _)| id as NodeId)
        .expect("paragraph")
}

fn env() -> LayoutEnv<'static> {
    LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    }
}

#[test]
fn a_block_window_lays_out_only_the_requested_blocks() {
    let doc = doc(40);
    let from = paragraph(&doc, 30);
    let to = paragraph(&doc, 31);
    let c = PageConstraint::new(320.0, 480.0);

    let full = layout(&doc, &env(), &c);
    let mut opts = LayoutOptions::scroll();
    opts.window = Some(ebook_layout::BlockWindow { from, to });
    let window = layout(&doc, &env(), &c); // 先确认默认不受影响
    let win = {
        let mut l = ebook_layout::Layout::new();
        l.layout_with(&doc, &env(), &c, &opts)
    };

    assert!(full.runs.lines.len() > win.runs.lines.len());
    assert!(!win.runs.lines.is_empty());
    assert!(!win.blocks.is_empty());
    assert_eq!(window.runs.lines.len(), full.runs.lines.len());

    let from_path = doc.node(from).src.as_ref().expect("src").path.clone();
    let to_path = doc.node(to).src.as_ref().expect("src").path.clone();
    for line in &win.runs.lines {
        let anchor = line.anchor.as_ref().expect("line anchor");
        assert!(
            anchor.path.starts_with(&from_path) || anchor.path.starts_with(&to_path),
            "窗口外的行漏进来了: {:?}",
            anchor.path
        );
    }

    // 窗口坐标从 0 附近开始，不带着前面 30 段的高度。
    let top = win
        .blocks
        .iter()
        .filter(|b| b.lines > 0)
        .map(|b| b.rect.y)
        .fold(f32::INFINITY, f32::min);
    assert!(top.abs() < 0.01, "top = {top}");
    assert!(win.pages[0].origin.y.abs() < 0.01);
}

#[test]
fn a_window_inside_one_paragraph_keeps_the_paragraph_whole() {
    let doc = doc(3);
    let p = paragraph(&doc, 1);
    let c = PageConstraint::new(200.0, 300.0);
    let opts = LayoutOptions {
        mode: Mode::Scroll,
        window: Some(ebook_layout::BlockWindow { from: p, to: p }),
        ..LayoutOptions::default()
    };
    let win = ebook_layout::Layout::new().layout_with(&doc, &env(), &c, &opts);
    assert!(!win.runs.lines.is_empty());
    let path = doc.node(p).src.as_ref().unwrap().path.clone();
    assert!(win
        .runs
        .lines
        .iter()
        .all(|l| l.anchor.as_ref().is_some_and(|a| a.path.starts_with(&path))));
}

#[test]
fn an_invalid_window_falls_back_to_the_full_chapter() {
    let doc = doc(2);
    let c = PageConstraint::new(200.0, 300.0);
    let opts = LayoutOptions {
        mode: Mode::Scroll,
        window: Some(ebook_layout::BlockWindow {
            from: 999_999,
            to: 999_999,
        }),
        ..LayoutOptions::default()
    };
    let win = ebook_layout::Layout::new().layout_with(&doc, &env(), &c, &opts);
    let full = layout(&doc, &env(), &c);
    assert_eq!(win.runs.lines.len(), full.runs.lines.len());
}

#[test]
fn a_reversed_window_falls_back_to_the_full_chapter() {
    let doc = doc(4);
    let from = paragraph(&doc, 3);
    let to = paragraph(&doc, 1);
    let c = PageConstraint::new(200.0, 300.0);
    let opts = LayoutOptions {
        mode: Mode::Scroll,
        window: Some(ebook_layout::BlockWindow { from, to }),
        ..LayoutOptions::default()
    };
    let win = ebook_layout::Layout::new().layout_with(&doc, &env(), &c, &opts);
    let full = layout(&doc, &env(), &c);
    assert_eq!(win.runs.lines.len(), full.runs.lines.len());
}
