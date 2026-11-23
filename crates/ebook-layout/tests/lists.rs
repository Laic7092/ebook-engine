//! 列表 marker：`<ul>` 出圆点、`<ol>` 出序号，marker 不并进正文。

use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{FontId, LayoutResult, Op, StyledDoc};
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

fn run(css: &str, body: &str) -> LayoutResult {
    let html = format!(
        "<html><head><style>body{{margin:0}}p,li{{margin:0;padding:0;border:0;\
         line-height:20px}}{css}</style></head><body>{body}</body></html>"
    );
    let doc: StyledDoc = build(&html, 0, &mut NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &NoImageSizes,
    };
    layout(&doc, &env, &PageConstraint::new(400.0, 800.0))
}

fn marker_texts(result: &LayoutResult) -> Vec<String> {
    result
        .display
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::GlyphRun(run) if run.text.starts_with('\u{2022}') || run.text.ends_with(". ") => {
                Some(run.text.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn unordered_items_get_a_bullet() {
    let result = run("", "<ul><li>alpha</li><li>beta</li></ul>");
    assert_eq!(result.runs.lines.len(), 2);
    let markers = marker_texts(&result);
    assert_eq!(markers.len(), 2, "每个 li 一个 marker: {markers:?}");
    assert!(markers.iter().all(|text| text.starts_with('\u{2022}')));
}

#[test]
fn ordered_items_count_from_one() {
    let result = run("", "<ol><li>alpha</li><li>beta</li><li>gamma</li></ol>");
    assert_eq!(marker_texts(&result), vec!["1. ", "2. ", "3. "]);
}

#[test]
fn ol_start_attribute_is_honored() {
    let result = run("", "<ol start=\"3\"><li>alpha</li><li>beta</li></ol>");
    assert_eq!(marker_texts(&result), vec!["3. ", "4. "]);

    // 书里常见的 `ol { list-style-type: decimal }` 不能把 start 重置成 1。
    let result = run(
        "ol{list-style-type:decimal}",
        "<ol start=\"3\"><li>alpha</li><li>beta</li></ol>",
    );
    assert_eq!(marker_texts(&result), vec!["3. ", "4. "]);
}

#[test]
fn list_style_none_suppresses_markers() {
    let result = run(
        "ul{list-style-type:none}",
        "<ul><li>alpha</li><li>beta</li></ul>",
    );
    assert!(marker_texts(&result).is_empty());
}

#[test]
fn markers_sit_outside_the_line_and_out_of_the_selection() {
    let result = run("", "<ul><li>alpha</li></ul>");
    let line = &result.runs.lines[0];
    let marker = result
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::GlyphRun(run) if run.text.starts_with('\u{2022}') => Some(run),
            _ => None,
        })
        .expect("marker run");
    assert!(
        marker.origin.x < line.rect.x,
        "marker 在内容左侧: {} vs {}",
        marker.origin.x,
        line.rect.x
    );
    assert!(
        marker.glyphs.start >= line.glyphs.end(),
        "marker 不属于正文行区间"
    );
}
