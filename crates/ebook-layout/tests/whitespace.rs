//! `white-space` 的换行与空白语义：pre / pre-line / pre-wrap / break-spaces。
//!
//! 这里用真实的 CSS 解析构造 `StyledDoc`，所以同时覆盖格式层的取值映射与
//! 排版层的展平逻辑。

use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{FontId, LayoutResult, Op, StyledDoc};
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
}

fn run(width: f32, css: &str, body: &str) -> LayoutResult {
    let html = format!(
        "<html><head><style>body{{margin:0}}p{{margin:0;padding:0;border:0;\
         line-height:20px}}{css}</style></head><body>{body}</body></html>"
    );
    let doc: StyledDoc = build(&html, 0, &mut NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &NoImageSizes,
    };
    layout(&doc, &env, &PageConstraint::new(width, 800.0))
}

fn line_texts(result: &LayoutResult) -> Vec<String> {
    result
        .display
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::GlyphRun(run) => Some(run.text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn pre_keeps_hard_line_breaks() {
    let result = run(200.0, "p{white-space:pre}", "<p>a\nb</p>");
    assert_eq!(result.runs.lines.len(), 2);
    assert_eq!(line_texts(&result), ["a", "b"]);
    assert!((result.runs.lines[0].rect.w - 5.0).abs() < 0.01);
    assert!((result.runs.lines[1].rect.w - 5.0).abs() < 0.01);
}

#[test]
fn pre_preserves_consecutive_spaces() {
    let result = run(200.0, "p{white-space:pre}", "<p>a  b</p>");
    assert_eq!(result.runs.lines.len(), 1);
    // a(5) + 两个空格(4+4) + b(5)，折叠时应当是 13 而不是 18。
    assert!((result.runs.lines[0].rect.w - 18.0).abs() < 0.01);
    assert_eq!(line_texts(&result), ["a  b"]);
}

#[test]
fn pre_expands_tabs_to_eight_column_stops() {
    let result = run(200.0, "p{white-space:pre}", "<p>a\tb</p>");
    assert_eq!(result.runs.lines.len(), 1);
    // a 在第 1 列，制表符补到第 8 列：7 个空格。
    assert_eq!(line_texts(&result), ["a       b"]);
    assert!((result.runs.lines[0].rect.w - 38.0).abs() < 0.01);
}

#[test]
fn pre_does_not_soft_wrap() {
    let result = run(20.0, "p{white-space:pre}", "<p>aaaa bbbb</p>");
    assert_eq!(result.runs.lines.len(), 1);
    assert!(result.runs.lines[0].rect.w > 20.0);
}

#[test]
fn pre_line_collapses_spaces_but_keeps_hard_breaks() {
    let result = run(200.0, "p{white-space:pre-line}", "<p>a  b\nc</p>");
    assert_eq!(result.runs.lines.len(), 2);
    assert_eq!(line_texts(&result), ["a b", "c"]);
    assert!((result.runs.lines[0].rect.w - 14.0).abs() < 0.01);
    assert!((result.runs.lines[1].rect.w - 5.0).abs() < 0.01);
}

#[test]
fn pre_wrap_soft_wraps_and_preserves_spaces() {
    let result = run(65.0, "p{white-space:pre-wrap}", "<p>aaaa  bbbb  cccc</p>");
    assert_eq!(result.runs.lines.len(), 2);
    // 软换行发生在最后一个双空格之后，行末空格保留。
    assert!((result.runs.lines[0].rect.w - 56.0).abs() < 0.01);
    assert!((result.runs.lines[1].rect.w - 20.0).abs() < 0.01);
    assert_eq!(line_texts(&result), ["aaaa  bbbb  ", "cccc"]);
}

#[test]
fn break_spaces_does_not_trim_line_end_spaces() {
    let result = run(17.0, "p{white-space:break-spaces}", "<p>aa  bb</p>");
    assert_eq!(result.runs.lines.len(), 2);
    // 第一行末尾的空格属于内容，不能被裁掉；第二行行首空格同样保留。
    assert_eq!(line_texts(&result), ["aa ", " bb"]);
    assert!((result.runs.lines[0].rect.w - 14.0).abs() < 0.01);
    assert!((result.runs.lines[1].rect.w - 14.0).abs() < 0.01);
}

#[test]
fn normal_still_collapses_spaces_and_treats_newlines_as_spaces() {
    let result = run(200.0, "", "<p>a  b\nc</p>");
    assert_eq!(result.runs.lines.len(), 1);
    assert_eq!(line_texts(&result), ["a b c"]);
    assert!((result.runs.lines[0].rect.w - 23.0).abs() < 0.01);
}
