//! IR 1.4 新增字段的编解码：链接 op、字形样式、下划线、行盒索引、诊断条目。

use ebook_ir::{
    codec, Anchor, Dir, DisplayList, EdgesF, FontId, FontStyle, Glyph, GlyphRun, Issue, IssueCode,
    Limits, LineBox, Op, Page, PointF, RectF, Rgba8, Rotate, RunMap, SizeF, Span, Unresolved,
    UnresolvedReason, IR_VERSION,
};

fn ir() -> ebook_ir::LayoutResult {
    let glyphs = vec![Glyph {
        gid: 7,
        dx: 6.5,
        dy: 0.0,
        ch: 3,
    }];
    let ops = vec![
        Op::GlyphRun(GlyphRun {
            font: FontId(2),
            size: 12.0,
            color: Rgba8::BLACK,
            origin: PointF::new(10.0, 20.0),
            glyphs: Span::new(0, 1),
            text: String::from("x"),
            dir: Dir::Ltr,
            rotate: Rotate::None,
            weight: 700,
            style: FontStyle::Italic,
            underline: true,
        }),
        Op::Link {
            rect: RectF::new(10.0, 20.0, 6.5, 16.0),
            href: String::from("ch2.xhtml#n"),
        },
        Op::Unresolved(Unresolved {
            node: 4,
            rect: RectF::new(0.0, 0.0, 8.0, 8.0),
            reason: UnresolvedReason::ImagePending,
        }),
    ];
    let lines = vec![LineBox {
        rect: RectF::new(10.0, 20.0, 6.5, 16.0),
        baseline: 32.0,
        dir: Dir::Ltr,
        src: Span::new(5, 1),
        anchor: Some(Anchor {
            spine: 0,
            path: vec![1, 2],
            offset: 5,
        }),
        glyphs: Span::new(0, 1),
        node: 9,
    }];
    ebook_ir::LayoutResult {
        version: IR_VERSION,
        spine: 1,
        pages: vec![Page {
            index: 0,
            origin: PointF::new(0.0, 100.0),
            size: SizeF::new(80.0, 100.0),
            inset: EdgesF::all(8.0),
            ops: Span::new(0, 3),
            lines: Span::new(0, 1),
            anchor: None,
        }],
        display: DisplayList { ops, glyphs },
        runs: RunMap { lines },
        blocks: vec![],
        dirty: vec![],
        issues: vec![Issue {
            node: 4,
            code: IssueCode::MissingImage,
            detail: Some(String::from("image pending")),
        }],
        truncated: false,
    }
}

#[test]
fn new_fields_survive_the_byte_contract() {
    let ir = ir();
    let bytes = codec::to_bytes(&ir).expect("encode");
    assert_eq!(&bytes[..4], &codec::MAGIC);
    assert_eq!(&bytes[4..8], &[1, 0, 5, 0], "1.5 header");
    let back: ebook_ir::LayoutResult = codec::decode(&bytes).expect("decode");
    assert_eq!(back, ir);
    assert_eq!(back.version, IR_VERSION);
}

#[test]
fn link_op_carries_its_href_through_the_codec() {
    let back: ebook_ir::LayoutResult = codec::decode(&codec::to_bytes(&ir()).unwrap()).unwrap();
    let link = back
        .display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::Link { href, rect } => Some((href.as_str(), *rect)),
            _ => None,
        })
        .expect("link op");
    assert_eq!(link.0, "ch2.xhtml#n");
    assert_eq!(link.1, RectF::new(10.0, 20.0, 6.5, 16.0));
}

#[test]
fn limits_zero_fields_fall_back_to_default() {
    let zero = Limits {
        max_nodes: 0,
        max_depth: 0,
        max_text_bytes: 0,
        max_pages: 0,
        max_lines: 0,
        max_image_pixels: 0,
    };
    assert_eq!(zero.resolved(), Limits::default());
}

/// 解码后的 IR 可能来自缓存/跨线程字节，span 不再是排版层亲手写的。
/// 越界 span 必须退化成空切片，而不是 `debug` panic / `release` 回绕后
/// 仍然切到边界外。`release` 用的是 `panic = "abort"`，这里 panic 就是进程级。
#[test]
fn corrupt_spans_degrade_to_empty_instead_of_panicking() {
    let mut ir = ir();
    ir.pages[0].ops = Span::new(u32::MAX, 2);
    ir.pages[0].lines = Span::new(u32::MAX, 1);
    ir.runs.lines[0].glyphs = Span::new(u32::MAX, 1);
    ir.display.ops[0] = Op::GlyphRun(GlyphRun {
        font: FontId(0),
        size: 12.0,
        color: Rgba8::BLACK,
        origin: PointF::new(0.0, 0.0),
        glyphs: Span::new(u32::MAX, 1),
        text: String::from("x"),
        dir: Dir::Ltr,
        rotate: Rotate::None,
        weight: 400,
        style: FontStyle::Normal,
        underline: false,
    });

    // 先过一遍编解码，确认坏 span 本身也编得进去、解得出来。
    let bytes = codec::to_bytes(&ir).expect("encode");
    let back: ebook_ir::LayoutResult = codec::decode(&bytes).expect("decode");

    assert_eq!(Span::new(u32::MAX, 2).end(), u32::MAX, "end 必须饱和");
    assert!(back.display.op_slice(back.pages[0].ops).is_empty());
    assert!(back.display.glyph_slice(Span::new(u32::MAX, 1)).is_empty());
    // 查询路径（页/行/命中）也不能因为坏 span panic。
    assert!(back.page_lines(0).is_empty());
    assert!(back.page_of_line(0).is_none());
    assert!(back.hit_test(0, PointF::new(1.0, 1.0)).is_none());
    assert!(back
        .location_of(&Anchor {
            spine: 0,
            path: vec![1],
            offset: 0
        })
        .is_none());
}
