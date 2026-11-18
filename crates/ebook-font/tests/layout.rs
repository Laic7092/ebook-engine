//! 整形接入排版层：合字/字距先整形后加字距，簇首字符映射回原文字节偏移。

use std::fs;
use std::path::PathBuf;

use ebook_font::FontBook;
use ebook_ir::doc::{FontSpec, TextStyle};
use ebook_ir::{
    Anchor, ComputedStyle, Dir, FontId, LayoutResult, LineHeight, Node, NodeId, NodeKind, Op,
    Rgba8, Span, StyledDoc, Tag,
};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, TextMeasurer};

fn book() -> FontBook {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/font/test.ttf");
    let bytes = fs::read(path).expect("test font");
    let mut book = FontBook::new();
    book.add(bytes).expect("font");
    book
}

fn block_style(size: f32) -> ComputedStyle {
    ComputedStyle {
        font: FontSpec {
            font: FontId(0),
            size,
            weight: 400,
            ..FontSpec::default()
        },
        line_height: LineHeight::Px(size * 1.5),
        ..ComputedStyle::default()
    }
}

fn inline_style(size: f32) -> ComputedStyle {
    ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size,
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

fn doc(body: &str, size: f32, text_style: TextStyle) -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = push(
        &mut doc,
        NodeKind::Element(Tag::Body),
        block_style(size),
        None,
    );
    doc.root = root;
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(size), None);
    let start = doc.text.len() as u32;
    doc.text.push_str(body);
    let text = push(
        &mut doc,
        NodeKind::Text(Span::new(start, body.len() as u32)),
        inline_style(size),
        None,
    );
    doc.node_mut(text).style.text = text_style;
    set_kids(&mut doc, p, &[text]);
    set_kids(&mut doc, root, &[p]);
    doc
}

fn run(doc: &StyledDoc) -> LayoutResult {
    let fonts = book();
    let env = LayoutEnv {
        measurer: &fonts,
        resources: &NoImageSizes,
    };
    layout(doc, &env, &PageConstraint::new(1000.0, 800.0))
}

fn glyphs(ir: &LayoutResult) -> Vec<(u16, u32, f32)> {
    let mut out = Vec::new();
    for op in &ir.display.ops {
        if let Op::GlyphRun(run) = op {
            for glyph in ir.display.glyph_slice(run.glyphs) {
                out.push((glyph.gid, glyph.ch, glyph.dx));
            }
        }
    }
    out
}

#[test]
fn ligature_survives_into_the_ir() {
    let ir = run(&doc("office", 32.0, TextStyle::default()));
    let glyphs = glyphs(&ir);
    // o | ffi | c | e
    assert_eq!(glyphs.len(), 4, "{glyphs:?}");
    assert_eq!(glyphs[0].1, 0);
    assert_eq!(glyphs[1].1, 1, "合字的簇首要落回原文的 f");
    assert_eq!(glyphs[2].1, 4);
    assert_eq!(glyphs[3].1, 5);
    // 合字的字形数少于字符数，但行宽仍然是整形后的宽度。
    assert_eq!(ir.runs.lines.len(), 1);
    let width: f32 = glyphs.iter().map(|glyph| glyph.2).sum();
    assert!((ir.runs.lines[0].rect.w - width).abs() < 0.01);
}

#[test]
fn kerning_is_in_the_line_width() {
    let fonts = book();
    let mut shaped = Vec::new();
    let kerned = fonts.shape("AV", FontId(0), 32.0, &mut shaped);
    let naive = fonts.advance('A', FontId(0), 32.0) + fonts.advance('V', FontId(0), 32.0);
    assert!(kerned < naive - 0.01);

    let ir = run(&doc("AV", 32.0, TextStyle::default()));
    assert!((ir.runs.lines[0].rect.w - kerned).abs() < 0.01);
}

#[test]
fn letter_spacing_is_added_after_shaping() {
    let fonts = book();
    let mut shaped = Vec::new();
    let base = fonts.shape("fi", FontId(0), 32.0, &mut shaped);
    assert_eq!(shaped.len(), 1);

    let style = TextStyle {
        letter_spacing: 2.0,
        ..TextStyle::default()
    };
    let ir = run(&doc("fi", 32.0, style));
    // 两个字距加在唯一的合字簇上。
    assert!(
        (ir.runs.lines[0].rect.w - (base + 4.0)).abs() < 0.01,
        "{}",
        ir.runs.lines[0].rect.w
    );
    let glyphs = glyphs(&ir);
    assert_eq!(glyphs.len(), 1);
    assert!((glyphs[0].2 - (base + 4.0)).abs() < 0.01);
}

#[test]
fn missing_glyphs_keep_their_advance_slot() {
    // test.ttf 不覆盖 CJK：缺字仍然占位，但行里没有对应字形。
    let ir = run(&doc("a中b", 32.0, TextStyle::default()));
    let glyphs = glyphs(&ir);
    assert_eq!(glyphs.len(), 2, "{glyphs:?}");
    assert!(ir
        .display
        .ops
        .iter()
        .any(|op| matches!(op, Op::Unresolved(_))));
    let fonts = book();
    let expected = fonts.advance('a', FontId(0), 32.0)
        + fonts.advance('中', FontId(0), 32.0)
        + fonts.advance('b', FontId(0), 32.0);
    assert!((ir.runs.lines[0].rect.w - expected).abs() < 0.01);
}
