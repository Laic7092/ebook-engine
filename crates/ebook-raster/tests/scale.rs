//! `ScaledSurface`：只乘几何，不改 IR。
//!
//! 两个必须成立的性质：
//! 1. `scale = 1` 与“不加装饰器直接画”逐字节相同（native 基线不变）。
//! 2. `scale = n` 时坐标、边框、advance 与**字号**一起乘 n——字号是光栅化
//!    入参，只有它变了，字形才是按设备像素渲染而不是被插值放大。

use ebook_ir::{EdgesF, GlyphBitmap, ImageId, Op, RectF, Resources, Rgba8, Span, StyledDoc};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
use ebook_raster::RgbaSurface;
use ebook_render::{paint, ScaledSurface};

/// 记录字体光栅化被要求的字号，返回一个固定 8×8 的方块。
#[derive(Default)]
struct Probe {
    sizes: Vec<f32>,
}

impl Resources for Probe {
    fn glyph_alpha(&mut self, _font: ebook_ir::FontId, _gid: u16, px: f32) -> Option<GlyphBitmap> {
        self.sizes.push(px);
        Some(GlyphBitmap {
            w: 8,
            h: 8,
            left: 0,
            top: -8,
            alpha: vec![255; 64],
        })
    }

    fn image(&mut self, _id: ImageId) -> Option<ebook_ir::ImageData> {
        None
    }
}

/// 20×20 的页面：一段 16px 文本 + 一块盖住左上半页的黑矩形。
fn ir() -> ebook_ir::LayoutResult {
    let html =
        "<html><body><p style=\"margin:0;font-size:16px;line-height:24px\">A</p></body></html>";
    let doc: StyledDoc =
        ebook_format::xhtml::build(html, 0, &mut ebook_format::xhtml::NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(20.0, 20.0).with_margin(EdgesF::all(0.0));
    let mut ir = layout(&doc, &env, &constraint);
    let start = ir.display.ops.len() as u32;
    ir.display.ops.push(Op::Rect {
        rect: RectF::new(0.0, 0.0, 10.0, 10.0),
        color: Rgba8::new(0, 0, 0, 255),
    });
    ir.pages[0].ops = Span::new(ir.pages[0].ops.start, start + 1);
    ir
}

#[test]
fn scale_one_is_bit_identical_to_direct_paint() {
    let ir = ir();
    let mut direct = RgbaSurface::new(40, 40);
    direct
        .render_page(&ir, 0, &mut Probe::default())
        .expect("render");
    let mut target = RgbaSurface::new(40, 40);
    target.clear(Rgba8::WHITE);
    {
        let mut scaled = ScaledSurface::new(&mut target, 1.0);
        paint(&ir, 0, &mut scaled, &mut Probe::default()).expect("paint");
    }
    assert_eq!(direct.pixels(), target.pixels());
}

#[test]
fn scale_multiplies_geometry_and_font_size() {
    let ir = ir();

    // 1x：矩形只覆盖 10×10，右下角仍是纸面。
    let mut one = RgbaSurface::new(40, 40);
    one.clear(Rgba8::WHITE);
    {
        let mut scaled = ScaledSurface::new(&mut one, 1.0);
        paint(&ir, 0, &mut scaled, &mut Probe::default()).expect("paint 1x");
    }
    assert_eq!(one.pixel(5, 5), Some(Rgba8::new(0, 0, 0, 255)));
    assert_eq!(one.pixel(30, 30), Some(Rgba8::WHITE));

    // 2x：同一个矩形覆盖到 20×20（设备像素），落点跟着翻倍。
    let mut two = RgbaSurface::new(40, 40);
    two.clear(Rgba8::WHITE);
    let mut probe = Probe::default();
    {
        let mut scaled = ScaledSurface::new(&mut two, 2.0);
        assert_eq!(scaled.scale(), 2.0);
        paint(&ir, 0, &mut scaled, &mut probe).expect("paint 2x");
    }
    assert_eq!(two.pixel(15, 15), Some(Rgba8::new(0, 0, 0, 255)));
    assert_eq!(two.pixel(25, 25), Some(Rgba8::WHITE));

    // 字号必须跟着放大，否则就是“先画小再拉大”。
    let sizes = probe.sizes;
    assert!(!sizes.is_empty(), "layout produced glyph runs");
    for px in sizes {
        assert!((px - 32.0).abs() < 1.0e-3, "px = {px}");
    }
}

#[test]
fn invalid_scale_falls_back_to_one() {
    for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
        let mut surface = RgbaSurface::new(4, 4);
        let scaled = ScaledSurface::new(&mut surface, bad);
        assert_eq!(scaled.scale(), 1.0);
    }
}

/// 缩放后的克隆不能改动原 run：`paint` 之后 IR 仍然逐字节可用。
#[test]
fn scaling_does_not_mutate_the_ir() {
    let ir = ir();
    let before = ir.clone();
    let mut surface = RgbaSurface::new(80, 80);
    {
        let mut scaled = ScaledSurface::new(&mut surface, 3.0);
        paint(&ir, 0, &mut scaled, &mut Probe::default()).expect("paint 3x");
    }
    assert_eq!(ir, before);
    let runs = ir
        .display
        .ops
        .iter()
        .filter(|op| matches!(op, Op::GlyphRun(_)))
        .count();
    assert!(runs > 0);
}
