use ebook_ir::{
    codec, ComputedStyle, Dir, DisplayList, EdgesF, FontId, FontSpec, ImageId, IssueCode,
    LayoutResult, Limits, Node, NodeKind, Op, Rgba8, SizeF, Span, StyledDoc, Tag, UnresolvedReason,
    IR_VERSION,
};
use ebook_layout::{
    Layout, LayoutEnv, LayoutOptions, NoImageSizes, PageConstraint, ResourceView, StubMeasurer,
};

fn constraint(width: f32, height: f32) -> PageConstraint {
    PageConstraint::new(width, height).with_margin(EdgesF::all(8.0))
}

fn env(resources: &dyn ResourceView) -> LayoutEnv<'_> {
    LayoutEnv {
        measurer: &StubMeasurer,
        resources,
    }
}

fn node(
    kind: NodeKind,
    style: ComputedStyle,
    parent: Option<u32>,
    src: Option<ebook_ir::Anchor>,
) -> Node {
    Node {
        kind,
        style,
        parent,
        first_child: 0,
        child_len: 0,
        src,
        link: None,
    }
}

/// 根 + 一段可以自己控制长度的文本。
fn doc_with_words(words: usize) -> StyledDoc {
    let mut doc = StyledDoc::default();
    let root = doc.push_node(node(
        NodeKind::Element(Tag::Root),
        ComputedStyle::default(),
        None,
        None,
    ));
    doc.root = root;
    let start = doc.text.len() as u32;
    for index in 0..words {
        if index > 0 {
            doc.text.push(' ');
        }
        doc.text.push_str("winter");
    }
    let span = Span::new(start, doc.text.len() as u32 - start);
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
    let text = doc.push_node(node(NodeKind::Text(span), style, Some(root), None));
    doc.set_kids(root, &[text]);
    doc
}

/// 一条 depth 条边的链：root(0) -> 1 -> ... -> depth。
fn doc_chain(depth: u32) -> StyledDoc {
    let mut doc = StyledDoc::default();
    let root = doc.push_node(node(
        NodeKind::Element(Tag::Root),
        ComputedStyle::default(),
        None,
        None,
    ));
    doc.root = root;
    let mut parent = root;
    for _ in 0..depth {
        let child = doc.push_node(node(
            NodeKind::Element(Tag::Div),
            ComputedStyle::default(),
            Some(parent),
            None,
        ));
        doc.set_kids(parent, &[child]);
        parent = child;
    }
    doc
}

fn doc_image() -> StyledDoc {
    let mut doc = StyledDoc::default();
    let root = doc.push_node(node(
        NodeKind::Element(Tag::Root),
        ComputedStyle::default(),
        None,
        None,
    ));
    doc.root = root;
    let image = doc.push_node(node(
        NodeKind::Image(0),
        ComputedStyle::default(),
        Some(root),
        None,
    ));
    doc.set_kids(root, &[image]);
    doc
}

/// 只有资源层知道图片尺寸：100 x 50 = 5000 像素。
struct FixedImage;

impl ResourceView for FixedImage {
    fn image_size(&self, _id: ImageId) -> Option<SizeF> {
        Some(SizeF::new(100.0, 50.0))
    }
}

fn run(
    doc: &StyledDoc,
    c: &PageConstraint,
    limits: Limits,
    resources: &dyn ResourceView,
) -> LayoutResult {
    Layout::new().layout_with(doc, &env(resources), c, &LayoutOptions::with_limits(limits))
}

fn default_result(doc: &StyledDoc, c: &PageConstraint) -> LayoutResult {
    run(doc, c, Limits::default(), &NoImageSizes)
}

fn assert_legal(ir: &LayoutResult) {
    let mut ops = 0u32;
    let mut lines = 0u32;
    let mut prev = f32::NEG_INFINITY;
    for page in &ir.pages {
        assert_eq!(page.ops.start, ops, "page ops are not contiguous");
        assert_eq!(page.lines.start, lines, "page lines are not contiguous");
        ops += page.ops.len;
        lines += page.lines.len;
        assert!(page.origin.y >= prev - 1e-3, "page origin went backwards");
        prev = page.origin.y;
        for line in &ir.runs.lines[page.lines.as_range()] {
            assert!(line.rect.y >= page.origin.y - 1e-3, "line above page");
            assert!(
                line.rect.bottom() <= page.origin.y + page.size.h + 1e-3,
                "line below page"
            );
            assert!(line.glyphs.end() as usize <= ir.display.glyphs.len());
        }
    }
    assert_eq!(ops as usize, ir.display.ops.len(), "pages do not cover ops");
    assert_eq!(
        lines as usize,
        ir.runs.lines.len(),
        "pages do not cover lines"
    );
}

fn assert_same_bytes(a: &LayoutResult, b: &LayoutResult) {
    assert_eq!(
        codec::to_bytes(a).expect("encode a"),
        codec::to_bytes(b).expect("encode b")
    );
}

fn limit_issue(ir: &LayoutResult) -> bool {
    ir.issues
        .iter()
        .any(|issue| issue.code == IssueCode::LimitExceeded)
}

#[test]
fn zero_fields_are_defaults() {
    let zero = Limits {
        max_nodes: 0,
        max_depth: 0,
        max_text_bytes: 0,
        max_pages: 0,
        max_lines: 0,
        max_image_pixels: 0,
    };
    assert_eq!(zero.resolved(), Limits::default());
    assert_eq!(Limits::default().resolved(), Limits::default());

    let doc = doc_with_words(40);
    let c = constraint(200.0, 120.0);
    let via_zero = run(&doc, &c, zero, &NoImageSizes);
    let via_default = default_result(&doc, &c);
    assert_same_bytes(&via_zero, &via_default);
    assert!(!via_zero.truncated);
}

#[test]
fn max_nodes_is_checked_before_flow() {
    let doc = doc_with_words(20);
    let c = constraint(200.0, 400.0);
    let total = doc.nodes.len() as u32;
    assert!(total >= 2);

    let exact = run(
        &doc,
        &c,
        Limits {
            max_nodes: total,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(!exact.truncated);
    assert!(!exact.pages.is_empty());
    assert_legal(&exact);

    let over = run(
        &doc,
        &c,
        Limits {
            max_nodes: total - 1,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(over.truncated);
    assert!(over.pages.is_empty());
    assert!(over.display.ops.is_empty());
    assert!(over.runs.lines.is_empty());
    assert_eq!(over.version, IR_VERSION);
    assert_eq!(over.spine, doc.meta.spine);
    assert!(limit_issue(&over));
    assert_legal(&over);
}

#[test]
fn max_depth_is_checked_before_flow() {
    let doc = doc_chain(6); // 最大深度 = 6 条边
    let c = constraint(200.0, 400.0);

    let exact = run(
        &doc,
        &c,
        Limits {
            max_depth: 6,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(!exact.truncated);
    assert_legal(&exact);

    let over = run(
        &doc,
        &c,
        Limits {
            max_depth: 5,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(over.truncated);
    assert!(over.pages.is_empty());
    assert!(limit_issue(&over));
    assert_legal(&over);
}

#[test]
fn max_text_bytes_is_checked_before_flow() {
    let doc = doc_with_words(30);
    let c = constraint(200.0, 400.0);
    let bytes = doc.text.len() as u32;

    let exact = run(
        &doc,
        &c,
        Limits {
            max_text_bytes: bytes,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(!exact.truncated);
    assert_same_bytes(&exact, &default_result(&doc, &c));

    let over = run(
        &doc,
        &c,
        Limits {
            max_text_bytes: bytes - 1,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(over.truncated);
    assert!(over.pages.is_empty());
    assert!(limit_issue(&over));
    assert_legal(&over);
}

#[test]
fn max_lines_keeps_the_prefix_it_produced() {
    let doc = doc_with_words(220);
    let c = constraint(200.0, 120.0);
    let full = default_result(&doc, &c);
    let total = full.runs.lines.len() as u32;
    assert!(total > 8, "corpus too thin: {total} lines");

    let exact = run(
        &doc,
        &c,
        Limits {
            max_lines: total,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(!exact.truncated);
    assert_same_bytes(&exact, &full);

    let cut = 5u32;
    let over = run(
        &doc,
        &c,
        Limits {
            max_lines: cut,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(over.truncated);
    assert_eq!(over.runs.lines.len(), cut as usize);
    assert!(over.runs.lines.len() < total as usize);
    // 保留的必须是前缀，而不是任意一段。
    assert_eq!(&over.runs.lines[..], &full.runs.lines[..cut as usize]);
    assert!(limit_issue(&over));
    assert_legal(&over);

    // 截断结果两次必须相同：不能依赖线程调度。
    let again = run(
        &doc,
        &c,
        Limits {
            max_lines: cut,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert_same_bytes(&over, &again);
}

#[test]
fn max_pages_hard_cuts_whole_pages() {
    let doc = doc_with_words(400);
    let c = constraint(220.0, 110.0);
    let full = default_result(&doc, &c);
    let total = full.pages.len() as u32;
    assert!(total > 4, "corpus too thin: {total} pages");

    let exact = run(
        &doc,
        &c,
        Limits {
            max_pages: total,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(!exact.truncated);
    assert_same_bytes(&exact, &full);

    let cut = 3u32;
    let over = run(
        &doc,
        &c,
        Limits {
            max_pages: cut,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(over.truncated);
    assert_eq!(over.pages.len(), cut as usize);
    assert!(limit_issue(&over));
    for (page, expected) in over.pages.iter().zip(&full.pages) {
        assert_eq!(page.origin.y, expected.origin.y);
        assert_eq!(page.index, expected.index);
    }
    assert!(over.runs.lines.len() < full.runs.lines.len());
    assert_legal(&over);

    let again = run(
        &doc,
        &c,
        Limits {
            max_pages: cut,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert_same_bytes(&over, &again);
}

fn has_image_op(ir: &LayoutResult) -> bool {
    ir.display.ops.iter().any(|op| match op {
        Op::Clip { ops, .. } => ops.iter().any(|inner| matches!(inner, Op::Image { .. })),
        Op::Image { .. } => true,
        _ => false,
    })
}

fn has_size_from_content(ir: &LayoutResult) -> bool {
    ir.display.ops.iter().any(|op| match op {
        Op::Unresolved(unresolved) => unresolved.reason == UnresolvedReason::SizeFromContent,
        _ => false,
    })
}

#[test]
fn max_image_pixels_degrades_over_budget_images() {
    let doc = doc_image();
    let c = constraint(200.0, 400.0);

    let exact = run(
        &doc,
        &c,
        Limits {
            max_image_pixels: 5000,
            ..Default::default()
        },
        &FixedImage,
    );
    assert!(!exact.truncated);
    assert!(has_image_op(&exact));
    assert!(!has_size_from_content(&exact));

    let over = run(
        &doc,
        &c,
        Limits {
            max_image_pixels: 4999,
            ..Default::default()
        },
        &FixedImage,
    );
    assert!(over.truncated);
    assert!(!has_image_op(&over));
    assert!(has_size_from_content(&over));
    assert!(limit_issue(&over));
    assert_legal(&over);

    // 资源层不知道尺寸时 flow 已经降级成 ImagePending，像素预算无从累加。
    let unknown = run(
        &doc,
        &c,
        Limits {
            max_image_pixels: 1,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(!unknown.truncated);
    assert!(!has_image_op(&unknown));
    assert!(!has_size_from_content(&unknown));
    assert!(unknown
        .issues
        .iter()
        .any(|issue| issue.code == IssueCode::MissingImage));
}

#[test]
fn truncated_results_are_still_decodable() {
    let doc = doc_with_words(220);
    let c = constraint(200.0, 120.0);
    let ir = run(
        &doc,
        &c,
        Limits {
            max_lines: 4,
            max_pages: 2,
            ..Default::default()
        },
        &NoImageSizes,
    );
    assert!(ir.truncated);
    let bytes = codec::to_bytes(&ir).expect("encode");
    let back: LayoutResult = codec::decode(&bytes).expect("decode");
    assert_eq!(back, ir);
    assert!(back.truncated);
    assert_legal(&back);
    let _ = (DisplayList::default(), Span::EMPTY);
}
