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

/// 记录字形查询次数：窗口外的 op 连光栅查询都不该发生。
struct Counting {
    calls: std::cell::Cell<usize>,
}

impl Resources for Counting {
    fn glyph_alpha(&mut self, _font: FontId, _gid: u16, _px: f32) -> Option<GlyphBitmap> {
        self.calls.set(self.calls.get() + 1);
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

fn counted(ir: &LayoutResult, top: f32, height: f32) -> usize {
    let mut surface = RgbaSurface::new(200, height as u32);
    surface.clear(Rgba8::WHITE);
    let mut res = Counting {
        calls: std::cell::Cell::new(0),
    };
    ebook_render::paint_window(ir, 0, top, &mut surface, &mut res).expect("window");
    res.calls.get()
}

#[test]
fn a_tile_only_touches_glyphs_inside_its_band() {
    let ir = scroll_ir();
    let page = &ir.pages[0];
    // 整页 = 全章：每行都查。
    let whole = counted(&ir, 0.0, page.size.h);
    assert!(whole > 100, "整页字形数 {whole}");

    // 128pt 的一屏只该查那几行的字形，而不是整章。
    let tile = counted(&ir, page.size.h / 2.0, 128.0);
    assert!(tile > 0, "窗口里应该有字");
    assert!(
        tile * 4 < whole,
        "128pt 窗口查了 {tile} 个字形，整章 {whole}：裁剪没生效"
    );

    // 窗口下移仍然只查自己那一段（代价跟 y 无关，跟窗口高有关）。
    let lower = counted(&ir, page.size.h - 128.0, 128.0);
    assert!(lower > 0);
    assert!(lower * 4 < whole, "底部窗口查了 {lower} 个字形");

    // 内容之外的窗口一个字形都不查。
    assert_eq!(counted(&ir, page.size.h + 500.0, 128.0), 0);
}
