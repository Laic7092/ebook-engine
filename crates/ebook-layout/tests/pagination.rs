use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{EdgesF, LayoutResult, StyledDoc};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

const CHARS_PER_LINE: usize = 66;

fn lines(n: usize) -> String {
    "a".repeat(n * CHARS_PER_LINE)
}

fn layout_html(css: &str, body: &str) -> LayoutResult {
    layout_html_lang(None, css, body)
}

/// `lang` 决定版式档案：`None` 走零语言假设的 `generic`，`zh` 走简中档案。
fn layout_html_lang(lang: Option<&str>, css: &str, body: &str) -> LayoutResult {
    let lang = lang.map(|l| format!(" lang=\"{l}\"")).unwrap_or_default();
    let html = format!(
        "<html{lang}><head><style>body{{margin:0}}p{{margin:0;line-height:24px}}\
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

/// 简中档案（CLREQ §7.1.2）把“孤行挪到上一页、突出版心”放在第一位：
/// 这一页多放一行，页盒长高一点，而不是回撤留白。
#[test]
fn zh_profile_pushes_the_widow_out_of_the_type_area() {
    // 32 行放进 31 行的页：最后一行会单独变成下一页的孤行。
    let body = format!("<p>{}</p>", lines(32));
    let generic = layout_html("", &body);
    let zh = layout_html_lang(Some("zh"), "", &body);
    assert_eq!(per_page(&generic), vec![30, 2]);
    assert_eq!(per_page(&zh), vec![32]);
    // 版心高 752 对不齐 24px 的行高，简中档案只放行 31 行（744）；
    // 孤行推出版心时要真的越过 752 才让页盒长高，所以这里是 768。
    assert_eq!(zh.pages[0].size.h, 768.0);
    for line in &zh.runs.lines[zh.pages[0].lines.as_range()] {
        assert!(line.rect.bottom() <= zh.pages[0].origin.y + zh.pages[0].size.h + 0.01);
    }
}

/// 推不动（剩下不止一行）时退回 generic 的回撤行为，不会为了齐底无限往下塞。
#[test]
fn zh_profile_gives_up_pushing_out_more_than_one_line() {
    let body = format!("<p>{}</p>", lines(40));
    let zh = layout_html_lang(Some("zh"), "p{widows:10}", &body);
    assert_eq!(per_page(&zh), vec![30, 10]);
    for page in &zh.pages {
        assert_eq!(page.size.h, 752.0);
    }
}

/// 简中档案把版心高度对齐到行高（744 = 31 × 24），但档案同时允许 `push_lines`
/// 行的「突出版心」（CLREQ §7.1.2）。所以页尾是个 28px 的标题时，宁可让它探出
/// 格线 4px，也不把它单独甩到下一页——把标题甩走才是真的难看。
///
/// 版心 752 装得下 748，页盒不长高；探出去的只有那条格线，不是版心。
#[test]
fn zh_dp_keeps_a_trailing_heading_on_the_page() {
    // 30 行正文（720）+ 一个 28px 行高的标题 = 748。
    let body = format!("<p>{}</p><h2>h</h2>", lines(30));
    let css = "h2{line-height:28px}";
    let generic = layout_html(css, &body);
    let zh = layout_html_lang(Some("zh"), css, &body);
    assert_eq!(per_page(&generic), vec![31]);
    assert_eq!(per_page(&zh), vec![31]);
    assert_eq!(zh.pages[0].size.h, 752.0);
    assert_eq!(zh.pages[0].origin.y, 0.0);
}

/// 页级 DP 只在该多放一行**能少开一页**的时候才突出版心。
///
/// 40 行正文：31 + 9 和 32 + 8 页数相同，但前者页底正好齐平，代价更小。
#[test]
fn zh_dp_does_not_push_out_without_gaining_a_page() {
    let zh = layout_html_lang(Some("zh"), "", &format!("<p>{}</p>", lines(40)));
    assert_eq!(per_page(&zh), vec![31, 9]);
    assert_eq!(zh.pages[0].size.h, 752.0);
}

/// 同一份输入排两遍，切点必须逐字节一样：DP 的代价是浮点数，平局要有确定的
/// 胜负（先到先得），否则同一本书在真机和 WASM 上分页会不同。
#[test]
fn zh_dp_is_deterministic() {
    let body = format!(
        "<p>{}</p><p>{}</p><p>{}</p>",
        lines(17),
        lines(23),
        lines(31)
    );
    let first = layout_html_lang(Some("zh"), "", &body);
    let second = layout_html_lang(Some("zh"), "", &body);
    assert_eq!(per_page(&first), per_page(&second));
    assert_eq!(first.pages.len(), second.pages.len());
    for (a, b) in first.pages.iter().zip(&second.pages) {
        assert_eq!(a.origin, b.origin);
        assert_eq!(a.size, b.size);
    }
}

/// 页底齐平（LaTeX `\flushbottom`）：页面高度除不尽行高时，零头摊到这一页的
/// **段间距**上，而不是留一条白、也不是硬塞一行。
///
/// 4 段 × 7 行 + 3 道 10px 段间距 = 702，版心对齐后 744，剩 42。每道缝最多撑
/// 0.1 行（2.4px），3 道只能摊 7.2——剩下 34.8px 就让它空着。宁可页底留白，
/// 也不把整页的段间距改成「这页怎么这么松」。行距一个像素都不动。
#[test]
fn zh_flushes_the_page_bottom_into_paragraph_gaps() {
    // 末尾那一段强制换页：否则 DP 会往这一页再塞一行（突出版心），
    // 页面就不再有零头可摊，测的就不是齐平了。
    let body = format!(
        "{}<p class=\"next\">{}</p>",
        (0..4)
            .map(|_| format!("<p>{}</p>", lines(7)))
            .collect::<String>(),
        lines(7)
    );
    let css = "p{margin:0 0 10px} p.next{break-before:page}";
    let zh = layout_html_lang(Some("zh"), css, &body);
    let first_page = &zh.runs.lines[zh.pages[0].lines.as_range()];
    assert_eq!(first_page.len(), 28);
    let mut gaps = Vec::new();
    for pair in first_page.windows(2) {
        let gap = pair[1].rect.y - (pair[0].rect.y + pair[0].rect.h);
        if gap > 0.5 {
            gaps.push(gap);
        }
    }
    assert_eq!(gaps.len(), 3, "{gaps:?}");
    for gap in gaps {
        assert!((gap - 12.4).abs() < 0.01, "{gap}");
    }
    let last = first_page.last().unwrap();
    assert!(
        (last.rect.y + last.rect.h - 709.2).abs() < 0.01,
        "{}",
        last.rect.y + last.rect.h
    );
    // 差的那 34.8px 是承意的：撑不下就留白，别把整页撑松。
    assert!(last.rect.y + last.rect.h <= 744.0 + 0.01);
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
