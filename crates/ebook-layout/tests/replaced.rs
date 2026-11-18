//! 行内图片与 ruby：不再静默丢节点，尺寸未知时留占位与诊断。

use ebook_ir::doc::FontSpec;
use ebook_ir::{
    Anchor, ComputedStyle, Dir, FontId, ImageId, LayoutResult, LineHeight, Node, NodeId, NodeKind,
    Op, Rgba8, SizeF, Span, StyledDoc, Tag, UnresolvedReason,
};
use ebook_layout::{layout, LayoutEnv, PageConstraint, ResourceView, TextMeasurer};

struct Fixed;

impl TextMeasurer for Fixed {
    fn advance(&self, _ch: char, _font: FontId, _size: f32) -> f32 {
        10.0
    }

    fn ascent(&self, _font: FontId, _size: f32) -> f32 {
        10.0
    }

    fn descent(&self, _font: FontId, _size: f32) -> f32 {
        2.0
    }
}

struct Sizes;

impl ResourceView for Sizes {
    fn image_size(&self, id: ImageId) -> Option<SizeF> {
        match id {
            0 => Some(SizeF::new(20.0, 30.0)),
            _ => None,
        }
    }
}

fn block_style() -> ComputedStyle {
    ComputedStyle {
        font: FontSpec {
            font: FontId(0),
            size: 10.0,
            weight: 400,
            ..FontSpec::default()
        },
        line_height: LineHeight::Px(12.0),
        ..ComputedStyle::default()
    }
}

fn inline_style() -> ComputedStyle {
    ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 10.0,
            weight: 400,
            ..FontSpec::default()
        },
        Rgba8::BLACK,
        Dir::Ltr,
    )
}

fn push(
    doc: &mut StyledDoc,
    kind: NodeKind,
    style: ComputedStyle,
    parent: Option<NodeId>,
) -> NodeId {
    doc.push_node(Node {
        kind,
        style,
        parent,
        first_child: 0,
        child_len: 0,
        src: Some(Anchor::default()),
        link: None,
    })
}

fn text_node(doc: &mut StyledDoc, body: &str, parent: Option<NodeId>) -> NodeId {
    let start = doc.text.len() as u32;
    doc.text.push_str(body);
    push(
        doc,
        NodeKind::Text(Span::new(start, body.len() as u32)),
        inline_style(),
        parent,
    )
}

fn set_kids(doc: &mut StyledDoc, parent: NodeId, kids: &[NodeId]) {
    doc.set_kids(parent, kids);
    for (index, &kid) in kids.iter().enumerate() {
        doc.node_mut(kid).parent = Some(parent);
        doc.node_mut(kid).src = Some(Anchor {
            spine: 0,
            path: [index as u16].to_vec(),
            offset: 0,
        });
    }
}

fn run(doc: &StyledDoc) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &Sizes,
    };
    layout(doc, &env, &PageConstraint::new(400.0, 800.0))
}

#[test]
fn inline_image_uses_its_intrinsic_size() {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = push(&mut doc, NodeKind::Element(Tag::Body), block_style(), None);
    doc.root = root;
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(), None);
    let a = text_node(&mut doc, "a", Some(p));
    let img = push(&mut doc, NodeKind::Image(0), inline_style(), Some(p));
    let b = text_node(&mut doc, "b", Some(p));
    set_kids(&mut doc, p, &[a, img, b]);
    set_kids(&mut doc, root, &[p]);

    let ir = run(&doc);
    let image = ir
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::Image { id, rect, .. } => Some((*id, *rect)),
            _ => None,
        })
        .expect("inline image op");
    assert_eq!(image.0, 0);
    assert!((image.1.w - 20.0).abs() < 0.01);
    assert!((image.1.h - 30.0).abs() < 0.01);
    // 行宽 = a + 图片 + b。
    assert!((ir.runs.lines[0].rect.w - 40.0).abs() < 0.01);
    // 图片底边贴基线，行盒至少和图片一样高。
    assert!(ir.runs.lines[0].rect.h >= 30.0);
}

#[test]
fn unknown_inline_image_is_reported_not_dropped() {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = push(&mut doc, NodeKind::Element(Tag::Body), block_style(), None);
    doc.root = root;
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(), None);
    let a = text_node(&mut doc, "a", Some(p));
    let img = push(&mut doc, NodeKind::Image(7), inline_style(), Some(p));
    set_kids(&mut doc, p, &[a, img]);
    set_kids(&mut doc, root, &[p]);

    let ir = run(&doc);
    let unresolved = ir
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::Unresolved(u) if u.reason == UnresolvedReason::SizeFromContent => Some(u.rect),
            _ => None,
        })
        .expect("SizeFromContent placeholder");
    // 1em 宽的占位，行高可预测（>= 1.5em）。
    assert!((unresolved.w - 10.0).abs() < 0.01);
    assert!(unresolved.h >= 15.0);
    assert!(ir
        .dirty
        .iter()
        .any(|d| d.reason == UnresolvedReason::SizeFromContent));
    assert!(ir
        .issues
        .iter()
        .any(|issue| issue.code == ebook_ir::IssueCode::SizeFromContent));
}

#[test]
fn ruby_draws_rt_above_the_base_without_widening_the_line() {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = push(&mut doc, NodeKind::Element(Tag::Body), block_style(), None);
    doc.root = root;
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(), None);
    let start = doc.text.len() as u32;
    doc.text.push_str("漢字かんじ");
    let base = Span::new(start, "漢字".len() as u32);
    let rt = Span::new(start + "漢字".len() as u32, "かんじ".len() as u32);
    let ruby = push(
        &mut doc,
        NodeKind::Ruby { base, rt },
        inline_style(),
        Some(p),
    );
    set_kids(&mut doc, p, &[ruby]);
    set_kids(&mut doc, root, &[p]);

    let ir = run(&doc);
    assert_eq!(ir.runs.lines.len(), 1);
    // 基文两个字 = 20；注音不参与断行宽度。
    assert!((ir.runs.lines[0].rect.w - 20.0).abs() < 0.01);

    let runs: Vec<_> = ir
        .display
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::GlyphRun(run) => Some(run),
            _ => None,
        })
        .collect();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].text, "漢字");
    assert_eq!(runs[1].text, "かんじ");
    assert!((runs[1].size - 5.0).abs() < 0.01);

    let base_glyphs = ir.display.glyph_slice(runs[0].glyphs);
    let rt_glyphs = ir.display.glyph_slice(runs[1].glyphs);
    let base_width: f32 = base_glyphs.iter().map(|g| g.dx).sum();
    let rt_width: f32 = rt_glyphs.iter().map(|g| g.dx).sum();
    assert!((base_width - rt_width).abs() < 0.01);
    assert!(rt_glyphs.iter().all(|g| (g.dy + 5.0).abs() < 0.01));
    // 注音与基文起点相同。
    assert!((runs[0].origin.x - runs[1].origin.x).abs() < 0.01);
}
