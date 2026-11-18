//! 坏输入与畸形容器的回归：不 panic、不无限循环，结论只能是 `Err` 或带 issue 的
//! 部分结果。

mod support;

use ebook_format::epub::EpubReader;
use ebook_format::{open, ByteSource};

fn minimal_opf(spine: &str, manifest: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Robust</dc:title><dc:language>en</dc:language><dc:identifier id="id">r</dc:identifier>
  </metadata>
  <manifest>{manifest}</manifest>
  <spine>{spine}</spine>
</package>"##
    )
}

fn open_bytes(bytes: Vec<u8>) -> Result<ebook_format::Opened, ebook_format::FormatError> {
    let src = support::source(bytes);
    open(&src)
}

#[test]
fn truncated_archive_is_an_error() {
    let bytes = support::epub(
        "OEBPS/content.opf",
        &minimal_opf(
            r#"<itemref idref="c1"/>"#,
            r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
        ),
        &[(
            "OEBPS/ch1.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#,
        )],
    );
    for cut in [1, 8, bytes.len() / 3, bytes.len() - 1] {
        let mut broken = bytes.clone();
        broken.truncate(cut);
        let src = support::source(broken);
        assert!(open(&src).is_err(), "cut at {cut} should not open");
    }
}

#[test]
fn missing_container_is_an_error_not_an_empty_book() {
    let bytes = support::stored_zip(&[("mimetype", b"application/epub+zip".to_vec())]);
    assert!(open_bytes(bytes).is_err());
}

#[test]
fn empty_zip_is_an_error() {
    assert!(open_bytes(support::stored_zip(&[])).is_err());
}

#[test]
fn opf_without_spine_is_an_error() {
    let bytes = support::epub(
        "content.opf",
        &minimal_opf(
            "",
            r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
        ),
        &[],
    );
    assert!(open_bytes(bytes).is_err());
}

#[test]
fn opf_with_dangling_spine_item_keeps_the_rest_and_reports() {
    let opf = minimal_opf(
        r#"<itemref idref="ghost"/><itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let bytes = support::epub(
        "OEBPS/content.opf",
        &opf,
        &[(
            "OEBPS/ch1.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#,
        )],
    );
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    assert_eq!(opened.len(), 1);
    assert!(opened
        .book
        .issues()
        .iter()
        .any(|issue| issue.code == ebook_format::EpubIssueCode::MissingSpineItem));
}

#[test]
fn unclosed_xhtml_does_not_take_down_the_book() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>never closed"#;
    let bytes = support::epub("OEBPS/content.opf", &opf, &[("OEBPS/ch1.xhtml", ch1)]);
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    assert!(opened.chapter(&src, 0).is_err());
}

#[test]
fn huge_unterminated_entity_run_stays_linear() {
    let mut body = String::from("&");
    body.push_str(&"&".repeat(200_000));
    body.push_str("tail");
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let ch1 =
        format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>{body}</p></body></html>"#);
    let bytes = support::epub(
        "OEBPS/content.opf",
        &opf,
        &[("OEBPS/ch1.xhtml", ch1.as_bytes())],
    );
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("chapter");
    assert!(doc.text.contains("tail"));
}

#[test]
fn cyclic_navigation_and_css_imports_terminate() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
           <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
           <item id="css" href="style.css" media-type="text/css"/>"#,
    );
    let nav = r##"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body><nav epub:type="toc"><ol>
<li><a href="nav.xhtml">itself</a><ol><li><a href="ch1.xhtml">chapter</a></li></ol></li>
</ol></nav></body></html>"##;
    let ch1 = r##"<html xmlns="http://www.w3.org/1999/xhtml"><head>
<link rel="stylesheet" href="style.css"/></head><body><p>x</p></body></html>"##;
    let css = "@import url(style.css);\n@import url(other.css);\np { color: red; }\n";
    let bytes = support::epub(
        "OEBPS/content.opf",
        &opf,
        &[
            ("OEBPS/nav.xhtml", nav.as_bytes()),
            ("OEBPS/ch1.xhtml", ch1.as_bytes()),
            ("OEBPS/style.css", css.as_bytes()),
        ],
    );
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("chapter");
    assert!(!doc.text.is_empty());
}

#[test]
fn zip_path_traversal_entries_are_never_reachable() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let bytes = support::epub(
        "OEBPS/content.opf",
        &opf,
        &[
            (
                "OEBPS/ch1.xhtml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#,
            ),
            ("../escape.xhtml", b"outside"),
            ("/absolute.css", b"body { color: red }"),
            ("OEBPS/../../escape2.xhtml", b"outside"),
        ],
    );
    let src = support::source(bytes);
    let raw = src.document().expect("document");
    let reader = EpubReader::open(&raw).expect("reader");

    assert!(reader.asset("../escape.xhtml").is_none());
    assert!(reader.asset("../../escape2.xhtml").is_none());
    assert!(reader
        .issues()
        .iter()
        .any(|issue| issue.code == ebook_format::EpubIssueCode::UnsafePath));
    // 合法的条目照常可读。
    assert!(reader.book().asset("OEBPS/ch1.xhtml").is_some());
}
