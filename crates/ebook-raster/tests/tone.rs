//! 墨色/伽马开关：默认关闭保持既有 sRGB 行为，打开后走查表、整数运算。

use ebook_ir::{EdgesF, Op, RectF, Rgba8, Span, StyledDoc};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
use ebook_raster::{RgbaSurface, Tone};

fn rect_ir(rect: RectF, color: Rgba8) -> ebook_ir::LayoutResult {
    let html = "<html><body><p>A</p></body></html>";
    let doc: StyledDoc =
        ebook_format::xhtml::build(html, 0, &mut ebook_format::xhtml::NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(40.0, 40.0).with_margin(EdgesF::all(0.0));
    let mut ir = layout(&doc, &env, &constraint);
    ir.display.ops.clear();
    ir.display.glyphs.clear();
    ir.runs.lines.clear();
    ir.pages.truncate(1);
    ir.pages[0].ops = Span::EMPTY;
    ir.pages[0].lines = Span::EMPTY;
    ir.display.ops.push(Op::Rect { rect, color });
    ir.pages[0].ops = Span::new(0, 1);
    ir
}

fn render_rect(tone: Option<Tone>) -> RgbaSurface {
    let ir = rect_ir(RectF::new(0.0, 0.0, 10.0, 10.0), Rgba8::new(0, 0, 0, 128));
    let mut surface = RgbaSurface::new(10, 10).with_tone(tone);
    let mut res = ebook_ir::NoResources;
    surface.render_page(&ir, 0, &mut res).expect("render");
    surface
}

#[test]
fn tone_is_off_by_default() {
    assert_eq!(RgbaSurface::new(4, 4).tone(), None);
    let surface = render_rect(None);
    assert_eq!(surface.pixel(5, 5), Some(Rgba8::new(127, 127, 127, 255)));
}

#[test]
fn dark_ink_raises_edge_coverage() {
    let tone = Tone::ink(Rgba8::BLACK);
    assert_eq!(tone.coverage(0), 0);
    assert_eq!(tone.coverage(255), 255);
    assert!(tone.coverage(128) > 128, "edge should gain weight");
    // 单调：覆盖率不能被修反。
    for a in 1..256u16 {
        assert!(tone.coverage(a as u8) >= tone.coverage(a as u8 - 1));
    }
}

#[test]
fn pale_ink_gets_no_contrast() {
    let tone = Tone::ink(Rgba8::WHITE);
    for a in 0..=255u8 {
        assert_eq!(tone.coverage(a), a);
    }
}

#[test]
fn grow_lifts_half_coverage() {
    let plain = Tone::ink(Rgba8::BLACK).coverage(128);
    let grown = Tone::ink_with_grow(Rgba8::BLACK, 0.5).coverage(128);
    assert!(grown > plain);
}

#[test]
fn linear_gamma_blend_is_lighter_and_deterministic() {
    let srgb = render_rect(None);
    let linear = render_rect(Some(Tone::ink(Rgba8::BLACK).linear()));
    let a = srgb.pixel(5, 5).expect("pixel");
    let b = linear.pixel(5, 5).expect("pixel");
    assert_eq!(a, Rgba8::new(127, 127, 127, 255));
    // 线性光里 50% 黑对应 sRGB 约 188；偏暗的 sRGB 直接插值得到 127。
    assert!(b.r > 180 && b.r < 195, "{b:?}");
    assert_eq!(b.r, b.g);
    assert_eq!(b.g, b.b);
    assert_eq!(b.a, 255);

    let again = render_rect(Some(Tone::ink(Rgba8::BLACK).linear()));
    assert_eq!(linear.to_ppm(), again.to_ppm());
}

#[test]
fn tone_can_be_cleared() {
    let mut surface = RgbaSurface::new(4, 4).with_tone(Some(Tone::ink(Rgba8::BLACK)));
    surface.set_tone(None);
    assert_eq!(surface.tone(), None);
}
