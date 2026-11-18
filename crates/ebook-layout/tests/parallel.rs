use ebook_ir::{
    codec, ComputedStyle, Dir, EdgesF, FontId, FontSpec, Limits, Node, NodeKind, Rgba8, Span,
    StyledDoc, Tag,
};
use ebook_layout::{Layout, LayoutEnv, LayoutOptions, NoImageSizes, PageConstraint, StubMeasurer};

fn node(kind: NodeKind, style: ComputedStyle, parent: Option<u32>) -> Node {
    Node {
        kind,
        style,
        parent,
        first_child: 0,
        child_len: 0,
        src: None,
        link: None,
    }
}

fn paragraph_doc(paragraphs: usize, words: usize) -> StyledDoc {
    let mut doc = StyledDoc::default();
    let root = doc.push_node(node(
        NodeKind::Element(Tag::Root),
        ComputedStyle::default(),
        None,
    ));
    doc.root = root;
    let style = ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 16.0,
            weight: 400,
            ..Default::default()
        },
        Rgba8::BLACK,
        Dir::Ltr,
    );
    let mut kids = Vec::new();
    for p in 0..paragraphs {
        let block = doc.push_node(node(
            NodeKind::Element(Tag::P),
            ComputedStyle::default(),
            Some(root),
        ));
        let start = doc.text.len() as u32;
        for index in 0..words {
            if index > 0 {
                doc.text.push(' ');
            }
            doc.text
                .push_str(if p % 3 == 0 { "winter" } else { "lantern" });
        }
        let span = Span::new(start, doc.text.len() as u32 - start);
        let text = doc.push_node(node(NodeKind::Text(span), style, Some(block)));
        doc.set_kids(block, &[text]);
        kids.push(block);
    }
    doc.set_kids(root, &kids);
    doc
}

fn bytes(ir: &ebook_ir::LayoutResult) -> Vec<u8> {
    codec::to_bytes(ir).expect("encode")
}

#[test]
fn parallel_matches_serial_byte_for_byte() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let docs = [
        paragraph_doc(1, 30),
        paragraph_doc(6, 40),
        paragraph_doc(12, 90),
    ];
    let sizes = [(220.0, 150.0), (600.0, 800.0), (900.0, 1200.0)];

    for doc in &docs {
        for (width, height) in sizes {
            let constraint = PageConstraint::new(width, height).with_margin(EdgesF::all(12.0));
            let serial = Layout::new().layout(doc, &env, &constraint);
            let serial_bytes = bytes(&serial);
            for workers in [1usize, 2, 8] {
                let parallel = Layout::new().layout_parallel(
                    doc,
                    &env,
                    &constraint,
                    workers,
                    &LayoutOptions::default(),
                );
                assert_eq!(
                    bytes(&parallel),
                    serial_bytes,
                    "workers={workers} changed the bytes at {width}x{height}"
                );
                assert_eq!(parallel, serial);
            }
        }
    }
}

#[test]
fn parallel_respects_limits_the_same_way() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let doc = paragraph_doc(8, 60);
    let constraint = PageConstraint::new(240.0, 140.0).with_margin(EdgesF::all(10.0));
    let opts = LayoutOptions::with_limits(Limits {
        max_lines: 7,
        max_pages: 2,
        ..Default::default()
    });
    let serial = Layout::new().layout_with(&doc, &env, &constraint, &opts);
    assert!(serial.truncated);
    for workers in [1usize, 2, 8] {
        let parallel = Layout::new().layout_parallel(&doc, &env, &constraint, workers, &opts);
        assert_eq!(bytes(&parallel), bytes(&serial));
        assert_eq!(parallel.runs.lines.len(), serial.runs.lines.len());
        assert_eq!(parallel.pages.len(), serial.pages.len());
    }
}
