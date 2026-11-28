//! 宿主选的版式档案：`LayoutOptions.profile` 指了就用它，否则照 `meta.lang`。
//!
//! 两件事要钉住：选「中文」和书上写 `lang="zh"` 完全等价；选「通用」能把
//! 一本 `lang="zh"` 的书按零语言假设的规矩排。

use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{EdgesF, LayoutResult, StyledDoc};
use ebook_layout::{
    Layout, LayoutEnv, LayoutOptions, Mode, NoImageSizes, PageConstraint, Profile, StubMeasurer,
};

const BODY: &str = "他说：“这本书不好读。”她笑了一下，没有接话。窗外下着雨，\
雨点打在铁皮雨棚上，响得像有人在上面走。第二天早上，院子里积了一层水，\
倒映着灰白的天。\n\n“要走吗？”他问。她摇头，把书翻到下一页，慢慢看了下去。";

fn layout(lang: Option<&str>, profile: Option<Profile>) -> LayoutResult {
    let html = format!(
        "<html><head><style>body{{margin:0}}p{{margin:0;line-height:24px}}\
         </style></head><body><p>{BODY}</p></body></html>"
    );
    let mut doc: StyledDoc = build(&html, 0, &mut NoAssets).expect("xhtml");
    doc.meta.lang = lang.map(str::to_string);
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(320.0, 480.0).with_margin(EdgesF::all(20.0));
    let options = LayoutOptions {
        mode: Mode::Paged,
        profile,
        ..LayoutOptions::default()
    };
    Layout::new().layout_with(&doc, &env, &constraint, &options)
}

#[test]
fn choosing_chinese_is_the_same_as_the_book_saying_zh() {
    let said = layout(Some("zh"), None);
    let chosen = layout(None, Some(Profile::ZhHans));
    assert_eq!(
        format!("{said:?}"),
        format!("{chosen:?}"),
        "设置里选「中文」必须和书上写 lang=zh 逐字节一致"
    );
}

#[test]
fn choosing_generic_overrides_a_chinese_book() {
    let book_says = layout(Some("zh"), None);
    let forced = layout(Some("zh"), Some(Profile::Generic));
    assert_ne!(
        format!("{book_says:?}"),
        format!("{forced:?}"),
        "简中档案会两端对齐、走行级 DP，通用档案不会：结果应当不同"
    );
}

#[test]
fn no_choice_leaves_the_book_in_charge() {
    assert_eq!(
        format!("{:?}", layout(Some("en"), None)),
        format!("{:?}", layout(None, None)),
        "没有 lang 和 lang=en 都落回通用档案"
    );
}
