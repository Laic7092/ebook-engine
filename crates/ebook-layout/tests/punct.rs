//! 中文排版政策：成对标点挤压、行末悬挂、中西文自动间距。
//!
//! 三个开关默认关闭；这里逐个打开验证可观测结果，关闭时的输出由 golden 守住。

use ebook_ir::doc::FontSpec;
use ebook_ir::{
    Anchor, ComputedStyle, Dir, Display, FontId, LayoutResult, Len, LineHeight, Node, NodeId,
    NodeKind, Op, Rgba8, Span, StyledDoc, Tag, TextRules, TextStyle,
};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, TextMeasurer};

struct Fixed;

impl TextMeasurer for Fixed {
    fn advance(&self, ch: char, _font: FontId, _size: f32) -> f32 {
        if ch == ' ' {
            4.0
        } else if ch.is_ascii_alphanumeric() {
            5.0
        } else {
            10.0
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
    let mut style = ComputedStyle::default();
    style.display = Display::Block;
    style.font = FontSpec {
        font: FontId(0),
        size: 10.0,
        weight: 400,
        ..FontSpec::default()
    };
    style.line_height = LineHeight::Px(20.0);
    style.margin.top = Len::ZERO;
    style.margin.bottom = Len::ZERO;
    style
}

fn inline_style(rules: TextRules) -> ComputedStyle {
    let mut style = ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 10.0,
            weight: 400,
            ..FontSpec::default()
        },
        Rgba8::BLACK,
        Dir::Ltr,
    );
    style.text = TextStyle {
        rules,
        ..TextStyle::default()
    };
    style
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

fn doc(body: &str, rules: TextRules) -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = push(&mut doc, NodeKind::Element(Tag::Body), block_style(), None);
    doc.root = root;
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(), None);
    let start = doc.text.len() as u32;
    doc.text.push_str(body);
    let text = push(
        &mut doc,
        NodeKind::Text(Span::new(start, body.len() as u32)),
        inline_style(rules),
        None,
    );
    set_kids(&mut doc, p, &[text]);
    set_kids(&mut doc, root, &[p]);
    doc
}

fn run(doc: &StyledDoc, width: f32) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &NoImageSizes,
    };
    layout(doc, &env, &PageConstraint::new(width, 800.0))
}

fn glyph_dx(ir: &LayoutResult, ch: u32) -> f32 {
    let mut found = None;
    for op in &ir.display.ops {
        if let Op::GlyphRun(run) = op {
            for glyph in ir.display.glyph_slice(run.glyphs) {
                if glyph.ch == ch {
                    found = Some(glyph.dx);
                }
            }
        }
    }
    found.unwrap_or_else(|| panic!("no glyph at {ch}"))
}

#[test]
fn trimming_halves_the_quote_pairs() {
    // 他说：“走。”——`：`+`“` 压 `“` 的左半宽，`。`+`”` 压 `”` 的右半宽。
    let body = "他说：“走。”";
    let plain = run(&doc(body, TextRules::default()), 400.0);
    let mut rules = TextRules::default();
    rules.spacing_trim = true;
    let trimmed = run(&doc(body, rules), 400.0);

    assert_eq!(plain.runs.lines.len(), 1);
    assert!((plain.runs.lines[0].rect.w - 70.0).abs() < 0.01);
    // 只压拥有空白半边、且在可压集合里的那一个：`：“` 压 `“` 的左半宽。
    // `。”` 里的 `。` 不是可压的闭引号（`~/layx/render/punct.go` 的
    // trimmableClosing 只含 U+2019/201D/3009），所以 `”` 保持全宽。
    assert!((trimmed.runs.lines[0].rect.w - 65.0).abs() < 0.01);
    // 字节偏移：他 0、说 3、：6、“9、走 12、。15、”18。
    assert!((glyph_dx(&trimmed, 9) - 5.0).abs() < 0.01, "opening quote");
    assert!(
        (glyph_dx(&trimmed, 18) - 10.0).abs() < 0.01,
        "closing quote"
    );
    // 不在表里的标点不压。
    assert!((glyph_dx(&trimmed, 6) - 10.0).abs() < 0.01);
    assert!((glyph_dx(&trimmed, 15) - 10.0).abs() < 0.01);
}

#[test]
fn adjacent_brackets_do_not_trim_without_a_pair() {
    // 「你好」没有相邻标点对：挤压开关打开也不改宽度（规则表只压引号类）。
    let body = "「你好」";
    let plain = run(&doc(body, TextRules::default()), 400.0);
    let mut rules = TextRules::default();
    rules.spacing_trim = true;
    let trimmed = run(&doc(body, rules), 400.0);
    assert!((plain.runs.lines[0].rect.w - trimmed.runs.lines[0].rect.w).abs() < 0.01);
}

#[test]
fn hanging_keeps_the_closing_mark_on_the_line() {
    let body = "「你好」";
    // 40 宽只差一个标点：100 宽放得下，35 宽需要悬挂半宽才放得下。
    let plain = run(&doc(body, TextRules::default()), 35.0);
    assert_eq!(plain.runs.lines.len(), 2, "不开悬挂时标点被挤到下一行");

    let mut rules = TextRules::default();
    rules.hanging_punctuation = true;
    let hanging = run(&doc(body, rules), 35.0);
    assert_eq!(hanging.runs.lines.len(), 1);
    // 行盒右边界跟着探出，字形坐标不变。
    assert!((hanging.runs.lines[0].rect.w - 40.0).abs() < 0.01);
    assert!((glyph_dx(&hanging, 9) - 10.0).abs() < 0.01);
}

#[test]
fn autospacing_inserts_a_quarter_em_between_cjk_and_latin() {
    let body = "中文English混排";
    let plain = run(&doc(body, TextRules::default()), 400.0);
    // 中(10) 文(10) E n g l i s h(7×5) 混(10) 排(10)
    assert!((plain.runs.lines[0].rect.w - 75.0).abs() < 0.01);

    let mut rules = TextRules::default();
    rules.autospacing = true;
    let spaced = run(&doc(body, rules), 400.0);
    // 两处 1/4 em = 2 × 2.5。
    assert!((spaced.runs.lines[0].rect.w - 80.0).abs() < 0.01);
}

#[test]
fn autospacing_is_not_left_at_the_line_end() {
    let mut rules = TextRules::default();
    rules.autospacing = true;
    let spaced = run(&doc("中a", rules), 10.0);
    assert_eq!(spaced.runs.lines.len(), 2);
    // 行末的空隙收回：中文行仍是 10，不是 12.5。
    assert!((spaced.runs.lines[0].rect.w - 10.0).abs() < 0.01);
    assert!((spaced.runs.lines[1].rect.w - 5.0).abs() < 0.01);
}

#[test]
fn rules_are_opt_in() {
    assert_eq!(
        TextRules::default(),
        TextRules {
            spacing_trim: false,
            hanging_punctuation: false,
            autospacing: false,
        }
    );
}
