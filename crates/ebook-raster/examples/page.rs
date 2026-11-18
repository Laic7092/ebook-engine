use std::env;
use std::fs;

use ebook_font::FontBook;
use ebook_ir::Rgba8;
use ebook_format::{open, MemorySource};
use ebook_ir::EdgesF;
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint};
use ebook_raster::RgbaSurface;

fn main() {
    let args: Vec<String> = env::args().collect();
    let book_path = args.get(1).cloned().unwrap_or_else(|| String::from("fixtures/en.epub"));
    let font_path = args.get(2).cloned().unwrap_or_else(|| String::from("fixtures/font/test.ttf"));
    let spine: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
    let page: u32 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);
    let out = args.get(5).cloned().unwrap_or_else(|| String::from("/tmp/page.ppm"));

    let mut src = MemorySource::new();
    let name = book_path.rsplit('/').next().unwrap_or("book.epub");
    src.insert(name, fs::read(&book_path).expect("book"));
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, spine).expect("chapter");

    let mut fonts = FontBook::new();
    fonts.add(fs::read(&font_path).expect("font")).expect("parse font");

    let env = LayoutEnv { measurer: &fonts, resources: &NoImageSizes };
    let constraint = PageConstraint::new(720.0, 960.0).with_margin(EdgesF::all(54.0));
    let ir = layout(&doc, &env, &constraint);

    let page_ref = ir.page(page).expect("page");
    let size = page_ref.box_size();
    let mut surface = RgbaSurface::from_page(page_ref);
    surface.clear(Rgba8::new(0xff, 0xfd, 0xf8, 0xff));
    let mut res = fonts;
    surface.render_page(&ir, page, &mut res).expect("render");
    fs::write(&out, surface.to_ppm()).expect("write");
    let ink = surface.pixels().iter().filter(|p| p.r < 200).count();
    println!(
        "page {page} of spine {spine}: pages={} size={}x{} ink={} pixels -> {out}",
        ir.pages.len(),
        size.w as u32,
        size.h as u32,
        ink
    );
}
