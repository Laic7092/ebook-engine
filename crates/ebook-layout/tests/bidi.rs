//! 双向文本：段内 UAX #9 重排、run 方向与行盒方向。

use ebook_ir::doc::FontSpec;
use ebook_ir::{
    Anchor, ComputedStyle, Dir, Display, FontId, LayoutResult, Len, LineHeight, Node, NodeId,
    NodeKind, Op, Rgba8, Span, StyledDoc, Tag,
};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, TextMeasurer};

struct Fixed;

impl TextMeasurer for Fixed {
    fn advance(&self, ch: char, _font: FontId, _size: f32) -> f32 {
        if ch == ' ' {
            4.0
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

fn block_style(dir: Dir) -> ComputedStyle {
    let mut style = ComputedStyle::default();
    style.display = Display::Block;
    style.dir = dir;
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

fn inline_style(dir: Dir) -> ComputedStyle {
    ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 10.0,
            weight: 400,
            ..FontSpec::default()
        },
        Rgba8::BLACK,
        dir,
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

fn doc(body: &str, dir: Dir) -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    doc.meta.dir = dir;
    let root = push(
        &mut doc,
        NodeKind::Element(Tag::Body),
        block_style(dir),
        None,
    );
    doc.root = root;
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(dir), None);
    let start = doc.text.len() as u32;
    doc.text.push_str(body);
    let text = push(
        &mut doc,
        NodeKind::Text(Span::new(start, body.len() as u32)),
        inline_style(dir),
        None,
    );
    set_kids(&mut doc, p, &[text]);
    set_kids(&mut doc, root, &[p]);
    doc
}

fn run(doc: &StyledDoc) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &NoImageSizes,
    };
    layout(doc, &env, &PageConstraint::new(400.0, 800.0))
}

/// 视觉顺序的字形：(行内字节偏移, 方向)。
fn visual(ir: &LayoutResult) -> Vec<(u32, Dir, f32)> {
    let mut out = Vec::new();
    for op in &ir.display.ops {
        if let Op::GlyphRun(run) = op {
            for glyph in ir.display.glyph_slice(run.glyphs) {
                out.push((glyph.ch, run.dir, glyph.dx));
            }
        }
    }
    out
}

#[test]
fn mixed_ltr_paragraph_reorders_the_rtl_run() {
    // "abc אבג def"：Hebrew 三字各占 2 字节，视觉顺序是 abc + 反向的 אבג + def。
    let ir = run(&doc("abc אבג def", Dir::Ltr));
    assert_eq!(ir.runs.lines.len(), 1);
    assert_eq!(ir.runs.lines[0].dir, Dir::Ltr);
    let glyphs = visual(&ir);
    let offsets: Vec<u32> = glyphs.iter().map(|(ch, _, _)| *ch).collect();
    assert_eq!(offsets, vec![0, 1, 2, 3, 8, 6, 4, 10, 11, 12, 13]);
    let dirs: Vec<Dir> = glyphs.iter().map(|(_, dir, _)| *dir).collect();
    assert_eq!(
        dirs,
        vec![
            Dir::Ltr,
            Dir::Ltr,
            Dir::Ltr,
            Dir::Ltr,
            Dir::Rtl,
            Dir::Rtl,
            Dir::Rtl,
            Dir::Ltr,
            Dir::Ltr,
            Dir::Ltr,
            Dir::Ltr
        ]
    );
    // 前进宽度按视觉顺序累加；每段 run 的起点就是它左边所有字形的宽度和。
    let expected: [f32; 11] = [
        0.0, 10.0, 20.0, 30.0, 34.0, 44.0, 54.0, 64.0, 68.0, 78.0, 88.0,
    ];
    let mut pen = 0.0f32;
    for (glyph, want) in glyphs.iter().zip(expected) {
        assert!((pen - want).abs() < 0.01, "pen {pen} want {want}");
        pen += glyph.2;
    }
}

#[test]
fn rtl_paragraph_puts_latin_on_the_left() {
    // 段方向 RTL：逻辑开头的 Hebrew 在右，拉丁在左。
    let ir = run(&doc("אבג abc", Dir::Rtl));
    assert_eq!(ir.runs.lines[0].dir, Dir::Rtl);
    let glyphs = visual(&ir);
    let offsets: Vec<u32> = glyphs.iter().map(|(ch, _, _)| *ch).collect();
    assert_eq!(offsets, vec![7, 8, 9, 6, 4, 2, 0]);
    let dirs: Vec<Dir> = glyphs.iter().map(|(_, dir, _)| *dir).collect();
    // 段尾的空白跟着 RTL 段走（UAX #9：RTL 段落里的段落末尾空白按段级）。
    assert_eq!(
        dirs,
        vec![
            Dir::Ltr,
            Dir::Ltr,
            Dir::Ltr,
            Dir::Rtl,
            Dir::Rtl,
            Dir::Rtl,
            Dir::Rtl
        ]
    );
}

#[test]
fn rtl_trailing_punctuation_stays_at_the_visual_end() {
    // "...אבג." 的句号在 RTL 段里跟着 RTL 走，视觉上落在最左（行末）。
    let ir = run(&doc("אבג.", Dir::Rtl));
    let offsets: Vec<u32> = visual(&ir).iter().map(|(ch, _, _)| *ch).collect();
    assert_eq!(offsets, vec![6, 4, 2, 0]);
    let dirs: Vec<Dir> = visual(&ir).iter().map(|(_, dir, _)| *dir).collect();
    assert!(dirs.iter().all(|dir| *dir == Dir::Rtl));
}

#[test]
fn pure_rtl_line_reverses_entirely() {
    let ir = run(&doc("אבגד", Dir::Rtl));
    let offsets: Vec<u32> = visual(&ir).iter().map(|(ch, _, _)| *ch).collect();
    assert_eq!(offsets, vec![6, 4, 2, 0]);
    // 行盒仍记原文范围，锚点不因重排改变。
    assert_eq!(ir.runs.lines[0].src.start, 0);
    assert_eq!(ir.runs.lines[0].src.len, 8);
}

#[test]
fn bidi_runs_split_at_script_boundaries() {
    let ir = run(&doc("aאבגb", Dir::Ltr));
    let runs: Vec<(Dir, String)> = ir
        .display
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::GlyphRun(run) => Some((run.dir, run.text.clone())),
            _ => None,
        })
        .collect();
    // 视觉顺序是 a | גבא | b；run.text 保持逻辑顺序，方向由 run.dir 表达。
    assert_eq!(
        runs,
        vec![
            (Dir::Ltr, String::from("a")),
            (Dir::Rtl, String::from("אבג")),
            (Dir::Ltr, String::from("b"))
        ]
    );
}
