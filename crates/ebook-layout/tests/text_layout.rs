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

fn run_y(result: &LayoutResult, needle: &str) -> f32 {
    result
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::GlyphRun(run) if run.text == needle => Some(run.origin.y),
            _ => None,
        })
        .expect("glyph run")
}

#[test]
fn mixed_font_sizes_share_one_line_box() {
    let result = run(
        200.0,
        "p{line-height:normal} .big{font-size:20px} .small{font-size:10px}",
        "<p>a<span class=\"big\">b</span><span class=\"small\">c</span></p>",
    );
    assert_eq!(result.runs.lines.len(), 1);
    let line = &result.runs.lines[0];
    assert!((line.rect.h - 30.0).abs() < 0.01);
    assert!((line.baseline - 19.0).abs() < 0.01);
    assert!((run_y(&result, "a") - 19.0).abs() < 0.01);
    assert!((run_y(&result, "b") - 19.0).abs() < 0.01);
    assert!((run_y(&result, "c") - 19.0).abs() < 0.01);
}

#[test]
fn superscript_and_subscript_shift_baselines_and_grow_the_line() {
    let result = run(
        200.0,
        ".sup{vertical-align:super} .sub{vertical-align:sub}",
        "<p>a<span class=\"sup\">b</span><span class=\"sub\">c</span>d</p>",
    );
    assert_eq!(result.runs.lines.len(), 1);
    let line = &result.runs.lines[0];
    assert!(line.rect.h > 20.0);
    let base = run_y(&result, "a");
    assert!(run_y(&result, "b") < base);
    assert!(run_y(&result, "c") > base);
    assert!((base - run_y(&result, "d")).abs() < 0.01);
    assert!((run_y(&result, "b") - (base - 16.0 / 3.0)).abs() < 0.01);
    assert!((run_y(&result, "c") - (base + 16.0 / 5.0)).abs() < 0.01);
}

#[test]
fn vertical_align_length_raises_the_baseline() {
    let result = run(
        200.0,
        ".up{vertical-align:5px}",
        "<p>a<span class=\"up\">b</span></p>",
    );
    let base = run_y(&result, "a");
    assert!((run_y(&result, "b") - (base - 5.0)).abs() < 0.01);
}

#[test]
fn parent_strut_keeps_a_small_inline_from_collapsing_the_line() {
    let result = run(
        200.0,
        "p{line-height:normal} span{font-size:8px;line-height:normal}",
        "<p><span>a</span></p>",
    );
    assert_eq!(result.runs.lines.len(), 1);
    let line = &result.runs.lines[0];
    assert!((line.rect.h - 24.0).abs() < 0.01);
    assert!((line.baseline - 16.0).abs() < 0.01);
    assert!((run_y(&result, "a") - 16.0).abs() < 0.01);
}

#[test]
fn parent_and_first_child_top_margins_collapse() {
    let result = run(
        200.0,
        ".parent{margin-top:20px}.child{margin-top:10px;line-height:10px}",
        "<div class=\"parent\">\n <div class=\"child\">x</div>\n</div>",
    );
    assert_eq!(result.runs.lines.len(), 1);
    assert!((result.runs.lines[0].rect.y - 20.0).abs() < 0.01);
}

#[test]
fn border_top_prevents_parent_child_margin_collapse() {
    let result = run(
        200.0,
        ".parent{margin-top:20px;border:1px solid}.child{margin-top:10px;line-height:10px}",
        "<div class=\"parent\"><div class=\"child\">x</div></div>",
    );
    assert_eq!(result.runs.lines.len(), 1);
    assert!((result.runs.lines[0].rect.y - 31.0).abs() < 0.01);
}

#[test]
fn adjacent_positive_and_negative_margins_collapse() {
    let result = run(
        200.0,
        ".a{line-height:10px;margin-bottom:-5px}.b{line-height:10px;margin-top:10px}",
        "<div class=\"a\">a</div><div class=\"b\">b</div>",
    );
    assert_eq!(result.runs.lines.len(), 2);
    let gap = result.runs.lines[1].rect.y - result.runs.lines[0].rect.bottom();
    assert!((gap - 5.0).abs() < 0.01);
}

#[test]
fn empty_block_collapses_its_own_margins() {
    let result = run(
        200.0,
        ".empty{margin-top:30px;margin-bottom:10px}.line{line-height:10px}",
        "<div class=\"empty\"></div><div class=\"line\">x</div>",
    );
    assert_eq!(result.runs.lines.len(), 1);
    assert!((result.runs.lines[0].rect.y - 30.0).abs() < 0.01);
}

#[test]
fn empty_block_keeps_negative_margins_in_the_collapsed_chain() {
    let result = run(
        200.0,
        ".a{line-height:10px;margin-bottom:-5px}.empty{margin-top:10px;margin-bottom:20px}\
         .b{line-height:10px}",
        "<div class=\"a\">a</div><div class=\"empty\"></div><div class=\"b\">b</div>",
    );
    assert_eq!(result.runs.lines.len(), 2);
    let gap = result.runs.lines[1].rect.y - result.runs.lines[0].rect.bottom();
    assert!((gap - 15.0).abs() < 0.01);
}

#[test]
fn cjk_stays_one_glyph_per_character() {
    // 没有 CJK 字体时走缺字路径；这里用 Fixed 度量，每个汉字仍然各自一个字形、
    // 各自贡献一个 advance（不会被整形合并或挤压）。
    let result = run(200.0, "", "<p>中文排</p>");
    assert_eq!(result.runs.lines.len(), 1);
    assert!((result.runs.lines[0].rect.w - 30.0).abs() < 0.01);
    let glyphs: Vec<_> = result
        .display
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::GlyphRun(run) => Some(result.display.glyph_slice(run.glyphs).to_vec()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(glyphs.len(), 3);
    assert!(glyphs.iter().all(|glyph| (glyph.dx - 10.0).abs() < 0.01));
    assert_eq!(
        glyphs.iter().map(|glyph| glyph.ch).collect::<Vec<_>>(),
        vec![0, 3, 6]
    );
}
