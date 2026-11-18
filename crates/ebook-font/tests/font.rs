use std::{fs, path::PathBuf};

use ebook_font::FontBook;
use ebook_ir::{FontId, Resources};
use ebook_layout::TextMeasurer;

fn fixture() -> (FontBook, FontId) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/font/test.ttf");
    let bytes = fs::read(path).expect("test font");
    let mut book = FontBook::new();
    let id = book.add(bytes).expect("font id");
    (book, id)
}

#[test]
fn cmap_is_real() {
    let (book, id) = fixture();
    assert!(book.covers('A', id));
    assert_ne!(book.glyph_id('A', id), 0x41);
}

#[test]
fn m_is_wider_than_i() {
    let (book, id) = fixture();
    assert!(book.advance('M', id, 32.0) > book.advance('i', id, 32.0));
}

#[test]
fn missing_cjk() {
    let (book, id) = fixture();
    assert!(!book.covers('中', id));
    assert_eq!(book.glyph_id('中', id), 0);
    assert_eq!(book.advance('中', id, 32.0), 0.0);
}

#[test]
fn metrics_follow_units_per_em() {
    let (book, id) = fixture();
    assert!(book.ascent(id, 32.0) > 0.0);
    assert!(book.descent(id, 32.0) > 0.0);
}

#[test]
fn raster_has_coverage() {
    let (book, id) = fixture();
    let gid = book.glyph_id('A', id);
    let bitmap = book.rasterize(id, gid, 48.0).expect("bitmap");
    assert!(bitmap.w > 0);
    assert!(bitmap.h > 0);
    assert_eq!(bitmap.alpha.len(), bitmap.w as usize * bitmap.h as usize);
    assert!(bitmap.alpha.iter().any(|&a| a > 0));
}

#[test]
fn raster_is_deterministic() {
    let (book, id) = fixture();
    let gid = book.glyph_id('M', id);
    assert_eq!(book.rasterize(id, gid, 37.5), book.rasterize(id, gid, 37.5));
}

#[test]
fn resources_uses_rasterizer() {
    let (mut book, id) = fixture();
    let gid = book.glyph_id('i', id);
    assert!(book.glyph_alpha(id, gid, 20.0).is_some());
    assert!(book.image(0).is_none());
}
