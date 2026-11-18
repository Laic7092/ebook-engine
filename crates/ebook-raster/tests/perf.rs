//! 整页渲染耗时。只打印，不做时间闸门：CI 机器的绝对速度不是契约。

use std::fs;
use std::time::Instant;

use ebook_font::FontBook;
use ebook_ir::{EdgesF, Rgba8, Surface};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint};
use ebook_raster::RgbaSurface;

fn page_ir() -> (ebook_ir::LayoutResult, FontBook) {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let book = fs::read(format!("{root}/fixtures/typography.epub")).expect("book");
    let font = fs::read(format!("{root}/fixtures/font/test.ttf")).expect("font");
    let mut src = ebook_format::MemorySource::new();
    src.insert("typography.epub", book);
    let opened = ebook_format::open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("chapter");
    let mut fonts = FontBook::new();
    fonts.add(font).expect("font");
    let env = LayoutEnv {
        measurer: &fonts,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(480.0, 640.0).with_margin(EdgesF::all(36.0));
    (Layout::new().layout(&doc, &env, &constraint), fonts)
}

#[test]
fn full_page_render_time_is_printed() {
    let (ir, mut fonts) = page_ir();
    assert!(ir.pages.len() > 0);
    let page = ir.page(0).expect("page");
    let mut surface = RgbaSurface::from_page(page);

    // 冷启动一次（字体解析、缓存），再计时。
    surface.render_page(&ir, 0, &mut fonts).expect("render");
    let runs = 10;
    let start = Instant::now();
    for _ in 0..runs {
        surface.render_page(&ir, 0, &mut fonts).expect("render");
    }
    let elapsed = start.elapsed();
    println!(
        "page render: {}x{}, {} runs, {:.2} ms/page",
        surface.width(),
        surface.height(),
        runs,
        elapsed.as_secs_f64() * 1000.0 / runs as f64
    );
    assert!(surface.pixels().iter().any(|pixel| *pixel != Rgba8::WHITE));
}

#[test]
fn full_page_solid_fill_time_is_printed() {
    let mut surface = RgbaSurface::new(1200, 1600);
    let rect = ebook_ir::RectF::new(0.0, 0.0, 1200.0, 1600.0);
    let color = Rgba8::new(0xfa, 0xf4, 0xe8, 255);
    let runs = 50;
    let start = Instant::now();
    for _ in 0..runs {
        surface.rect(rect, color);
    }
    let elapsed = start.elapsed();
    println!(
        "solid fill: {}x{}, {} runs, {:.3} ms/fill",
        surface.width(),
        surface.height(),
        runs,
        elapsed.as_secs_f64() * 1000.0 / runs as f64
    );
    assert_eq!(surface.pixel(0, 0), Some(color));
    assert_eq!(surface.pixel(1199, 1599), Some(color));
}
