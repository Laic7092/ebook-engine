//! 章内图片表：`ImageId` 是章内顺序，`ImageMeta::src` 是解析后的归档路径。

use ebook_format::epub::EpubReader;
use ebook_ir::{NodeKind, SizeF};

fn reader_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/images.epub"
    ))
    .expect("fixtures/images.epub")
}

#[test]
fn chapter_images_are_numbered_in_document_order_and_carry_src() {
    let raw = reader_bytes();
    let reader = EpubReader::open(&raw).expect("open");

    let (first, _, _) = reader.chapter_with_anchors(0).expect("chapter 0");
    assert_eq!(first.images.len(), 2);
    assert_eq!(
        first.images[0].src.as_deref(),
        Some("OEBPS/pics/wide.png"),
        "src is resolved against the chapter directory"
    );
    assert_eq!(first.images[0].size, Some(SizeF::new(12.0, 6.0)));
    assert_eq!(first.images[0].mime.as_deref(), Some("image/png"));
    assert_eq!(first.images[1].src.as_deref(), Some("OEBPS/pics/tall.png"));
    assert_eq!(first.images[1].size, Some(SizeF::new(4.0, 10.0)));

    // `NodeKind::Image` 的下标就是这张表的下标。
    let ids: Vec<u16> = first
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            NodeKind::Image(id) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec![0, 1]);

    // 第二章里同一张图重新编号成它自己的顺序（章节级表，不是 manifest 顺序）。
    let (second, _, _) = reader.chapter_with_anchors(1).expect("chapter 1");
    assert_eq!(second.images.len(), 1);
    assert_eq!(second.images[0].src.as_deref(), Some("OEBPS/pics/tall.png"));
    assert_eq!(second.images[0].size, Some(SizeF::new(4.0, 10.0)));
}

#[test]
fn fragments_resolve_to_node_anchors() {
    let raw = reader_bytes();
    let reader = EpubReader::open(&raw).expect("open");
    let (doc, _, anchors) = reader.chapter_with_anchors(0).expect("chapter 0");

    let note = anchors
        .iter()
        .find(|entry| entry.id == "note")
        .expect("#note");
    assert_eq!(note.anchor.spine, 0);
    // Anchor 的 path 指向同一章里 id="note" 的那个节点。
    let node = doc
        .walk()
        .find(|&id| {
            let node = doc.node(id);
            node.src.as_ref().map(|src| &src.path) == Some(&note.anchor.path)
        })
        .expect("node with the anchor path");
    assert!(doc.nodes[node as usize].child_len > 0);

    let toc = reader.book().toc();
    assert_eq!(toc.len(), 3, "{toc:#?}");
    assert_eq!(toc[0].label, "One: The Wide Picture");
    assert_eq!(toc[0].depth, 0);
    assert_eq!(toc[1].label, "A note in chapter one");
    assert_eq!(toc[1].fragment.as_deref(), Some("note"));
    assert_eq!(toc[1].depth, 1);
    assert_eq!(toc[2].spine, 1);
    assert_eq!(toc[2].fragment.as_deref(), Some("target"));
}

#[test]
fn embedded_font_is_read_with_its_family() {
    let raw = reader_bytes();
    let reader = EpubReader::open(&raw).expect("open");
    let fonts = reader.book().fonts();
    assert_eq!(fonts.len(), 1, "{fonts:#?}");
    assert_eq!(fonts[0].family, "Book Serif");
    assert_eq!(fonts[0].href, "OEBPS/font/test.ttf");
    assert_eq!(fonts[0].weight, 400);
    assert!(!fonts[0].bytes.is_empty());
}

#[test]
fn links_are_raw_and_resolve_against_the_chapter() {
    let raw = reader_bytes();
    let reader = EpubReader::open(&raw).expect("open");
    let (_, links, _) = reader.chapter_with_anchors(0).expect("chapter 0");
    // `Node.link` / `Op::Link.href` 存属性原文（相对本章），解析交给宿主。
    assert!(links.iter().any(|link| link.href == "ch2.xhtml#target"));
    let target = reader
        .resolve_link_from(0, "ch2.xhtml#target")
        .expect("resolves");
    assert_eq!(target.spine, 1);
    assert_eq!(target.fragment.as_deref(), Some("target"));
}
