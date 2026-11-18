//! 断字接入：`hyphens:auto` + 注入的英文词典才会出现行末连字符。

use ebook_ir::doc::FontSpec;
use ebook_ir::{
    Anchor, ComputedStyle, Dir, FontId, LayoutResult, LineHeight, Node, NodeId, NodeKind, Op,
    Rgba8, Span, StyledDoc, Tag, TextStyle,
};
use ebook_layout::{
    layout, English, Hyphenator, LayoutEnv, NoImageSizes, PageConstraint, TextMeasurer,
};

struct Measurer {
    english: English,
}

impl Measurer {
    fn new() -> Self {
        Self {
            english: English::new(),
        }
    }
}

impl TextMeasurer for Measurer {
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

    fn hyphenator(&self) -> Option<&dyn Hyphenator> {
        Some(&self.english)
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

fn inline_style(hyphens: bool) -> ComputedStyle {
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
        hyphens,
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

fn doc(body: &str, hyphens: bool) -> StyledDoc {
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
        inline_style(hyphens),
        None,
    );
    set_kids(&mut doc, p, &[text]);
    set_kids(&mut doc, root, &[p]);
    doc
}

fn run(doc: &StyledDoc, with_dictionary: bool) -> LayoutResult {
    if with_dictionary {
        let measurer = Measurer::new();
        let env = LayoutEnv {
            measurer: &measurer,
            resources: &NoImageSizes,
        };
        layout(doc, &env, &PageConstraint::new(35.0, 800.0))
    } else {
        let env = LayoutEnv {
            measurer: &NoDictionary,
            resources: &NoImageSizes,
        };
        layout(doc, &env, &PageConstraint::new(35.0, 800.0))
    }
}

/// 没有词典的度量：`hyphens:auto` 也不生效。
struct NoDictionary;

impl TextMeasurer for NoDictionary {
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

fn texts(ir: &LayoutResult) -> Vec<String> {
    ir.display
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::GlyphRun(run) => Some(run.text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn dictionary_breaks_the_word_and_the_line_ends_with_a_hyphen() {
    // 35 宽 = 7 个字符：hyphen 是 6 个字符 + 连字符，正好落在 hy-phen 的断点上。
    let ir = run(&doc("hyphenation", true), true);
    assert_eq!(ir.runs.lines.len(), 2);
    assert_eq!(ir.runs.lines[0].src.len, 6);
    assert_eq!(ir.runs.lines[1].src.start, 6);
    assert_eq!(texts(&ir), vec!["hyphen", "-", "ation"]);

    // 连字符的 ch 停在行末最后一个字符上，不会越出 src。
    let hyphen = ir
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::GlyphRun(run) if run.text == "-" => {
                let glyph = ir.display.glyph_slice(run.glyphs)[0];
                Some(glyph)
            }
            _ => None,
        })
        .expect("hyphen glyph");
    assert_eq!(hyphen.ch, 5);
    assert!(hyphen.ch < ir.runs.lines[0].src.len);
    // 行宽算上行末连字符。
    assert!((ir.runs.lines[0].rect.w - 35.0).abs() < 0.01);
}

#[test]
fn without_a_dictionary_the_word_is_not_broken() {
    let ir = run(&doc("hyphenation", true), false);
    assert!(!texts(&ir).iter().any(|text| text == "-"));
    // 没有断字就退回 UAX #14 的断点（词内没有断点，只能整体溢出）。
    assert_eq!(ir.runs.lines.len(), 2);
    assert!(!texts(&ir)[0].contains('-'));
}

#[test]
fn hyphens_auto_is_required() {
    let ir = run(&doc("hyphenation", false), true);
    assert!(!texts(&ir).iter().any(|text| text == "-"));
}

#[test]
fn short_words_have_no_break_points() {
    let mut out = Vec::new();
    English::new().breaks("four", &mut out);
    assert!(out.is_empty());
    let mut long = Vec::new();
    English::new().breaks("hyphenation", &mut long);
    assert_eq!(long, vec![2, 6]);
    assert_eq!(English::new().name(), "en-us");
}

#[test]
fn hyphenation_keeps_clusters_intact() {
    // 断点只落在簇首字符上：合字/组合字符不会被从中间切开。
    let ir = run(&doc("office", true), true);
    let runs = texts(&ir);
    assert!(runs.iter().all(|text| text != "-"));
}
