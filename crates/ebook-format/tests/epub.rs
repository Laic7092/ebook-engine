use ebook_format::{open, MemorySource};
use ebook_ir::{Len, NodeId, NodeKind, Rgba8, StyledDoc, Tag, TextAlign};

fn source() -> MemorySource {
    let mut src = MemorySource::new();
    src.insert(
        "demo.epub",
        include_bytes!("../../../fixtures/demo.epub").to_vec(),
    );
    src
}

fn chapter(index: usize) -> StyledDoc {
    let src = source();
    let opened = open(&src).expect("open");
    opened.chapter(&src, index).expect("chapter")
}

fn find(doc: &StyledDoc, tag: Tag) -> Vec<NodeId> {
    doc.walk()
        .filter(|&id| matches!(doc.node(id).kind, NodeKind::Element(t) if t == tag))
        .collect()
}

#[test]
fn reads_metadata_and_spine() {
    let src = source();
    let opened = open(&src).expect("open");
    assert_eq!(opened.adapter_name(), "epub");
    assert_eq!(opened.book.title.as_deref(), Some("雪夜"));
    assert_eq!(opened.book.lang.as_deref(), Some("zh-Hans"));
    assert_eq!(opened.len(), 2);
    assert_eq!(opened.book.spine[0].href, "OEBPS/ch1.xhtml");
    assert_eq!(opened.book.spine[1].href, "OEBPS/ch2.xhtml");
    assert!(opened.book.spine.iter().all(|item| item.linear));
}

#[test]
fn cascade_applies_element_class_and_id_rules() {
    let doc = chapter(0);

    let h1 = find(&doc, Tag::H1)[0];
    assert_eq!(doc.node(h1).style.font.size, 36.0);
    assert_eq!(doc.node(h1).style.text.align, TextAlign::Center);

    let paragraphs = find(&doc, Tag::P);
    let lead = paragraphs
        .iter()
        .copied()
        .find(|&id| (doc.node(id).style.font.size - 19.8).abs() < 0.01)
        .expect("p.lead");
    assert_eq!(
        doc.node(lead).style.color,
        Rgba8::new(0x55, 0x55, 0x55, 255)
    );

    let note = paragraphs
        .iter()
        .copied()
        .find(|&id| matches!(doc.node(id).style.border.width.top, Len::Px(v) if v > 0.5))
        .expect("p#note");
    assert_eq!(doc.node(note).style.padding.top, Len::Px(8.0));

    let em = find(&doc, Tag::Em)[0];
    assert_eq!(doc.node(em).style.color, Rgba8::new(0x99, 0x00, 0x00, 255));
}

#[test]
fn inheritance_reaches_text_nodes() {
    let doc = chapter(0);
    let lead = find(&doc, Tag::P)
        .into_iter()
        .find(|&id| (doc.node(id).style.font.size - 19.8).abs() < 0.01)
        .expect("p.lead");
    let text = doc.kids(lead)[0];
    assert!(doc.node(text).kind.is_text());
    assert!((doc.node(text).style.font.size - 19.8).abs() < 0.01);
    assert_eq!(
        doc.node(text).style.color,
        Rgba8::new(0x55, 0x55, 0x55, 255)
    );
}

#[test]
fn anchors_cover_every_node() {
    let doc = chapter(1);
    for id in doc.walk() {
        assert!(doc.node(id).src.is_some());
    }
    assert_eq!(doc.meta.spine, 1);
}

#[test]
fn text_spans_are_valid_utf8() {
    let doc = chapter(0);
    assert!(!doc.text.is_empty());
    for id in doc.walk() {
        if let NodeKind::Text(span) = doc.node(id).kind {
            assert!(!doc.text_of(span).is_empty());
        }
    }
}

#[test]
fn broken_or_missing_input_is_an_error() {
    let mut src = MemorySource::new();
    src.insert("nothing.epub", b"not a zip".to_vec());
    assert!(open(&src).is_err());
}

fn entity_source() -> MemorySource {
    let mut src = MemorySource::new();
    src.insert(
        "entity.epub",
        include_bytes!("../../../fixtures/entity.epub").to_vec(),
    );
    src
}

#[test]
fn reads_chapters_with_named_and_numeric_entities() {
    let src = entity_source();
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("chapter");
    let text = doc.text.as_str();
    assert!(text.contains('\u{a0}'), "missing nbsp: {text:?}");
    assert!(text.contains('\u{2014}'), "missing mdash: {text:?}");
    assert!(text.contains('\u{2026}'), "missing hellip: {text:?}");
    assert!(text.contains('\u{2019}'), "missing rsquo: {text:?}");
    assert!(text.contains('&'), "missing ampersand: {text:?}");
    assert!(
        text.contains("&bogus;"),
        "unknown entity should survive as text: {text:?}"
    );
}

#[test]
fn unknown_entities_do_not_break_the_chapter() {
    let src = entity_source();
    let opened = open(&src).expect("open");
    assert_eq!(opened.len(), 1);
    let doc = opened.chapter(&src, 0).expect("chapter");
    assert!(!doc.nodes.is_empty());
}

mod support;

/// pandoc / calibre / Sigil 产出的 XHTML 几乎都带 `<!DOCTYPE html>`。
/// roxmltree 默认拒绝 DOCTYPE，曾经让整本书每个章节都 `Malformed("xhtml")`。
#[test]
fn pandoc_style_doctype_is_accepted() {
    let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Doctype</dc:title><dc:language>zh-Hans</dc:language><dc:identifier id="id">d</dc:identifier>
  </metadata>
  <manifest><item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#;
    let chapter = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="zh-Hans" xml:lang="zh-Hans">
<head><meta charset="utf-8" /><title>ch001.xhtml</title></head>
<body epub:type="bodymatter"><section id="x" class="level1"><h1>第一卷</h1><p>正文 &amp; 版权</p></section></body>
</html>"#;
    let bytes = support::epub(
        "EPUB/content.opf",
        opf,
        &[("EPUB/ch1.xhtml", chapter.as_bytes())],
    );
    let src = support::source(bytes);
    let opened = open(&src).expect("open");
    let doc = opened.chapter(&src, 0).expect("chapter");
    let text = doc.text.as_str();
    assert!(text.contains("第一卷"), "{text:?}");
    assert!(text.contains("正文 & 版权"), "{text:?}");
}
