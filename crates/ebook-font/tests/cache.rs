//! 字体层缓存：命中与未命中必须给出逐字节相同的光栅/整形结果，且失效点要正确。

use std::{fs, path::PathBuf};

use ebook_font::FontBook;
use ebook_ir::FontId;
use ebook_layout::{ShapeGlyph, TextMeasurer};

fn bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/font/test.ttf");
    fs::read(path).expect("test font")
}

fn fixture() -> (FontBook, FontId) {
    let mut book = FontBook::new();
    let id = book.add(bytes()).expect("font id");
    (book, id)
}

fn shape(book: &FontBook, text: &str, id: FontId, size: f32) -> (Vec<ShapeGlyph>, f32) {
    let mut glyphs = Vec::new();
    let advance = book.shape(text, id, size, &mut glyphs);
    (glyphs, advance)
}

#[test]
fn font_book_stays_send() {
    fn assert_send<T: Send>() {}
    assert_send::<FontBook>();
}

#[test]
fn repeated_raster_with_offset_is_identical() {
    let (book, id) = fixture();
    let gid = book.glyph_id('A', id);
    let first = book
        .rasterize_at(id, gid, 17.5, 0.25, 0.75)
        .expect("bitmap");
    let second = book
        .rasterize_at(id, gid, 17.5, 0.25, 0.75)
        .expect("bitmap");
    assert_eq!(first, second);

    // 失败结果（这里是不存在的高位 gid）也缓存，重复询问仍然是 `None`。
    let missing = book.rasterize_at(id, u16::MAX, 17.5, 0.25, 0.75);
    assert!(missing.is_none());
    assert_eq!(missing, book.rasterize_at(id, u16::MAX, 17.5, 0.25, 0.75));
}

#[test]
fn raster_key_separates_size_and_phase() {
    let (book, id) = fixture();
    let gid = book.glyph_id('M', id);

    let at_12 = book.rasterize_at(id, gid, 12.0, 0.0, 0.0).expect("12px");
    let at_20 = book.rasterize_at(id, gid, 20.0, 0.0, 0.0).expect("20px");
    let back_to_12 = book
        .rasterize_at(id, gid, 12.0, 0.0, 0.0)
        .expect("12px again");
    assert_eq!(at_12, back_to_12, "回到同一 px 必须命中同一份位图");
    assert_ne!(at_12, at_20, "不同 px 不能共用缓存条目");

    let shifted_x = book
        .rasterize_at(id, gid, 12.0, 0.5, 0.0)
        .expect("dx bitmap");
    let shifted_y = book
        .rasterize_at(id, gid, 12.0, 0.0, 0.5)
        .expect("dy bitmap");
    assert_ne!(at_12, shifted_x, "dx 必须进键");
    assert_ne!(at_12, shifted_y, "dy 必须进键");
    assert_eq!(
        shifted_x,
        book.rasterize_at(id, gid, 12.0, 0.5, 0.0)
            .expect("dx bitmap again")
    );
}

#[test]
fn repeated_shape_is_identical() {
    let (book, id) = fixture();
    let (first, first_advance) = shape(&book, "office AV fi", id, 21.5);
    let (second, second_advance) = shape(&book, "office AV fi", id, 21.5);
    assert_eq!(first, second, "同一段文本的字形序列必须完全一致");
    assert_eq!(first_advance, second_advance, "advance 必须一致");

    // 不同字号仍然是不同条目，且能回到第一次的结果。
    let (large, _) = shape(&book, "office AV fi", id, 32.0);
    let (back, back_advance) = shape(&book, "office AV fi", id, 21.5);
    assert_ne!(first, large);
    assert_eq!(first, back);
    assert_eq!(first_advance, back_advance);
}

#[test]
fn set_language_does_not_return_the_old_shape() {
    let mut book = FontBook::new();
    let id = book.add(bytes()).expect("font id");
    // test.ttf 的 cyrl/`SRB` 有 `locl` 替换：塞尔维亚语的 `б` 是不同字形。
    let text = "бгдпт";
    let (default, _) = shape(&book, text, id, 32.0);

    book.set_language(Some("sr"));
    let (serbian, serbian_advance) = shape(&book, text, id, 32.0);
    assert_ne!(default, serbian, "换语言后不能命中旧条目");

    let mut fresh = FontBook::new();
    fresh.add(bytes()).expect("font id");
    fresh.set_language(Some("sr"));
    let (expected, expected_advance) = shape(&fresh, text, id, 32.0);
    assert_eq!(serbian, expected, "复用 FontBook 的结果必须与新建的一致");
    assert_eq!(serbian_advance, expected_advance);
}

#[test]
fn adding_a_face_keeps_old_font_id_results() {
    let (mut book, id) = fixture();
    let gid = book.glyph_id('M', id);
    let bitmap = book
        .rasterize_at(id, gid, 24.0, 0.125, 0.375)
        .expect("bitmap");
    let (shaped, advance) = shape(&book, "AV office", id, 24.0);

    let second = book.add(bytes()).expect("second font id");
    assert_ne!(id, second, "新 face 不能复用旧下标");

    assert_eq!(
        book.rasterize_at(id, gid, 24.0, 0.125, 0.375),
        Some(bitmap),
        "旧 FontId 的光栅结果不能被新 face 改变"
    );
    let (again, again_advance) = shape(&book, "AV office", id, 24.0);
    assert_eq!(again, shaped);
    assert_eq!(again_advance, advance);
}
