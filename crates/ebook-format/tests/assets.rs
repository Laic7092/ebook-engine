//! 图片与内嵌字体：manifest 资产表、字节读取、`@font-face` 的继承。

mod support;

use ebook_format::epub::EpubReader;
use ebook_format::{open, ByteSource};

#[test]
fn embedded_fonts_are_read_from_the_zip() {
    let src = support::source(support::sample_epub3());
    let opened = open(&src).expect("open");
    let fonts = opened.book.fonts();
    assert_eq!(fonts.len(), 1, "fonts: {fonts:#?}");
    let font = &fonts[0];
    assert_eq!(font.family, "Book Serif");
    assert_eq!(font.weight, 700);
    assert_eq!(font.style, ebook_ir::FontStyle::Italic);
    // 相对路径按声明它的 CSS 文件解析。
    assert_eq!(font.href, "OEBPS/font/book.ttf");
    assert_eq!(font.bytes, b"FONTBYTES");
}

#[test]
fn assets_expose_bytes_and_media_type() {
    let src = support::source(support::sample_epub3());
    let raw = src.document().expect("document");
    let reader = EpubReader::open(&raw).expect("reader");

    // 相对包根目录的路径与大小写不敏感路径都能找到同一份字节。
    let (bytes, mime) = reader.asset("img/pic.png").expect("asset by relative path");
    assert_eq!(bytes, b"\x89PNG pic");
    assert_eq!(mime, "image/png");

    let (bytes, _) = reader.asset("OEBPS/IMG/PIC.PNG").expect("case-insensitive");
    assert_eq!(bytes, b"\x89PNG pic");

    assert!(reader.asset("missing.png").is_none());
    assert!(reader.asset("https://example.com/pic.png").is_none());

    // manifest 图片按出现顺序列出，但它不是章内 `ImageId` 表：`ImageId` 由
    // 每章的 `StyledDoc::images` 定义，两边都按 `src`/`href` 对字节。
    let images = &reader.book().images;
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].href, "OEBPS/img/pic.png");
    assert_eq!(images[1].href, "OEBPS/img/cover.png");
    assert_eq!(
        reader
            .book()
            .image("OEBPS/img/cover.png")
            .map(|image| image.href.as_str()),
        Some("OEBPS/img/cover.png")
    );
    assert_eq!(
        reader
            .book()
            .asset("OEBPS/font/book.ttf")
            .map(|asset| asset.media_type.as_str()),
        Some("font/ttf")
    );
}

#[test]
fn font_face_collection_ignores_print_only_blocks() {
    let src = support::source(support::sample_epub3());
    let raw = src.document().expect("document");
    let reader = EpubReader::open(&raw).expect("reader");
    let faces = reader.font_faces("OEBPS/style.css");
    assert_eq!(faces.len(), 1);
    assert_eq!(faces[0].family, "Book Serif");
    assert_eq!(faces[0].src.as_deref(), Some("font/book.ttf"));

    let css = r#"@media print { @font-face { font-family: "Print Serif"; src: url(p.ttf); } }
@font-face { font-family: "Screen Serif"; src: url('s.ttf') format("truetype"); }"#;
    let faces = ebook_format::css::parse_font_faces(css);
    assert_eq!(faces.len(), 1);
    assert_eq!(faces[0].family, "Screen Serif");
    assert_eq!(faces[0].src.as_deref(), Some("s.ttf"));
}
