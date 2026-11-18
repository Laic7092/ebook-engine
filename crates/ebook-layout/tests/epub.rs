use ebook_format::{open, MemorySource};
use ebook_ir::{codec, EdgesF, Op, StyledDoc};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

fn chapter() -> StyledDoc {
    let mut src = MemorySource::new();
    src.insert(
        "demo.epub",
        include_bytes!("../../../fixtures/demo.epub").to_vec(),
    );
    let opened = open(&src).expect("open");
    opened.chapter(&src, 0).expect("chapter")
}

fn ir() -> ebook_ir::LayoutResult {
    let doc = chapter();
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
    layout(&doc, &env, &constraint)
}

#[test]
fn unresolved_images_are_reported_not_guessed() {
    let result = ir();
    assert_eq!(result.dirty.len(), 1);
    assert!(result
        .display
        .ops
        .iter()
        .any(|op| matches!(op, Op::Unresolved(_))));
}

#[test]
fn epub_pages_partition_ops() {
    let result = ir();
    assert!(!result.pages.is_empty());
    let mut ops = 0u32;
    for page in &result.pages {
        assert_eq!(page.ops.start, ops);
        assert!(page.ops.len > 0);
        ops += page.ops.len;
    }
    assert_eq!(ops as usize, result.display.ops.len());
}

#[test]
fn epub_layout_is_deterministic() {
    let first = codec::to_bytes(&ir()).expect("encode");
    let second = codec::to_bytes(&ir()).expect("encode");
    assert_eq!(first, second);
}
