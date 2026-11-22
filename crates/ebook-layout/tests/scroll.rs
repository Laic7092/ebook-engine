//! 滚动模式：整章排成一页，断行与分页模式逐行一致，`Page.size.h` 是内容
//! 实际高度，所有 op/line 都在这一页里。

use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{EdgesF, LayoutResult, StyledDoc};
use ebook_layout::{
    Layout, LayoutEnv, LayoutOptions, Mode, NoImageSizes, PageConstraint, StubMeasurer,
};

const CHARS_PER_LINE: usize = 66;

fn lines(n: usize) -> String {
    "a".repeat(n * CHARS_PER_LINE)
}

fn layout_html(mode: Mode, css: &str, body: &str) -> LayoutResult {
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
    let options = LayoutOptions {
        mode,
        ..LayoutOptions::default()
    };
    Layout::new().layout_with(&doc, &env, &constraint, &options)
}

#[test]
fn scroll_is_one_page_with_every_line() {
    let body = format!("<p>{}</p>", lines(40));
    let paged = layout_html(Mode::Paged, "", &body);
    let scroll = layout_html(Mode::Scroll, "", &body);

    assert!(paged.pages.len() > 1, "分页模式应该不止一页");
    assert_eq!(scroll.pages.len(), 1, "滚动模式整章一页");
    assert_eq!(
        paged.runs.lines.len(),
        scroll.runs.lines.len(),
        "滚动不重新断行，行数必须相同"
    );

    let page = &scroll.pages[0];
    assert_eq!(page.origin.x, 0.0);
    assert_eq!(page.origin.y, 0.0);
    assert_eq!(page.ops.start, 0);
    assert_eq!(page.ops.len as usize, scroll.display.ops.len());
    assert_eq!(page.lines.start, 0);
    assert_eq!(page.lines.len as usize, scroll.runs.lines.len());
    // 内容盒宽仍是正文宽；高是最后一行底边，不是视口高。
    assert!((page.size.w - (600.0 - 48.0)).abs() < 0.01);
    let last_bottom = scroll.runs.lines.last().unwrap().rect.bottom();
    assert!(
        (page.size.h - last_bottom).abs() < 0.01,
        "page {} vs last line {}",
        page.size.h,
        last_bottom
    );
    assert!(page.size.h > 800.0 - 48.0, "整章比一屏高");
}

#[test]
fn scroll_shares_breaks_and_glyphs_with_paged() {
    let body = format!("<p>{}</p><h2>heading</h2><p>{}</p>", lines(29), lines(40));
    let paged = layout_html(Mode::Paged, "p{widows:2;orphans:2}", &body);
    let scroll = layout_html(Mode::Scroll, "p{widows:2;orphans:2}", &body);
    assert_eq!(scroll.runs.lines, paged.runs.lines);
    assert_eq!(scroll.display.glyphs, paged.display.glyphs);
}

#[test]
fn scroll_is_deterministic() {
    let body = format!("<p>{}</p><p>{}</p>", lines(29), lines(17));
    assert_eq!(
        layout_html(Mode::Scroll, "", &body),
        layout_html(Mode::Scroll, "", &body)
    );
}

#[test]
fn an_empty_chapter_makes_no_page() {
    let scroll = layout_html(Mode::Scroll, "", "");
    assert!(scroll.pages.is_empty());
    assert!(scroll.runs.lines.is_empty());
}
