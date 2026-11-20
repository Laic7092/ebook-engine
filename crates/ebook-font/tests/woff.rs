//! WOFF / WOFF2 内嵌字体：解包后必须与裸 TTF 走同一条度量、整形与光栅链路。

use std::{fs, path::PathBuf};

use ebook_font::{FontBook, FontLoadError, WoffKind};
use ebook_ir::{FontDesc, FontId};
use ebook_layout::{ShapeGlyph, TextMeasurer};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/font")
        .join(name);
    fs::read(path).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn book(bytes: Vec<u8>) -> (FontBook, FontId) {
    let mut book = FontBook::new();
    let id = book.add(bytes).expect("font id");
    (book, id)
}

fn shape(book: &FontBook, text: &str, id: FontId, size: f32) -> (f32, Vec<ShapeGlyph>) {
    let mut glyphs = Vec::new();
    let advance = book.shape(text, id, size, &mut glyphs);
    (advance, glyphs)
}

fn assert_same_font(ttf: &FontBook, other: &FontBook) {
    let ttf_id = FontId(0);
    let other_id = FontId(0);

    // 合字、kerning、数字与西里尔 `locl` 都覆盖到；这些结果依赖 GSUB/GPOS/GDEF
    // 与 cmap，能验证 WOFF 解包没有丢表或改错 glyph id。
    for text in [
        "office fi fl ffi",
        "AVATAR To Wa",
        "Hello, world! 123",
        "Владимир",
        "Привет, мир",
    ] {
        for size in [12.0, 17.5, 32.0] {
            let expected = shape(ttf, text, ttf_id, size);
            let actual = shape(other, text, other_id, size);
            assert_eq!(actual, expected, "shape mismatch for {text:?} @ {size}px");
        }
    }

    for ch in ['A', 'M', 'i', 'ф', '中'] {
        assert_eq!(
            ttf.covers(ch, ttf_id),
            other.covers(ch, other_id),
            "coverage mismatch for {ch:?}"
        );
        assert_eq!(
            ttf.glyph_id(ch, ttf_id),
            other.glyph_id(ch, other_id),
            "glyph id mismatch for {ch:?}"
        );
        assert_eq!(
            ttf.advance(ch, ttf_id, 18.0),
            other.advance(ch, other_id, 18.0),
            "advance mismatch for {ch:?}"
        );
    }
    assert_eq!(ttf.ascent(ttf_id, 18.0), other.ascent(other_id, 18.0));
    assert_eq!(ttf.descent(ttf_id, 18.0), other.descent(other_id, 18.0));

    let gid = ttf.glyph_id('M', ttf_id);
    assert_eq!(gid, other.glyph_id('M', other_id));
    assert_eq!(
        ttf.rasterize_at(ttf_id, gid, 37.5, 0.25, 0.75),
        other.rasterize_at(other_id, gid, 37.5, 0.25, 0.75),
        "raster mismatch"
    );
}

#[test]
fn woff_shapes_like_the_same_font_as_ttf() {
    let (ttf, _) = book(fixture("test.ttf"));
    let (woff, _) = book(fixture("test.woff"));
    assert_same_font(&ttf, &woff);
}

#[test]
fn woff2_shapes_like_the_same_font_as_ttf() {
    let (ttf, _) = book(fixture("test.ttf"));
    let (woff2, _) = book(fixture("test.woff2"));
    assert_same_font(&ttf, &woff2);
}

#[test]
fn broken_woff_is_diagnosed_instead_of_silently_dropped() {
    let mut book = FontBook::new();
    let error = book
        .try_add_face(FontDesc::default(), b"wOF2broken".to_vec())
        .expect_err("corrupt WOFF2");
    assert_eq!(
        error,
        FontLoadError::Woff {
            kind: WoffKind::Woff2
        }
    );
    assert_eq!(book.diagnostics().len(), 1);
    assert_eq!(book.diagnostics()[0].error, error);

    assert!(book.add(b"not a font".to_vec()).is_none());
    assert_eq!(book.diagnostics().len(), 2);
    assert_eq!(
        book.diagnostics()[1].error,
        FontLoadError::InvalidFont,
        "non-sfnt bytes must report a parse failure"
    );

    let diagnostics = book.take_diagnostics();
    assert_eq!(diagnostics.len(), 2);
    assert!(book.diagnostics().is_empty());
}

#[test]
fn woff_keeps_the_requested_face_description() {
    let mut book = FontBook::new();
    let id = book
        .try_add_styled(
            "Book Serif",
            700,
            ebook_ir::FontStyle::Italic,
            fixture("test.woff"),
        )
        .expect("woff");
    let desc = book.desc(id).expect("desc");
    assert_eq!(desc.family, "Book Serif");
    assert_eq!(desc.weight, 700);
    assert_eq!(desc.style, ebook_ir::FontStyle::Italic);
}
