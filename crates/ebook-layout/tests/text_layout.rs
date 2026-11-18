use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{FontId, LayoutResult, Op, StyledDoc, UnresolvedReason};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, TextMeasurer};

struct Fixed;

impl TextMeasurer for Fixed {
    fn advance(&self, ch: char, _font: FontId, _size: f32) -> f32 {
        match ch {
            ' ' => 4.0,
            'a'..='z' => 5.0,
            _ => 10.0,
        }
    }

    fn ascent(&self, _font: FontId, _size: f32) -> f32 {
        10.0
    }

    fn descent(&self, _font: FontId, _size: f32) -> f32 {
        2.0
    }

    fn covers(&self, ch: char, _font: FontId) -> bool {
        ch != 'X'
    }

    fn glyph_id(&self, ch: char, _font: FontId) -> u16 {
        500 + (ch as u32 % 200) as u16
    }
}

fn run(width: f32, css: &str, body: &str) -> LayoutResult {
    let html = format!(
        "<html><head><style>body{{margin:0}}p{{margin:0;padding:0;border:0;\
         line-height:20px}}{css}</style></head><body>{body}</body></html>"
    );
    let doc: StyledDoc = build(&html, 0, &mut NoAssets).expect("xhtml");
    build_layout(&doc, width)
}

fn build_layout(doc: &StyledDoc, width: f32) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &NoImageSizes,
    };
    layout(doc, &env, &PageConstraint::new(width, 800.0))
}

#[test]
fn justify_stretches_spaces_but_not_the_last_line() {
    let result = run(50.0, "p{text-align:justify}", "<p>aa aa aa aa</p>");
    assert_eq!(result.runs.lines.len(), 2);
    assert!((result.runs.lines[0].rect.w - 50.0).abs() < 0.01);
    assert!((result.runs.lines[1].rect.w - 10.0).abs() < 0.01);
}

#[test]
fn justify_without_spaces_stretches_all_but_the_last_glyph() {
    let result = run(
        95.0,
        "p{text-align:justify}",
        "<p>中中中中中中中中中中中中中中中</p>",
    );
    assert_eq!(result.runs.lines.len(), 2);
    assert!((result.runs.lines[0].rect.w - 95.0).abs() < 0.01);
    assert!((result.runs.lines[1].rect.w - 60.0).abs() < 0.01);

    let first = result
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::GlyphRun(run) => Some(run),
            _ => None,
        })
        .expect("first run");
    let glyphs = result.display.glyph_slice(first.glyphs);
    assert!((glyphs[0].dx - 10.625).abs() < 0.01);
    assert!((glyphs[8].dx - 10.0).abs() < 0.01);
}

#[test]
fn nowrap_keeps_everything_on_one_line() {
    let result = run(55.0, "p{white-space:nowrap}", "<p>aa aa aa aa aa aa</p>");
    assert_eq!(result.runs.lines.len(), 1);
    assert!(result.runs.lines[0].rect.w > 55.0);
}

#[test]
fn text_transform_is_applied_before_measurement() {
    let result = run(200.0, "p{text-transform:uppercase}", "<p>abc</p>");
    assert_eq!(result.runs.lines.len(), 1);
    assert!((result.runs.lines[0].rect.w - 30.0).abs() < 0.01);
    let text = result
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::GlyphRun(run) => Some(run.text.clone()),
            _ => None,
        })
        .expect("glyph run");
    assert_eq!(text, "ABC");

    let lowered = run(200.0, "p{text-transform:lowercase}", "<p>ABC</p>");
    assert!((lowered.runs.lines[0].rect.w - 15.0).abs() < 0.01);
}

#[test]
fn missing_glyphs_are_unresolved_and_dirty() {
    let result = run(200.0, "", "<p>abXcd</p>");
    assert!(result.display.ops.iter().any(|op| matches!(
        op,
        Op::Unresolved(u) if u.reason == UnresolvedReason::FontMissing
    )));
    assert_eq!(result.dirty.len(), 1);
    assert_eq!(result.dirty[0].reason, UnresolvedReason::FontMissing);
    assert!(!result.display.ops.iter().any(|op| match op {
        Op::GlyphRun(run) => run.text.contains('X'),
        _ => false,
    }));

    let glyph = result
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::GlyphRun(run) => result.display.glyph_slice(run.glyphs).first().copied(),
            _ => None,
        })
        .expect("glyph");
    assert_ne!(glyph.gid, 'a' as u16);
}
