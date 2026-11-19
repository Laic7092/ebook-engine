//! 内嵌字体装载：family/weight/style 与 `@font-face` 一致，断字词典在字体路径上注入。

use std::path::PathBuf;

use ebook_font::FontBook;
use ebook_ir::{FontDesc, FontSpec, FontStyle};
#[cfg(feature = "hyphen-en")]
use ebook_layout::{Hyphenator, TextMeasurer};

fn bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/font/test.ttf");
    std::fs::read(path).expect("test font")
}

#[test]
fn styled_face_wins_over_the_default_face() {
    let mut book = FontBook::new();
    // fallback 面：没有 family。
    book.add(bytes()).expect("fallback");
    let id = book
        .add_styled("Book Serif", 700, FontStyle::Italic, bytes())
        .expect("embedded");

    let spec = FontSpec::new(18.0, 700, FontStyle::Italic).with_family("Book Serif");
    let chosen = book.select(&spec);
    assert_eq!(
        chosen, id,
        "family/weight/style must select the embedded face"
    );
    let desc: &FontDesc = book.desc(chosen).expect("desc");
    assert_eq!(desc.family, "Book Serif");
    assert_eq!(desc.weight, 700);
    assert_eq!(desc.style, FontStyle::Italic);
}

#[cfg(feature = "hyphen-en")]
#[test]
fn language_is_injected_and_hyphenator_is_available() {
    let mut book = FontBook::new();
    book.add_styled("Book Serif", 400, FontStyle::Normal, bytes())
        .expect("font");
    book.set_language(Some("en-GB"));
    assert_eq!(book.language(), Some("en-gb"));

    let hyphenator: &dyn Hyphenator = book.hyphenator().expect("english hyphenator");
    assert_eq!(hyphenator.name(), "en-us");
    let mut points = Vec::new();
    hyphenator.breaks("hyphenation", &mut points);
    assert!(points.contains(&2), "hy-phen-ation: {points:?}");
}
