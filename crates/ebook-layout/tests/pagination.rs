use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{EdgesF, LayoutResult, StyledDoc};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

const CHARS_PER_LINE: usize = 66;

fn lines(n: usize) -> String {
    "a".repeat(n * CHARS_PER_LINE)
}

fn layout_html(css: &str, body: &str) -> LayoutResult {
    let html = format!(
        "<html><head><style>body{{margin:0}}p{{margin:0;line-height:24px}}\
         h2{{margin:0;line-height:24px}}{css}</style></head><body>{body}</body></html>"
    );
    let doc: StyledDoc = build(&html, 0, &mut NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
    layout(&doc, &env, &constraint)
}

fn per_page(result: &LayoutResult) -> Vec<usize> {
    result.pages.iter().map(|p| p.lines.len as usize).collect()
}

#[test]
fn greedy_fills_thirty_one_lines() {
    let result = layout_html("", &format!("<p>{}</p>", lines(40)));
    assert_eq!(per_page(&result), vec![31, 9]);
}

#[test]
fn widows_pulls_lines_to_the_next_page() {
    let result = layout_html("p{widows:10}", &format!("<p>{}</p>", lines(40)));
    assert_eq!(per_page(&result), vec![30, 10]);
}

#[test]
fn orphans_pushes_a_short_fragment_down() {
    let body = format!("<p>{}</p><p>{}</p>", lines(29), lines(40));
    assert_eq!(per_page(&layout_html("", &body)), vec![31, 31, 7]);
    assert_eq!(
        per_page(&layout_html("p{orphans:3}", &body)),
        vec![29, 31, 9]
    );
}

#[test]
fn heading_is_not_stranded_at_the_bottom() {
    let body = format!("<p>{}</p><h2>h</h2><p>{}</p>", lines(30), lines(5));
    assert_eq!(per_page(&layout_html("", &body)), vec![30, 6]);
    assert_eq!(
        per_page(&layout_html("h2{break-after:auto}", &body)),
        vec![31, 5]
    );
}

#[test]
fn break_before_page_starts_a_new_page() {
    let body = format!("<p>{}</p><h2>h</h2><p>{}</p>", lines(5), lines(5));
    let result = layout_html("h2{break-before:page}", &body);
    assert_eq!(per_page(&result), vec![5, 6]);
    assert_eq!(result.pages[1].lines.start, 5);
}

#[test]
fn break_before_at_the_top_does_not_emit_an_empty_page() {
    let body = "<h2>h</h2><p>aaaa</p>".to_string();
    let result = layout_html("h2{break-before:page}", &body);
    assert_eq!(result.pages.len(), 1);
}

#[test]
fn pages_start_at_their_first_line() {
    let result = layout_html("", &format!("<p>{}</p>", lines(70)));
    assert!(result.pages.len() >= 3);
    assert_eq!(result.pages[0].origin.y, 0.0);
    for page in &result.pages {
        let first = &result.runs.lines[page.lines.as_range()][0];
        assert_eq!(page.origin.y, first.rect.y);
        for line in &result.runs.lines[page.lines.as_range()] {
            assert!(line.rect.bottom() <= page.origin.y + page.size.h + 0.01);
        }
    }
    for pair in result.runs.lines.windows(2) {
        assert!(pair[0].rect.y <= pair[1].rect.y);
    }
}

#[test]
fn pagination_is_deterministic() {
    let body = format!(
        "<p>{}</p><h2>h</h2><p>{}</p><p>{}</p>",
        lines(29),
        lines(40),
        lines(7)
    );
    let first = layout_html("p{widows:2;orphans:2}", &body);
    let second = layout_html("p{widows:2;orphans:2}", &body);
    assert_eq!(first, second);
}
