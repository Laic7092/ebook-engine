use ebook_ir::{
    codec, ComputedStyle, Dir, EdgesF, FontId, FontSpec, LayoutResult, Node, NodeKind, Rgba8, Span,
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
                .push_str(if p % 2 == 0 { "lantern" } else { "harbor" });
        }
        let span = Span::new(start, doc.text.len() as u32 - start);
        let text = doc.push_node(node(NodeKind::Text(span), style, Some(block)));
        doc.set_kids(block, &[text]);
        kids.push(block);
    }
    doc.set_kids(root, &kids);
    doc
}

fn bytes(ir: &LayoutResult) -> Vec<u8> {
    codec::to_bytes(ir).expect("encode")
}

#[test]
fn parallel_layout_matches_serial_for_every_worker_count() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let docs = [
        paragraph_doc(1, 20),
        paragraph_doc(5, 45),
        paragraph_doc(10, 80),
    ];
    let sizes = [(200.0, 140.0), (480.0, 640.0), (800.0, 1000.0)];
    for doc in &docs {
        for (width, height) in sizes {
            let c = PageConstraint::new(width, height).with_margin(EdgesF::all(12.0));
            let serial = Layout::new().layout(doc, &env, &c);
            for workers in [1usize, 2, 8] {
                let parallel = Layout::new().layout_parallel(
                    doc,
                    &env,
                    &c,
                    workers,
                    &LayoutOptions::default(),
                );
                assert_eq!(
                    bytes(&parallel),
                    bytes(&serial),
                    "workers={workers} differs at {width}x{height}"
                );
            }
        }
    }
}

#[test]
fn parallel_layout_repeats_identically() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let doc = paragraph_doc(7, 60);
    let c = PageConstraint::new(300.0, 200.0).with_margin(EdgesF::all(10.0));
    let first = Layout::new().layout_parallel(&doc, &env, &c, 8, &LayoutOptions::default());
    for _ in 0..4 {
        let again = Layout::new().layout_parallel(&doc, &env, &c, 8, &LayoutOptions::default());
        assert_eq!(bytes(&again), bytes(&first));
    }
}
