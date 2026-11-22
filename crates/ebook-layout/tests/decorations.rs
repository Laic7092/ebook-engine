//! 块级背景/边框：布局层要真的产出 `Op::Rect` / `Op::Border`，并且跨页时
//! 每一页只拿自己那一段（首段有上边框、末段有下边框，中间段只有左右）。

use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{FontId, LayoutResult, Op, Rgba8, StyledDoc};
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
        8.0
    }

    fn descent(&self, _font: FontId, _size: f32) -> f32 {
        2.0
    }
}

fn layout_html(css: &str, body: &str, width: f32, height: f32) -> LayoutResult {
    let html = format!(
        "<html><head><style>body{{margin:0}}p{{margin:0;padding:0;border:0;\
         line-height:20px}}{css}</style></head><body>{body}</body></html>"
    );
    let doc: StyledDoc = build(&html, 0, &mut NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(width, height);
    layout(&doc, &env, &constraint)
}

#[test]
fn block_background_and_border_are_emitted_around_the_content() {
    let ir = layout_html(
        "div{background:#ff0000;border:2px solid #0000ff;padding:4px;margin:6px}",
        "<div>abc</div>",
        200.0,
        200.0,
    );
    let ops = &ir.display.ops;
    let background = ops
        .iter()
        .position(|op| matches!(op, Op::Rect { .. }))
        .expect("background op");
    let border = ops
        .iter()
        .position(|op| matches!(op, Op::Border(_)))
        .expect("border op");
    let first_glyph = ops
        .iter()
        .position(|op| matches!(op, Op::GlyphRun(_)))
        .expect("glyph op");
    assert!(background < border, "背景必须先于边框");
    assert!(border < first_glyph, "边框必须晚于内容之前绘制");

    let Op::Rect { rect, color } = &ops[background] else {
        unreachable!()
    };
    let Op::Border(border) = &ops[border] else {
        unreachable!()
    };
    assert_eq!(*color, Rgba8::new(255, 0, 0, 255));
    assert_eq!(rect, &border.rect, "背景默认铺满边框盒");
    assert_eq!(border.width.top, 2.0);
    assert_eq!(border.width.right, 2.0);
    assert_eq!(border.width.bottom, 2.0);
    assert_eq!(border.width.left, 2.0);
    assert_eq!(border.color, Rgba8::new(0, 0, 255, 255));

    // margin 6 + border 2 + padding 4，宽度 = 200 - 2×6。
    assert!((rect.x - 6.0).abs() < 0.01, "{rect:?}");
    assert!((rect.w - 188.0).abs() < 0.01, "{rect:?}");
    assert!(rect.h > 0.0);
}

#[test]
fn a_border_spanning_pages_is_split_and_keeps_only_the_right_ends() {
    let text = "word ".repeat(120);
    let ir = layout_html(
        "div{background:#eeeeee;border:2px solid #333333;padding:3px}",
        &format!("<div>{text}</div>"),
        120.0,
        60.0,
    );
    assert!(ir.pages.len() > 2, "test page must span several pages");

    let mut pages_with_border = 0;
    let mut pages_with_background = 0;
    for (index, page) in ir.pages.iter().enumerate() {
        let ops = ir.display.op_slice(page.ops);
        let borders: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Border(border) => Some(border),
                _ => None,
            })
            .collect();
        let backgrounds: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Rect { rect, .. } => Some(rect),
                _ => None,
            })
            .collect();
        if !borders.is_empty() {
            pages_with_border += 1;
        }
        if !backgrounds.is_empty() {
            pages_with_background += 1;
        }
        assert_eq!(
            borders.len(),
            backgrounds.len(),
            "page {index}: 背景与边框必须成对出现"
        );
        for (border, background) in borders.iter().zip(&backgrounds) {
            assert_eq!(
                &border.rect, *background,
                "page {index}: 背景盒与边框盒错位"
            );
            assert!(border.rect.h > 0.0, "page {index}: 空段不应产出 op");
            assert!(
                border.rect.y >= page.origin.y - 0.01,
                "page {index}: 段起点越出页顶"
            );
            assert!(
                border.rect.bottom() <= page.origin.y + page.size.h + 0.01,
                "page {index}: 段终点越出页底"
            );
            let first = index == 0;
            let last = index + 1 == ir.pages.len();
            assert_eq!(
                border.width.top,
                if first { 2.0 } else { 0.0 },
                "page {index}: 只有首段有上边框"
            );
            assert_eq!(
                border.width.bottom,
                if last { 2.0 } else { 0.0 },
                "page {index}: 只有末段有下边框"
            );
            assert_eq!(border.width.left, 2.0, "page {index}: 左边框不能断");
            assert_eq!(border.width.right, 2.0, "page {index}: 右边框不能断");
        }
    }
    assert!(pages_with_border > 1, "边框没有跨页切段");
    assert_eq!(pages_with_background, pages_with_border);
}

#[test]
fn background_starts_after_the_collapsed_top_margin() {
    let ir = layout_html(
        "p{margin:20px 0}div{background:#00ff00}",
        "<div><p>abc</p></div>",
        200.0,
        200.0,
    );
    let Op::Rect { rect, .. } = ir
        .display
        .ops
        .iter()
        .find(|op| matches!(op, Op::Rect { .. }))
        .expect("background")
    else {
        unreachable!()
    };
    // div 没有 border/padding，顶外边距与 p 折叠；背景从折叠后的位置开始，
    // 而不是从 0 开始把外边距也涂上。
    assert!((rect.y - 20.0).abs() < 0.01, "{rect:?}");
    let first_line = &ir.runs.lines[0];
    assert!(
        (first_line.rect.y - 20.0).abs() < 0.01,
        "{:?}",
        first_line.rect
    );
    assert!(rect.bottom() >= first_line.rect.bottom() - 0.01, "{rect:?}");
}
