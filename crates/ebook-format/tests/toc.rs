mod support;

use ebook_format::epub::EpubReader;
use ebook_format::{open, ByteSource, EpubIssueCode, TocEntry};
use ebook_ir::{Len, NodeKind, Rgba8, StyledDoc, Tag};

fn find(doc: &StyledDoc, tag: Tag) -> Vec<u32> {
    doc.walk()
        .filter(|&id| matches!(doc.node(id).kind, NodeKind::Element(t) if t == tag))
        .collect()
}

#[test]
fn epub3_nav_gives_ordered_nested_toc() {
    let src = support::source(support::sample_epub3());
    let opened = open(&src).expect("open");
    let book = opened.book.clone();
    let toc = book.toc();
    assert_eq!(toc.len(), 5, "toc: {toc:#?}");

    assert_eq!(toc[0].label, "One");
    assert_eq!(toc[0].href, "OEBPS/text/ch1.xhtml");
    assert_eq!(toc[0].spine, 0);
    assert_eq!(toc[0].fragment, None);
    assert_eq!(toc[0].depth, 0);

    assert_eq!(toc[1].label, "Section 1");
    assert_eq!(toc[1].href, "OEBPS/text/ch1.xhtml");
    assert_eq!(toc[1].fragment.as_deref(), Some("sec1"));
    assert_eq!(toc[1].spine, 0);
    assert_eq!(toc[1].depth, 1);

    assert_eq!(toc[2].label, "Two & a half");
    assert_eq!(toc[2].href, "OEBPS/text/ch2.xhtml");
    assert_eq!(toc[2].fragment.as_deref(), Some("top"));
    assert_eq!(toc[2].spine, 1);

    // 没有链接的分组行保留标签，但不属于阅读顺序。
    assert_eq!(toc[3].label, "Group");
    assert_eq!(toc[3].spine, TocEntry::NO_SPINE);
    assert!(!toc[3].in_spine());
    assert_eq!(toc[3].depth, 0);

    assert_eq!(toc[4].label, "Sub");
    assert_eq!(toc[4].spine, 1);
    assert_eq!(toc[4].depth, 1);
}

#[test]
fn metadata_is_filled_beyond_title_and_lang() {
    let src = support::source(support::sample_epub3());
    let opened = open(&src).expect("open");
    let book = &opened.book;
    assert_eq!(book.title.as_deref(), Some("Toc Book"));
    assert_eq!(book.creator.as_deref(), Some("Ada Lovelace"));
    assert_eq!(book.creators.len(), 1);
    assert_eq!(book.creators[0].name, "Ada Lovelace");
    assert_eq!(book.publisher.as_deref(), Some("Analytical Press"));
    assert_eq!(book.identifier.as_deref(), Some("urn:uuid:toc-book"));
    assert_eq!(book.language.as_deref(), Some("en"));
    assert_eq!(book.lang.as_deref(), Some("en"));
    assert_eq!(book.date.as_deref(), Some("1843-01-01"));
    assert_eq!(
        book.description.as_deref(),
        Some("A book about a table of contents.")
    );
    assert_eq!(book.cover_href.as_deref(), Some("OEBPS/img/cover.png"));
}

#[test]
fn multiple_creators_keep_order_and_roles() {
    let opf = r##"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Many Authors</dc:title>
    <dc:creator opf:role="aut" opf:file-as="Lovelace, Ada">Ada Lovelace</dc:creator>
    <dc:creator opf:role="edt" opf:file-as="Babbage, Charles">Charles Babbage</dc:creator>
    <dc:identifier id="id">many-authors</dc:identifier>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"##;
    let src = support::source(support::epub("OEBPS/content.opf", opf, &[]));
    let opened = open(&src).expect("open");
    let book = &opened.book;
    assert_eq!(book.creator.as_deref(), Some("Ada Lovelace"));
    assert_eq!(book.creators.len(), 2);
    assert_eq!(book.creators[0].name, "Ada Lovelace");
    assert_eq!(book.creators[0].role.as_deref(), Some("aut"));
    assert_eq!(book.creators[0].file_as.as_deref(), Some("Lovelace, Ada"));
    assert_eq!(book.creators[1].name, "Charles Babbage");
    assert_eq!(book.creators[1].role.as_deref(), Some("edt"));
}

#[test]
fn links_are_collected_and_resolved() {
    let src = support::source(support::sample_epub3());
    let raw = src.document().expect("document");
    let reader = EpubReader::open(&raw).expect("reader");
    let (doc, links) = reader.chapter_with_links(0).expect("chapter");
    assert_eq!(links.len(), 4, "links: {links:#?}");

    let anchors = find(&doc, Tag::A);
    assert_eq!(anchors.len(), links.len());
    for link in &links {
        assert!(anchors.contains(&link.node));
    }

    let self_link = reader.resolve_link_from(0, "#sec1").expect("same chapter");
    assert_eq!(self_link.spine, 0);
    assert_eq!(self_link.fragment.as_deref(), Some("sec1"));
    assert!(!self_link.external);

    let next = reader
        .resolve_link_from(0, "../text/ch2.xhtml#top")
        .expect("cross chapter");
    assert_eq!(next.spine, 1);
    assert_eq!(next.fragment.as_deref(), Some("top"));

    let external = reader
        .resolve_link_from(0, "https://example.com/x")
        .expect("external");
    assert!(external.external);

    assert!(reader.resolve_link_from(0, "missing.xhtml").is_none());

    // 没有章节上下文时 `#id` 取第一个包含该 id 的章节。
    let any = reader.resolve_link("#sec1").expect("fragment lookup");
    assert_eq!(any.spine, 0);
    assert_eq!(any.fragment.as_deref(), Some("sec1"));

    // 相对包根目录的路径也能解析。
    let rooted = reader
        .resolve_link("OEBPS/text/ch2.xhtml#top")
        .expect("rooted");
    assert_eq!(rooted.spine, 1);
}

#[test]
fn css_media_fonts_and_issues_are_applied() {
    let src = support::source(support::sample_epub3());
    let raw = src.document().expect("document");
    let reader = EpubReader::open(&raw).expect("reader");
    let (doc, _) = reader.chapter_with_links(0).expect("chapter");

    let body = find(&doc, Tag::Body)[0];
    // screen 分支胜出，print 分支被忽略。
    assert_eq!(
        doc.node(body).style.color,
        Rgba8::new(0x11, 0x11, 0x11, 255)
    );
    // margin: 0 auto 的两侧是 auto，不是 0。
    assert_eq!(doc.node(body).style.margin.left, Len::Auto);
    assert_eq!(doc.node(body).style.margin.right, Len::Auto);
    assert_eq!(doc.node(body).style.margin.top, Len::Px(0.0));

    // 内联 !important 压过类规则；font 简写拆成尺寸/字重/样式/字体栈。
    let styled = find(&doc, Tag::P)
        .into_iter()
        .find(|&id| (doc.node(id).style.font.size - 12.0).abs() < 0.01)
        .expect("p.styled");
    assert_eq!(
        doc.node(styled).style.color,
        Rgba8::new(0x12, 0x34, 0x56, 255)
    );
    assert_eq!(doc.node(styled).style.font.weight, 700);
    assert_eq!(
        doc.node(styled).style.font.style,
        ebook_ir::FontStyle::Italic
    );
    assert_eq!(
        doc.node(styled).style.line_height,
        ebook_ir::LineHeight::Mul(1.4)
    );
    assert_eq!(doc.node(styled).style.brk.before, ebook_ir::Break::Always);
    assert_eq!(doc.node(styled).style.font.families.len(), 1);

    let issues = reader.issues();
    assert!(
        issues
            .iter()
            .any(|issue| issue.code == EpubIssueCode::UnsupportedCss
                && issue.detail.contains("box-shadow")),
        "issues: {issues:#?}"
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.code == EpubIssueCode::DuplicateId),
        "issues: {issues:#?}"
    );
}

#[test]
fn epub2_ncx_is_the_toc_fallback() {
    let opf = r##"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Old Book</dc:title><dc:language>en</dc:language>
    <dc:identifier id="id">old</dc:identifier>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"##;

    let ncx = r##"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head><meta name="dtb:uid" content="old"/></head>
  <docTitle><text>Old Book</text></docTitle>
  <navMap>
    <navPoint id="n1" playOrder="1">
      <navLabel><text>Chapter One</text></navLabel>
      <content src="ch1.xhtml"/>
      <navPoint id="n1a" playOrder="2">
        <navLabel><text>Section</text></navLabel>
        <content src="ch1.xhtml#s1"/>
      </navPoint>
    </navPoint>
    <navPoint id="n2" playOrder="3">
      <navLabel><text>Chapter Two</text></navLabel>
      <content src="ch2.xhtml#top"/>
    </navPoint>
  </navMap>
</ncx>"##;

    let ch1 = r##"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">one</p></body></html>"##;
    let ch2 = r##"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p id="top">two</p></body></html>"##;

    let bytes = support::epub(
        "OEBPS/content.opf",
        opf,
        &[
            ("OEBPS/toc.ncx", ncx.as_bytes()),
            ("OEBPS/ch1.xhtml", ch1.as_bytes()),
            ("OEBPS/ch2.xhtml", ch2.as_bytes()),
        ],
    );
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    let toc = opened.book.toc();
    assert_eq!(toc.len(), 3, "toc: {toc:#?}");
    assert_eq!(toc[0].label, "Chapter One");
    assert_eq!(toc[0].spine, 0);
    assert_eq!(toc[0].depth, 0);
    assert_eq!(toc[1].label, "Section");
    assert_eq!(toc[1].fragment.as_deref(), Some("s1"));
    assert_eq!(toc[1].depth, 1);
    assert_eq!(toc[2].fragment.as_deref(), Some("top"));
    assert_eq!(toc[2].spine, 1);
}
