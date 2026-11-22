//! `paint_window`：滚动瓦片只画自己那一段，流坐标 `top` 落在 surface 的
//! y=0，窗口之外的内容不画。

use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{
    EdgesF, FontId, GlyphBitmap, ImageData, ImageId, LayoutResult, Resources, Rgba8, StyledDoc,
};
use ebook_layout::{
    Layout, LayoutEnv, LayoutOptions, Mode, NoImageSizes, PageConstraint, StubMeasurer,
};
use ebook_raster::RgbaSurface;

struct FixedResources;

impl Resources for FixedResources {
    fn glyph_alpha(&mut self, _font: FontId, _gid: u16, _px: f32) -> Option<GlyphBitmap> {
        Some(GlyphBitmap {
            w: 8,
            h: 8,
            left: 0,
            top: -8,
            alpha: vec![128; 64],
        })
    }

    fn image(&mut self, _id: ImageId) -> Option<ImageData> {
        None
    }
}

fn body(paragraphs: usize) -> String {
    let line = "a".repeat(60);
    (0..paragraphs)
        .map(|_| format!("<p>{line}</p>"))
        .collect::<String>()
}

fn scroll_ir() -> LayoutResult {
    let html = format!(
        "<html><head><style>body{{margin:0}}p{{margin:0;font-size:16px;line-height:24px}}\
         </style></head><body>{}</body></html>",
        body(20)
    );
    let doc: StyledDoc = build(&html, 0, &mut NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(200.0, 200.0).with_margin(EdgesF::all(0.0));
    let options = LayoutOptions {
        mode: Mode::Scroll,
        ..LayoutOptions::default()
    };
    Layout::new().layout_with(&doc, &env, &constraint, &options)
}

fn window(ir: &LayoutResult, top: f32, height: f32) -> RgbaSurface {
    let mut surface = RgbaSurface::new(200, height as u32);
    surface.clear(Rgba8::WHITE);
    let mut res = FixedResources;
    ebook_render::paint_window(ir, 0, top, &mut surface, &mut res).expect("window");
    surface
}

fn ink(surface: &RgbaSurface) -> usize {
    surface
        .pixels()
        .iter()
        .filter(|pixel| **pixel != Rgba8::WHITE)
        .count()
}

#[test]
fn a_tile_starts_at_its_own_content_y() {
    let ir = scroll_ir();
    assert_eq!(ir.pages.len(), 1);
    let page = &ir.pages[0];
    assert!(page.size.h > 400.0, "content height {}", page.size.h);

    let top = window(&ir, 0.0, 200.0);
    // 同一份内容，窗口下移半行：像素必然跟着走，说明 `top` 真的在平移，
    // 而不是每次从页首重画。
    let shifted = window(&ir, 12.0, 200.0);
    let bottom = window(&ir, page.size.h - 200.0, 200.0);
    assert!(ink(&top) > 0);
    assert!(ink(&shifted) > 0);
    assert!(ink(&bottom) > 0);
    assert_ne!(top.pixels(), shifted.pixels());
}

#[test]
fn beyond_the_content_is_paper() {
    let ir = scroll_ir();
    let page = &ir.pages[0];
    let empty = window(&ir, page.size.h + 10_000.0, 200.0);
    assert_eq!(ink(&empty), 0);
}

#[test]
fn a_window_is_the_same_every_time() {
    let ir = scroll_ir();
    let a = window(&ir, 96.0, 240.0);
    let b = window(&ir, 96.0, 240.0);
    assert_eq!(a.pixels(), b.pixels());
}
