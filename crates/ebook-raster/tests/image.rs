//! 图片采样：三种 `Fit` 都是同一个双线性映射，透明像素不污染背景。

use ebook_ir::{
    EdgesF, Fit, FontId, GlyphBitmap, ImageData, ImageId, LayoutResult, Op, RectF, Resources,
    Rgba8, Span, StyledDoc,
};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
use ebook_raster::RgbaSurface;

struct Images(ImageData);

impl Resources for Images {
    fn glyph_alpha(&mut self, _font: FontId, _gid: u16, _px: f32) -> Option<GlyphBitmap> {
        None
    }

    fn image(&mut self, _id: ImageId) -> Option<ImageData> {
        Some(self.0.clone())
    }
}

fn ir_with(image: Op) -> LayoutResult {
    let html = "<html><body><p>A</p></body></html>";
    let doc: StyledDoc =
        ebook_format::xhtml::build(html, 0, &mut ebook_format::xhtml::NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(120.0, 120.0).with_margin(EdgesF::all(0.0));
    let mut ir = layout(&doc, &env, &constraint);
    ir.display.ops.clear();
    ir.display.glyphs.clear();
    ir.runs.lines.clear();
    ir.pages.truncate(1);
    ir.pages[0].ops = Span::EMPTY;
    ir.pages[0].lines = Span::EMPTY;
    ir.display.ops.push(image);
    ir.pages[0].ops = Span::new(0, 1);
    ir
}

fn render(image: Op, data: ImageData) -> RgbaSurface {
    let ir = ir_with(image);
    let mut surface = RgbaSurface::new(120, 120);
    let mut res = Images(data);
    surface.render_page(&ir, 0, &mut res).expect("render");
    surface
}

#[test]
fn one_by_one_fills_the_whole_rect() {
    let green = Rgba8::new(0, 200, 0, 255);
    let surface = render(
        Op::Image {
            id: 0,
            rect: RectF::new(10.0, 10.0, 40.0, 20.0),
            fit: Fit::Fill,
        },
        ImageData {
            w: 1,
            h: 1,
            rgba: vec![0, 200, 0, 255],
        },
    );
    for y in 10..30 {
        for x in 10..50 {
            assert_eq!(surface.pixel(x, y), Some(green), "pixel {x},{y}");
        }
    }
    assert_eq!(surface.pixel(9, 10), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(50, 10), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(10, 9), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(10, 30), Some(Rgba8::WHITE));
}

#[test]
fn two_by_two_is_bilinear_not_nearest() {
    // 左上红、右上绿、左下蓝、右下白。
    let data = ImageData {
        w: 2,
        h: 2,
        rgba: vec![
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ],
    };
    let surface = render(
        Op::Image {
            id: 0,
            rect: RectF::new(0.0, 0.0, 8.0, 8.0),
            fit: Fit::Fill,
        },
        data,
    );
    // 目标像素中心到 texel 中心的距离决定权重；(4,0) 是红绿 3:5 混合。
    assert_eq!(surface.pixel(0, 0), Some(Rgba8::new(255, 0, 0, 255)));
    assert_eq!(surface.pixel(4, 0), Some(Rgba8::new(96, 159, 0, 255)));
    assert_eq!(surface.pixel(0, 4), Some(Rgba8::new(96, 0, 159, 255)));
    // 四角混合：红、绿、蓝、白各占一块，不允许出现最近邻的整块纯色。
    let corner = surface.pixel(4, 4).expect("corner");
    assert!(
        corner.r > 100 && corner.g > 100 && corner.b > 100,
        "{corner:?}"
    );
}

#[test]
fn contain_keeps_aspect_and_centres() {
    // 4x1 的条带放进 100x20：Contain 后是 80x20，左右各留 10。
    let data = ImageData {
        w: 4,
        h: 1,
        rgba: vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    };
    let surface = render(
        Op::Image {
            id: 0,
            rect: RectF::new(0.0, 0.0, 100.0, 20.0),
            fit: Fit::Contain,
        },
        data,
    );
    assert_eq!(surface.pixel(9, 10), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(10, 10), Some(Rgba8::new(255, 0, 0, 255)));
    assert_eq!(surface.pixel(89, 10), Some(Rgba8::new(255, 255, 255, 255)));
    assert_eq!(surface.pixel(90, 10), Some(Rgba8::WHITE));
}

#[test]
fn cover_crops_instead_of_leaving_bars() {
    // 4x1 放进 20x20：Cover 放大到 80x20，左右各裁掉 30，整块都有墨。
    let data = ImageData {
        w: 4,
        h: 1,
        rgba: vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    };
    let surface = render(
        Op::Image {
            id: 0,
            rect: RectF::new(0.0, 0.0, 20.0, 20.0),
            fit: Fit::Cover,
        },
        data,
    );
    for y in 0..20 {
        for x in 0..20 {
            assert_ne!(surface.pixel(x, y), Some(Rgba8::WHITE), "pixel {x},{y}");
        }
    }
}

#[test]
fn transparent_texels_do_not_darken_the_background() {
    // 左边的 texel 是“透明红”：alpha 为 0 时它的 RGB 不得参与插值，
    // 否则边缘会发黑/发红。红通道必须始终等于绿通道与蓝通道中的背景部分。
    let data = ImageData {
        w: 2,
        h: 1,
        rgba: vec![255, 0, 0, 0, 0, 0, 255, 255],
    };
    let surface = render(
        Op::Image {
            id: 0,
            rect: RectF::new(0.0, 0.0, 8.0, 1.0),
            fit: Fit::Fill,
        },
        data,
    );
    for x in 0..8 {
        let pixel = surface.pixel(x, 0).expect("pixel");
        assert_eq!(
            pixel.r, pixel.g,
            "pixel {x} polluted by transparent red: {pixel:?}"
        );
    }
    assert_eq!(surface.pixel(0, 0), Some(Rgba8::WHITE));
}
