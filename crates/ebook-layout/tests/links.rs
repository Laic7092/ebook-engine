//! 链接发射：排版层把带 `link` 的行盒翻译成 `Op::Link`，一行一个、按行顺序。
//!
//! 这里手工构造 `StyledDoc`，因为格式层填 `Node.link` 是另一条线的事；排版层只认数据。

use ebook_ir::doc::FontSpec;
use ebook_ir::{
    Anchor, ComputedStyle, Dir, FontId, LayoutResult, LineHeight, Node, NodeId, NodeKind, Op,
    Rgba8, Span, StyledDoc, Tag,
};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, TextMeasurer};

const HREF: &str = "https://example.com/x";

struct Fixed;

impl TextMeasurer for Fixed {
    fn advance(&self, ch: char, _font: FontId, _size: f32) -> f32 {
        if ch == ' ' {
            4.0
        } else {
            5.0
        }
    }

    fn ascent(&self, _font: FontId, _size: f32) -> f32 {
        10.0
    }

    fn descent(&self, _font: FontId, _size: f32) -> f32 {
        2.0
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
        line_height: LineHeight::Px(20.0),
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

fn text(doc: &mut StyledDoc, body: &str) -> NodeId {
    let start = doc.text.len() as u32;
    doc.text.push_str(body);
    push(
        doc,
        NodeKind::Text(Span::new(start, body.len() as u32)),
        inline_style(),
        None,
    )
}

/// body > p1 > a(link) > span > text1，另有一个 body > p2 > text2（无链接）。
fn doc_with_link() -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = push(&mut doc, NodeKind::Element(Tag::Body), block_style(), None);
    doc.root = root;

    let p1 = push(&mut doc, NodeKind::Element(Tag::P), block_style(), None);
    let a = push(&mut doc, NodeKind::Element(Tag::A), inline_style(), None);
    doc.node_mut(a).link = Some(String::from(HREF));
    let span = push(&mut doc, NodeKind::Element(Tag::Span), inline_style(), None);
    let t1 = text(&mut doc, "aaaa aaaa aaaa aaaa");
    set_kids(&mut doc, span, &[t1]);
    set_kids(&mut doc, a, &[span]);
    set_kids(&mut doc, p1, &[a]);

    let p2 = push(&mut doc, NodeKind::Element(Tag::P), block_style(), None);
    let t2 = text(&mut doc, "bb");
    set_kids(&mut doc, p2, &[t2]);

    set_kids(&mut doc, root, &[p1, p2]);
    doc
}

fn run(doc: &StyledDoc, width: f32) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &NoImageSizes,
    };
    layout(doc, &env, &PageConstraint::new(width, 800.0))
}

fn links(ir: &LayoutResult) -> Vec<(ebook_ir::RectF, String)> {
    ir.display
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Link { rect, href } => Some((*rect, href.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn two_link_lines_emit_two_ops_and_the_plain_line_emits_none() {
    let doc = doc_with_link();
    let ir = run(&doc, 50.0);

    assert_eq!(ir.runs.lines.len(), 3, "link 段落两行 + 无链接段落一行");
    let links = links(&ir);
    assert_eq!(links.len(), 2, "一行一个 Op::Link，不合并");
    assert!(links.iter().all(|(_, href)| href == HREF));

    // 热区就是行盒矩形，且按行顺序出现。
    assert_eq!(links[0].0, ir.runs.lines[0].rect);
    assert_eq!(links[1].0, ir.runs.lines[1].rect);
    // 无链接那一行没有热区。
    assert!(!links.iter().any(|(rect, _)| *rect == ir.runs.lines[2].rect));
}

#[test]
fn line_boxes_carry_node_and_glyph_span() {
    let doc = doc_with_link();
    let ir = run(&doc, 50.0);

    for line in &ir.runs.lines {
        let glyphs = ir.display.glyph_slice(line.glyphs);
        assert!(!glyphs.is_empty(), "有文字的行必须有字形");
        assert!(line.glyphs.end() <= ir.display.glyphs.len() as u32);
        for glyph in glyphs {
            assert!(
                glyph.ch < line.src.len,
                "字形字节偏移 {} 超出本行 src 长度 {}",
                glyph.ch,
                line.src.len
            );
        }
    }

    // 前两行的 node 是链接里的文本节点，第三行属于另一个段落。
    assert_eq!(ir.runs.lines[0].node, ir.runs.lines[1].node);
    assert_ne!(ir.runs.lines[1].node, ir.runs.lines[2].node);
}

#[test]
fn text_without_any_link_emits_no_link_op() {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = push(&mut doc, NodeKind::Element(Tag::Body), block_style(), None);
    doc.root = root;
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(), None);
    let t = text(&mut doc, "plain");
    set_kids(&mut doc, p, &[t]);
    set_kids(&mut doc, root, &[p]);

    let ir = run(&doc, 50.0);
    assert!(!ir.runs.lines.is_empty());
    assert!(links(&ir).is_empty());
}
