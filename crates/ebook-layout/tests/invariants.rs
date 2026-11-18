use ebook_format::MemorySource;
use ebook_ir::{codec, EdgesF, LayoutResult, Op, StyledDoc};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

fn doc() -> StyledDoc {
    let mut src = MemorySource::new();
    src.insert_text("demo.txt", include_str!("../../../fixtures/demo.txt"));
    let opened = ebook_format::open(&src).expect("format");
    opened.chapter(&src, 0).expect("chapter")
}

fn ir(width: f32, height: f32) -> LayoutResult {
    let doc = doc();
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(width, height).with_margin(EdgesF::all(24.0));
    layout(&doc, &env, &constraint)
}

#[test]
fn pages_partition_ops_and_lines() {
    let result = ir(600.0, 800.0);
    assert!(!result.pages.is_empty());

    let mut ops = 0u32;
    let mut lines = 0u32;
    for (i, page) in result.pages.iter().enumerate() {
        assert_eq!(page.index as usize, i);
        assert_eq!(page.ops.start, ops);
        assert_eq!(page.lines.start, lines);
        assert!(page.ops.len > 0);
        ops += page.ops.len;
        lines += page.lines.len;
    }
    assert_eq!(ops as usize, result.display.ops.len());
    assert_eq!(lines as usize, result.runs.lines.len());
}

#[test]
fn lines_stay_inside_their_page() {
    let result = ir(600.0, 800.0);
    for page in &result.pages {
        for line in &result.runs.lines[page.lines.as_range()] {
            assert!(line.rect.y >= page.origin.y - 0.01);
            assert!(
                line.rect.bottom() <= page.origin.y + page.size.h + 0.01,
                "line {:?} escapes page {:?}",
                line.rect,
                page.origin
            );
        }
    }
}

#[test]
fn lines_are_monotonic() {
    let result = ir(600.0, 800.0);
    for pair in result.runs.lines.windows(2) {
        assert!(pair[0].rect.y <= pair[1].rect.y);
    }
}

#[test]
fn source_spans_are_inside_buffers() {
    let doc = doc();
    let result = ir(600.0, 800.0);
    for line in &result.runs.lines {
        assert!(line.src.end() as usize <= doc.text.len());
    }
    for op in &result.display.ops {
        if let Op::GlyphRun(run) = op {
            assert!(run.glyphs.end() as usize <= result.display.glyphs.len());
        }
    }
}

#[test]
fn encoding_is_deterministic_and_idempotent() {
    let first = codec::to_bytes(&ir(600.0, 800.0)).expect("encode");
    let second = codec::to_bytes(&ir(600.0, 800.0)).expect("encode");
    assert_eq!(first, second);

    let decoded: LayoutResult = codec::decode(&first).expect("decode");
    let again = codec::to_bytes(&decoded).expect("encode");
    assert_eq!(first, again);
}

#[test]
fn layout_is_width_sensitive() {
    let narrow = ir(360.0, 800.0);
    let wide = ir(600.0, 800.0);
    assert!(narrow.runs.lines.len() > wide.runs.lines.len());
}
