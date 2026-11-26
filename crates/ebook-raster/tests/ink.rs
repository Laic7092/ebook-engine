//! 主题墨色只替换默认字色：作者显式写的链接色/强调色必须保留。

use ebook_ir::{
    Dir, FontId, FontStyle, Glyph, GlyphBitmap, GlyphRun, ImageData, ImageId, PointF, Resources,
    Rgba8, Rotate, Span, Surface,
};
use ebook_raster::RgbaSurface;

const THEME_INK: Rgba8 = Rgba8::new(0xd8, 0xd6, 0xcf, 255);
const AUTHOR_RED: Rgba8 = Rgba8::new(0xcc, 0x00, 0x00, 255);

struct Solid;

impl Resources for Solid {
    fn glyph_alpha(
        &mut self,
        _font: FontId,
        _gid: u16,
        _px: f32,
    ) -> Option<std::sync::Arc<GlyphBitmap>> {
        Some(std::sync::Arc::new(GlyphBitmap {
            w: 8,
            h: 8,
            left: 0,
            top: -8,
            alpha: vec![255; 64],
        }))
    }

    fn image(
        &mut self,
        _id: ImageId,
        _target: ebook_ir::SizeF,
    ) -> Option<std::sync::Arc<ImageData>> {
        None
    }
}

fn draw(surface: &mut RgbaSurface, color: Rgba8, x: f32) {
    let run = GlyphRun {
        font: FontId(0),
        size: 8.0,
        color,
        origin: PointF::new(x, 8.0),
        glyphs: Span::new(0, 1),
        text: "A".into(),
        dir: Dir::Ltr,
        rotate: Rotate::None,
        weight: 400,
        style: FontStyle::Normal,
        underline: false,
    };
    let glyphs = [Glyph {
        gid: 0,
        dx: 8.0,
        dy: 0.0,
        ch: 0,
    }];
    surface.glyph_run(&run, &glyphs, &mut Solid);
}

#[test]
fn ink_replacing_keeps_author_colors() {
    let mut surface = RgbaSurface::new(20, 8);
    surface.set_ink_replacing(Some(THEME_INK), Rgba8::BLACK);
    draw(&mut surface, Rgba8::BLACK, 0.0);
    draw(&mut surface, AUTHOR_RED, 10.0);

    assert_eq!(surface.pixel(4, 4), Some(THEME_INK), "默认黑字该换成主题墨");
    assert_eq!(
        surface.pixel(14, 4),
        Some(AUTHOR_RED),
        "作者显式颜色不能被主题墨盖掉"
    );
}

#[test]
fn set_ink_still_replaces_every_color() {
    let mut surface = RgbaSurface::new(20, 8);
    surface.set_ink(Some(THEME_INK));
    draw(&mut surface, Rgba8::BLACK, 0.0);
    draw(&mut surface, AUTHOR_RED, 10.0);

    assert_eq!(surface.pixel(4, 4), Some(THEME_INK));
    assert_eq!(surface.pixel(14, 4), Some(THEME_INK));
}
