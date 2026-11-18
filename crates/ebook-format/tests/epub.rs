use ebook_format::{open, MemorySource};
use ebook_ir::{Len, NodeId, NodeKind, Rgba8, StyledDoc, Tag, TextAlign};

fn source() -> MemorySource {
    let mut src = MemorySource::new();
    src.insert("demo.epub", include_bytes!("../../../fixtures/demo.epub").to_vec());
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
    assert_eq!(doc.node(lead).style.color, Rgba8::new(0x55, 0x55, 0x55, 255));

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
    assert_eq!(doc.node(text).style.color, Rgba8::new(0x55, 0x55, 0x55, 255));
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
