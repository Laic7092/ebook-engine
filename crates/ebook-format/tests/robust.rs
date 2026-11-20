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
fn unclosed_xhtml_is_repaired_not_fatal() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>never closed"#;
    let bytes = support::epub("OEBPS/content.opf", &opf, &[("OEBPS/ch1.xhtml", ch1)]);
    let src = support::source(bytes);
    let raw = src.document().expect("document");
    let reader = EpubReader::open(&raw).expect("open");
    let doc = reader.chapter(0).expect("repaired chapter");
    assert!(doc.text.contains("never closed"), "{:?}", doc.text);
    assert!(reader
        .issues()
        .iter()
        .any(|issue| issue.code == ebook_format::EpubIssueCode::RepairedXhtml));
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

fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[test]
fn missing_container_falls_back_to_an_opf_in_the_archive() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let chapter =
        br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>fallback</p></body></html>"#;
    let bytes = support::stored_zip(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("OEBPS/content.opf", opf.into_bytes()),
        ("OEBPS/ch1.xhtml", chapter.to_vec()),
    ]);
    let src = support::source(bytes);
    let opened = open(&src).expect("OPF fallback");
    assert_eq!(opened.len(), 1);
    assert!(opened
        .book
        .issues()
        .iter()
        .any(|issue| issue.code == ebook_format::EpubIssueCode::MissingAsset));
    let doc = opened.chapter(&src, 0).expect("chapter");
    assert!(doc.text.contains("fallback"));
}

#[test]
fn broken_container_path_falls_back_and_reports() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let container = support::container("OEBPS/wrong.opf");
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>ok</p></body></html>"#;
    let bytes = support::stored_zip(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container.into_bytes()),
        ("OEBPS/real.opf", opf.into_bytes()),
        ("OEBPS/ch1.xhtml", chapter.to_vec()),
    ]);
    let src = support::source(bytes);
    let opened = open(&src).expect("fallback from broken container");
    assert_eq!(opened.len(), 1);
    assert!(opened
        .book
        .issues()
        .iter()
        .any(|issue| issue.code == ebook_format::EpubIssueCode::MissingAsset));
}

#[test]
fn utf16_chapter_is_decoded() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let chapter = utf16le(
        r#"<?xml version="1.0" encoding="UTF-16"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p>正文</p></body></html>"#,
    );
    let bytes = support::epub(
        "OEBPS/content.opf",
        &opf,
        &[("OEBPS/ch1.xhtml", chapter.as_slice())],
    );
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("UTF-16 chapter");
    assert!(doc.text.contains("正文"), "{:?}", doc.text);
}

#[test]
fn gbk_chapter_is_decoded_from_its_declaration() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    // “正文”的 GBK 字节；声明本身是 ASCII，正文不是合法 UTF-8。
    let mut chapter = Vec::new();
    chapter.extend_from_slice(
        br#"<?xml version="1.0" encoding="gbk"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p>"#,
    );
    chapter.extend_from_slice(&[0xD5, 0xFD, 0xCE, 0xC4]);
    chapter.extend_from_slice(b"</p></body></html>");
    let bytes = support::epub("OEBPS/content.opf", &opf, &[("OEBPS/ch1.xhtml", &chapter)]);
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("GBK chapter");
    assert!(doc.text.contains("正文"), "{:?}", doc.text);
}

#[test]
fn invalid_xml_control_bytes_are_replaced_not_fatal() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let chapter =
        b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>a\x00b\x0bc</p></body></html>";
    let bytes = support::epub("OEBPS/content.opf", &opf, &[("OEBPS/ch1.xhtml", chapter)]);
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("sanitized chapter");
    assert!(doc.text.contains('\u{FFFD}'), "{:?}", doc.text);
}

#[test]
fn deeply_nested_xhtml_is_bounded_and_does_not_take_down_other_chapters() {
    let opf = minimal_opf(
        r#"<itemref idref="bad"/><itemref idref="good"/>"#,
        r#"<item id="bad" href="deep.xhtml" media-type="application/xhtml+xml"/>
           <item id="good" href="good.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let mut deep = String::from(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><div>正文"#);
    for _ in 0..5_000 {
        deep.push_str("<div>");
    }
    deep.push_str("bottom");
    for _ in 0..5_000 {
        deep.push_str("</div>");
    }
    deep.push_str("</div></body></html>");
    let good =
        br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>still readable</p></body></html>"#;
    let bytes = support::epub(
        "OEBPS/content.opf",
        &opf,
        &[
            ("OEBPS/deep.xhtml", deep.as_bytes()),
            ("OEBPS/good.xhtml", good),
        ],
    );
    let src = support::source(bytes);
    let raw = src.document().expect("document");
    let reader = EpubReader::open(&raw).expect("open");
    // 超深结构不再整章报错：降级成纯文本，正文仍可读。
    let doc = reader.chapter(0).expect("deep chapter keeps readable text");
    assert!(doc.text.contains("正文"), "{:?}", doc.text);
    let doc = reader.chapter(1).expect("second chapter");
    assert!(doc.text.contains("still readable"));
    assert!(reader
        .issues()
        .iter()
        .any(|issue| issue.code == ebook_format::EpubIssueCode::TextFallback));
}

#[test]
fn deeply_nested_nav_and_css_are_bounded() {
    let opf = minimal_opf(
        r#"<itemref idref="c1"/>"#,
        r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
           <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#,
    );
    let mut nav = String::from(
        r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc">"#,
    );
    for _ in 0..5_000 {
        nav.push_str("<ol><li>");
    }
    nav.push_str("<a href=\"ch1.xhtml\">deep</a>");
    for _ in 0..5_000 {
        nav.push_str("</li></ol>");
    }
    nav.push_str("</nav></body></html>");

    let mut chapter = String::from(r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style>"#);
    for _ in 0..2_000 {
        chapter.push_str("@media screen {");
    }
    chapter.push_str("p { color: red }");
    for _ in 0..2_000 {
        chapter.push('}');
    }
    chapter.push_str("p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p p { color: blue }");
    chapter.push_str("</style></head><body><p>survives</p></body></html>");

    let bytes = support::epub(
        "OEBPS/content.opf",
        &opf,
        &[
            ("OEBPS/nav.xhtml", nav.as_bytes()),
            ("OEBPS/ch1.xhtml", chapter.as_bytes()),
        ],
    );
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("chapter");
    assert!(doc.text.contains("survives"));
}

#[test]
fn dirty_html_variants_still_build_documents() {
    // 这些都是浏览器能读、严格 XML 会拒绝的真实写法。断言格式层也能读。
    let cases: &[(&str, &str)] = &[
        (r#"<html><body><p>a<p>b</body></html>"#, "b"),
        (r#"<html><body><div><p>a</div>"#, "a"),
        (r#"<p>one</p><p>two</p>"#, "two"),
        (
            r#"<HTML><BODY><P CLASS=x>Hi<BR>There</P></BODY></HTML>"#,
            "There",
        ),
        (r#"<p>a < b and c > d</p>"#, "c"),
        (
            r#"<p><a href=http://example.com/a?x=1&y=2>go</a></p>"#,
            "go",
        ),
        (r#"<ul><li>one<li>two</ul>"#, "two"),
    ];
    let mut no_assets = ebook_format::xhtml::NoAssets;
    for (xml, needle) in cases {
        let doc = ebook_format::xhtml::build(xml, 0, &mut no_assets)
            .unwrap_or_else(|error| panic!("{xml}: {error:?}"));
        assert!(doc.text.contains(needle), "{xml}: {:?}", doc.text);
    }
}

#[test]
fn svg_image_is_an_image_with_intrinsic_size() {
    let xml = r#"<html><body><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="cover.jpeg" width="430" height="571"/></svg></body></html>"#;
    let mut no_assets = ebook_format::xhtml::NoAssets;
    let doc = ebook_format::xhtml::build(xml, 0, &mut no_assets).expect("svg cover");
    assert_eq!(doc.images.len(), 1);
    let image = &doc.images[0];
    assert_eq!(image.src.as_deref(), Some("cover.jpeg"));
    let size = image.size.expect("width/height become intrinsic size");
    assert_eq!((size.w, size.h), (430.0, 571.0));
}

#[test]
fn malformed_ncx_keeps_camel_case_tags_and_toc() {
    let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>NCX</dc:title><dc:language>en</dc:language><dc:identifier id="id">n</dc:identifier>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx"><itemref idref="c1"/></spine>
</package>"#;
    // 裸属性触发容错修复；NCX 的 camelCase 标签必须原样保留。
    let ncx = r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <navMap><navPoint id="p1"><navLabel><text>Chapter One</text></navLabel>
  <content src=ch1.xhtml/></navPoint></navMap>
</ncx>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>hello</p></body></html>"#;
    let bytes = support::epub(
        "OEBPS/content.opf",
        opf,
        &[
            ("OEBPS/toc.ncx", ncx.as_bytes()),
            ("OEBPS/ch1.xhtml", chapter),
        ],
    );
    let src = support::source(bytes);
    let raw = src.document().expect("document");
    let reader = EpubReader::open(&raw).expect("open");
    let doc = reader.chapter(0).expect("chapter");
    assert!(doc.text.contains("hello"));
    assert!(
        reader
            .book()
            .toc
            .iter()
            .any(|entry| entry.label == "Chapter One"),
        "{:?}",
        reader.book().toc
    );
}
