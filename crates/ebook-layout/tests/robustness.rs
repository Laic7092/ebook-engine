//! 结构规模与预算的硬化用例。

use std::sync::atomic::{AtomicUsize, Ordering};

use ebook_ir::{
    ComputedStyle, Dir, FontId, FontSpec, Len, Limits, Node, NodeKind, Op, Rgba8, Span, StyledDoc,
    Tag, TextAlign, TextTransform,
};
use ebook_layout::{
    layout, Layout, LayoutEnv, LayoutOptions, NoImageSizes, PageConstraint, StubMeasurer,
    TextMeasurer,
};

fn node(kind: NodeKind, style: ComputedStyle, parent: Option<u32>) -> Node {
    Node {
        kind,
        style,
        parent,
        first_child: 0,
        child_len: 0,
        src: None,
        link: None,
    }
}

/// 一个块里的内联片段超过 `u16::MAX` 时，最后一片的样式仍然是它自己的。
///
/// `Ch.piece` 曾经是 `u16`：第 65537 个片段会回绕到 0，于是“最后一段”被
/// 当成第一段的样式画出来（颜色/字重/链接全错），而且不报任何错。
#[test]
fn more_than_u16_max_inline_pieces_keep_their_own_style() {
    const PIECES: usize = u16::MAX as usize + 2;
    const INK: Rgba8 = Rgba8 {
        r: 255,
        g: 0,
        b: 0,
        a: 255,
    };

    let base = ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 16.0,
            weight: 400,
            ..Default::default()
        },
        Rgba8::BLACK,
        Dir::Ltr,
    );
    let last_style = ComputedStyle { color: INK, ..base };

    let mut doc = StyledDoc::default();
    let root = doc.push_node(node(
        NodeKind::Element(Tag::P),
        ComputedStyle::default(),
        None,
    ));
    doc.root = root;

    let mut kids = Vec::with_capacity(PIECES);
    for index in 0..PIECES {
        let start = doc.text.len() as u32;
        // 最后一片用 'z' 区分；其余用 'a'，不夹空格，避免空白折叠跨片段。
        doc.text.push(if index + 1 == PIECES { 'z' } else { 'a' });
        let style = if index + 1 == PIECES {
            last_style
        } else {
            base
        };
        kids.push(doc.push_node(node(NodeKind::Text(Span::new(start, 1)), style, Some(root))));
    }
    doc.set_kids(root, &kids);

    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let ir = layout(&doc, &env, &PageConstraint::new(2000.0, 2000.0));

    let z = ir
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::GlyphRun(run) if run.text.contains('z') => Some(run),
            _ => None,
        })
        .expect("最后一片必须产出字形 run");
    assert_eq!(
        z.color, INK,
        "第 {PIECES} 个内联片段拿到了别的片段的样式（piece 下标被截断）"
    );
}

fn doc_paragraphs(paragraphs: usize, words: usize) -> StyledDoc {
    let mut doc = StyledDoc::default();
    let root = doc.push_node(node(
        NodeKind::Element(Tag::Root),
        ComputedStyle::default(),
        None,
    ));
    doc.root = root;
    let style = ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 16.0,
            weight: 400,
            ..Default::default()
        },
        Rgba8::BLACK,
        Dir::Ltr,
    );
    let mut blocks = Vec::with_capacity(paragraphs);
    for _ in 0..paragraphs {
        let block = doc.push_node(node(
            NodeKind::Element(Tag::P),
            ComputedStyle::default(),
            Some(root),
        ));
        let start = doc.text.len() as u32;
        for index in 0..words {
            if index > 0 {
                doc.text.push(' ');
            }
            doc.text.push_str("winter");
        }
        let span = Span::new(start, doc.text.len() as u32 - start);
        let text = doc.push_node(node(NodeKind::Text(span), style, Some(block)));
        doc.set_kids(block, &[text]);
        blocks.push(block);
    }
    doc.set_kids(root, &blocks);
    doc
}

#[derive(Default)]
struct Counting(AtomicUsize);

impl TextMeasurer for Counting {
    fn advance(&self, _ch: char, _font: FontId, _size: f32) -> f32 {
        self.0.fetch_add(1, Ordering::Relaxed);
        8.0
    }

    fn ascent(&self, _font: FontId, _size: f32) -> f32 {
        10.0
    }

    fn descent(&self, _font: FontId, _size: f32) -> f32 {
        2.0
    }
}

/// `max_lines` 必须在断行阶段生效：不能把整章都量完/断完再在组装侧丢弃。
#[test]
fn max_lines_stops_flow_before_shaping_the_whole_document() {
    let doc = doc_paragraphs(200, 12);
    let c = PageConstraint::new(120.0, 80.0);

    let full_counter = Counting::default();
    let full_env = LayoutEnv {
        measurer: &full_counter,
        resources: &NoImageSizes,
    };
    let full = Layout::new().layout(&doc, &full_env, &c);
    let full_calls = full_counter.0.load(Ordering::Relaxed);
    assert!(full.runs.lines.len() > 20, "语料太瘦");
    assert!(!full.truncated);

    let cut = 4u32;
    let counter = Counting::default();
    let env = LayoutEnv {
        measurer: &counter,
        resources: &NoImageSizes,
    };
    let options = LayoutOptions {
        limits: Limits {
            max_lines: cut,
            ..Default::default()
        },
        ..Default::default()
    };
    let ir = Layout::new().layout_with(&doc, &env, &c, &options);
    assert_eq!(ir.runs.lines.len(), cut as usize);
    assert!(ir.truncated);
    let calls = counter.0.load(Ordering::Relaxed);
    assert!(
        calls * 4 < full_calls,
        "max_lines 没有提前收手：limit 量了 {calls} 次，完整章 {full_calls} 次"
    );
    assert_eq!(
        ir.issues
            .iter()
            .filter(|issue| issue.detail.as_deref() == Some("max_lines"))
            .count(),
        1,
        "max_lines 诊断只能记一条"
    );
}

fn rtl_doc(align: TextAlign) -> StyledDoc {
    let mut doc = StyledDoc::default();
    let mut block = ComputedStyle {
        dir: Dir::Rtl,
        ..ComputedStyle::default()
    };
    block.text.align = align;
    let root = doc.push_node(node(NodeKind::Element(Tag::P), block, None));
    doc.root = root;
    let start = doc.text.len() as u32;
    doc.text.push_str("中中中中");
    let span = Span::new(start, doc.text.len() as u32 - start);
    let style = ComputedStyle::inline(FontSpec::default(), Rgba8::BLACK, Dir::Rtl);
    let text = doc.push_node(node(NodeKind::Text(span), style, Some(root)));
    doc.set_kids(root, &[text]);
    doc
}

/// `text-align: start` / `end` 是逻辑值：RTL 段落里 start 在右边。
#[test]
fn rtl_start_and_end_are_mirrored() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    // 4 个全角字 × 16px = 64px；页宽 200，右对齐时左边留 136。
    let c = PageConstraint::new(200.0, 200.0);

    let start = layout(&rtl_doc(TextAlign::Start), &env, &c);
    assert_eq!(start.runs.lines[0].rect.x, 136.0);
    let end = layout(&rtl_doc(TextAlign::End), &env, &c);
    assert_eq!(end.runs.lines[0].rect.x, 0.0);
}

fn indented_doc(dir: Dir, align: TextAlign, indent: f32) -> StyledDoc {
    let mut doc = StyledDoc::default();
    let mut block = ComputedStyle {
        dir,
        ..ComputedStyle::default()
    };
    block.text.align = align;
    block.text.indent = Len::Px(indent);
    let root = doc.push_node(node(NodeKind::Element(Tag::P), block, None));
    doc.root = root;
    let start = doc.text.len() as u32;
    doc.text.push_str("中中中中");
    let span = Span::new(start, doc.text.len() as u32 - start);
    let style = ComputedStyle::inline(FontSpec::default(), Rgba8::BLACK, dir);
    let text = doc.push_node(node(NodeKind::Text(span), style, Some(root)));
    doc.set_kids(root, &[text]);
    doc
}

/// `text-indent` 从行首一侧扣：LTR 扣左、RTL 扣右，不能把首行推出页外。
#[test]
fn text_indent_follows_the_paragraph_direction() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let c = PageConstraint::new(200.0, 200.0);

    let ltr = layout(&indented_doc(Dir::Ltr, TextAlign::Start, 10.0), &env, &c);
    assert_eq!(ltr.runs.lines[0].rect.x, 10.0);
    assert_eq!(ltr.runs.lines[0].rect.w, 64.0);

    let rtl = layout(&indented_doc(Dir::Rtl, TextAlign::Start, 10.0), &env, &c);
    assert_eq!(rtl.runs.lines[0].rect.x, 126.0, "200 - 10 - 64");
    assert_eq!(rtl.runs.lines[0].rect.w, 64.0);

    // `end` 也不能被缩进推出去：LTR 的 end 仍然贴右边。
    let end = layout(&indented_doc(Dir::Ltr, TextAlign::End, 10.0), &env, &c);
    assert_eq!(end.runs.lines[0].rect.x, 136.0);
}

/// `text-transform` 会改变字符个数（`ß` → `SS`），`LineBox.src` 仍必须
/// 覆盖原文的完整字节，否则选区末尾会少一个字符。
#[test]
fn line_source_covers_the_original_character_not_the_transformed_one() {
    let mut doc = StyledDoc::default();
    let root = doc.push_node(node(
        NodeKind::Element(Tag::P),
        ComputedStyle::default(),
        None,
    ));
    doc.root = root;
    let start = doc.text.len() as u32;
    doc.text.push_str("aß");
    let span = Span::new(start, doc.text.len() as u32 - start);
    let mut style = ComputedStyle::inline(FontSpec::default(), Rgba8::BLACK, Dir::Ltr);
    style.text.transform = TextTransform::Uppercase;
    let text = doc.push_node(node(NodeKind::Text(span), style, Some(root)));
    doc.set_kids(root, &[text]);

    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let ir = layout(&doc, &env, &PageConstraint::new(400.0, 200.0));
    let line = &ir.runs.lines[0];
    assert_eq!(line.src.len, 3, "src 必须覆盖 1 + 2 字节");
    assert_eq!(doc.text_of(line.src), "aß");
}
