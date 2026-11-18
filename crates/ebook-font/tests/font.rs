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

fn bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/font/test.ttf");
    fs::read(path).expect("test font")
}

fn desc(family: &str, weight: u16, style: ebook_ir::FontStyle) -> ebook_ir::FontDesc {
    ebook_ir::FontDesc {
        family: family.into(),
        weight,
        style,
    }
}

fn spec(family: &str, weight: u16, style: ebook_ir::FontStyle) -> ebook_ir::FontSpec {
    ebook_ir::FontSpec {
        weight,
        style,
        families: ebook_ir::FontFamilies::from_slice(&[family]),
        ..Default::default()
    }
}

#[test]
fn weight_selection_is_nearest() {
    let mut book = FontBook::new();
    let light = book
        .add_face(desc("Test", 300, ebook_ir::FontStyle::Normal), bytes())
        .expect("light");
    let medium = book
        .add_face(desc("Test", 500, ebook_ir::FontStyle::Normal), bytes())
        .expect("medium");
    let heavy = book
        .add_face(desc("Test", 900, ebook_ir::FontStyle::Normal), bytes())
        .expect("heavy");
    assert_eq!(
        book.base_font(book.select(&spec("Test", 600, ebook_ir::FontStyle::Normal))),
        medium
    );
    assert_eq!(
        book.base_font(book.select(&spec("Test", 280, ebook_ir::FontStyle::Normal))),
        light
    );
    assert_eq!(
        book.base_font(book.select(&spec("Test", 880, ebook_ir::FontStyle::Normal))),
        heavy
    );
}

#[test]
fn style_wins_over_weight_distance() {
    let mut book = FontBook::new();
    let normal = book
        .add_face(desc("Test", 400, ebook_ir::FontStyle::Normal), bytes())
        .expect("normal");
    let italic = book
        .add_face(desc("Test", 900, ebook_ir::FontStyle::Italic), bytes())
        .expect("italic");
    let chosen = book.select(&spec("Test", 400, ebook_ir::FontStyle::Italic));
    assert_eq!(chosen, italic);
    assert_ne!(chosen, normal);
    assert_eq!(
        book.desc(chosen).map(|d| d.style),
        Some(ebook_ir::FontStyle::Italic)
    );
}

#[test]
fn stack_order_uses_first_covering_face() {
    let mut book = FontBook::new();
    let first = book
        .add_face(desc("First", 400, ebook_ir::FontStyle::Normal), bytes())
        .expect("first");
    let second = book
        .add_face(desc("Second", 700, ebook_ir::FontStyle::Normal), bytes())
        .expect("second");
    let stack = ebook_ir::FontSpec {
        weight: 700,
        families: ebook_ir::FontFamilies::from_slice(&["Missing", "First", "Second"]),
        ..Default::default()
    };
    let chosen = book.select_char(&stack, 'A');
    assert!(book.covers('A', chosen));
    assert_eq!(book.base_font(chosen), first);
    assert_ne!(book.base_font(chosen), second);
    assert_eq!(book.desc(chosen).map(|d| d.family.as_str()), Some("First"));
}

#[test]
fn synthetic_bold_is_heavier() {
    let mut book = FontBook::new();
    let regular = book
        .add_face(desc("Test", 400, ebook_ir::FontStyle::Normal), bytes())
        .expect("regular");
    let chosen = book.select(&spec("Test", 700, ebook_ir::FontStyle::Normal));
    assert!(book.is_synthetic_bold(chosen));
    assert!(book.is_synthetic(chosen));
    let gid = book.glyph_id('A', regular);
    let plain = book.rasterize(regular, gid, 48.0).expect("plain");
    let bold = book.rasterize(chosen, gid, 48.0).expect("bold");
    let plain_ink: u64 = plain.alpha.iter().map(|&a| a as u64).sum();
    let bold_ink: u64 = bold.alpha.iter().map(|&a| a as u64).sum();
    assert!(bold_ink > plain_ink);
    assert!(bold.w >= plain.w);
}

#[test]
fn synthetic_italic_shears_the_outline() {
    let mut book = FontBook::new();
    let regular = book
        .add_face(desc("Test", 400, ebook_ir::FontStyle::Normal), bytes())
        .expect("regular");
    let chosen = book.select(&spec("Test", 400, ebook_ir::FontStyle::Italic));
    assert!(book.is_synthetic_italic(chosen));
    let gid = book.glyph_id('l', regular);
    let plain = book.rasterize(regular, gid, 64.0).expect("plain");
    let italic = book.rasterize(chosen, gid, 64.0).expect("italic");
    let top = centroid(&italic, 0, italic.h as usize / 3).expect("top ink");
    let bottom =
        centroid(&italic, italic.h as usize * 2 / 3, italic.h as usize).expect("bottom ink");
    assert!(top > bottom + 0.5);
    assert!(italic.w >= plain.w);
}

#[test]
fn synthesis_keeps_advance_and_metrics() {
    let mut book = FontBook::new();
    let regular = book
        .add_face(desc("Test", 400, ebook_ir::FontStyle::Normal), bytes())
        .expect("regular");
    let bold = book.select(&spec("Test", 700, ebook_ir::FontStyle::Normal));
    let italic = book.select(&spec("Test", 400, ebook_ir::FontStyle::Italic));
    assert_eq!(
        book.advance('M', regular, 32.0),
        book.advance('M', bold, 32.0)
    );
    assert_eq!(
        book.advance('M', regular, 32.0),
        book.advance('M', italic, 32.0)
    );
    assert_eq!(book.ascent(regular, 32.0), book.ascent(bold, 32.0));
    assert_eq!(book.descent(regular, 32.0), book.descent(italic, 32.0));
    assert_eq!(book.glyph_id('M', regular), book.glyph_id('M', bold));
    assert!(book.covers('M', bold));
}

fn centroid(bitmap: &ebook_ir::GlyphBitmap, start: usize, end: usize) -> Option<f32> {
    let w = bitmap.w as usize;
    let h = bitmap.h as usize;
    let mut weight = 0u64;
    let mut sum = 0.0f64;
    for y in start..end.min(h) {
        for x in 0..w {
            let alpha = bitmap.alpha[y * w + x] as u64;
            if alpha == 0 {
                continue;
            }
            weight += alpha;
            sum += (bitmap.left as f64 + x as f64 + 0.5) * alpha as f64;
        }
    }
    if weight == 0 {
        None
    } else {
        Some((sum / weight as f64) as f32)
    }
}

#[test]
fn ligature_merges_glyphs() {
    let (book, id) = fixture();
    let mut out = Vec::new();
    book.shape("fi", id, 32.0, &mut out);
    // test.ttf 有 fi 合字：两个字符合成一个字形。
    assert_eq!(out.len(), 1, "fi should be one ligature glyph");

    let mut ffi = Vec::new();
    let total = book.shape("ffi", id, 32.0, &mut ffi);
    let naive = book.advance('f', id, 32.0) * 2.0 + book.advance('i', id, 32.0);
    assert_eq!(ffi.len(), 1, "ffi should be one ligature glyph");
    assert!(
        total < naive - 0.01,
        "ffi {total} should be narrower than {naive}"
    );
}

#[test]
fn kerning_tightens_pairs() {
    let (book, id) = fixture();
    let mut out = Vec::new();
    let total = book.shape("AV", id, 32.0, &mut out);
    let naive = book.advance('A', id, 32.0) + book.advance('V', id, 32.0);
    assert!(total < naive - 0.01, "AV {total} should kern below {naive}");
}
