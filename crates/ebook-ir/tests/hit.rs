//! 命中测试与位置映射：不依赖排版器，直接构造 IR，验证坐标换算与往返回自洽。

use ebook_ir::{
    Anchor, Dir, DisplayList, EdgesF, FontId, FontStyle, Glyph, GlyphRun, LayoutResult, Limits,
    LineBox, Op, Page, PointF, RectF, Rgba8, Rotate, RunMap, SizeF, Span, IR_VERSION,
};

const LINE_H: f32 = 20.0;

fn glyph(byte: u32) -> Glyph {
    Glyph {
        gid: byte as u16,
        dx: 10.0,
        dy: 0.0,
        ch: byte,
    }
}

fn run(glyphs: Span, x: f32, y: f32) -> Op {
    Op::GlyphRun(GlyphRun {
        font: FontId(0),
        size: 10.0,
        color: Rgba8::BLACK,
        origin: PointF::new(x, y),
        glyphs,
        text: String::from("abcd"),
        dir: Dir::Ltr,
        rotate: Rotate::None,
        weight: 400,
        style: FontStyle::Normal,
        underline: false,
    })
}

fn line(x: f32, y: f32, w: f32, src: Span, glyphs: Span, offset: u32) -> LineBox {
    LineBox {
        rect: RectF::new(x, y, w, LINE_H),
        baseline: y + LINE_H * 0.8,
        dir: Dir::Ltr,
        src,
        anchor: Some(Anchor {
            spine: 0,
            path: alloc::vec![glyphs.start as u16],
            offset,
        }),
        glyphs,
        node: 7,
    }
}

fn page(index: u32, origin: PointF, inset: f32, ops: Span, lines: Span) -> Page {
    Page {
        index,
        origin,
        size: SizeF::new(100.0, 100.0),
        inset: EdgesF::all(inset),
        ops,
        lines,
        anchor: None,
    }
}

/// 两页：第一页 origin 为零，第二页 origin 与 inset 都非零。
/// 三个行盒，字形编号 0..9，每行 `ch` 从 0 重新计。
fn result() -> LayoutResult {
    // 每个字形记录它在本行 src 里的字节偏移：三行各自从 0 开始。
    let glyphs = (0..9).map(|i| glyph(i % 3)).collect::<Vec<_>>();
    let lines = alloc::vec![
        // 第 0 行：src 4 字节，ch = 0,1,2
        line(10.0, 10.0, 60.0, Span::new(0, 4), Span::new(0, 3), 0),
        // 第 1 行：另一个文本节点路径，ch = 0,1,2
        line(10.0, 40.0, 60.0, Span::new(4, 4), Span::new(3, 3), 0),
        // 第 2 行在第 2 页，流坐标仍在 400 附近
        line(24.0, 400.0, 60.0, Span::new(8, 4), Span::new(6, 3), 0),
    ];
    LayoutResult {
        version: IR_VERSION,
        spine: 3,
        pages: alloc::vec![
            page(0, PointF::ORIGIN, 0.0, Span::new(0, 2), Span::new(0, 2)),
            page(
                1,
                PointF::new(0.0, 400.0),
                24.0,
                Span::new(2, 1),
                Span::new(2, 1)
            ),
        ],
        display: DisplayList {
            ops: alloc::vec![
                run(Span::new(0, 3), 10.0, 26.0),
                run(Span::new(3, 3), 10.0, 56.0),
                run(Span::new(6, 3), 24.0, 416.0),
            ],
            glyphs,
        },
        runs: RunMap { lines },
        dirty: alloc::vec![],
        issues: alloc::vec![],
        truncated: false,
    }
}

#[test]
fn hit_test_inside_a_line_walks_the_glyphs() {
    let ir = result();
    let left = ir.hit_test(0, PointF::new(15.0, 15.0)).expect("hit");
    assert_eq!((left.page, left.line), (0, 0));
    assert_eq!(left.byte, 0);

    let middle = ir.hit_test(0, PointF::new(25.0, 15.0)).expect("hit");
    assert_eq!((middle.page, middle.line), (0, 0));
    assert_eq!(middle.byte, 1);

    let right = ir.hit_test(0, PointF::new(55.0, 15.0)).expect("hit");
    assert_eq!(right.byte, 2);

    // 行内不同 x → 不同字节；同一次点击必须落在同一行。
    assert_ne!(left.byte, middle.byte);
    assert_ne!(middle.byte, right.byte);
}

#[test]
fn hit_test_outside_every_line_is_none() {
    let ir = result();
    assert!(ir.hit_test(0, PointF::new(5.0, 5.0)).is_none());
    assert!(ir.hit_test(0, PointF::new(15.0, 900.0)).is_none());
    assert!(ir.hit_test(9, PointF::new(15.0, 15.0)).is_none());
}

#[test]
fn hit_test_accounts_for_page_origin_and_inset() {
    let ir = result();
    // 第 1 页 origin = (0, 400)，inset = 24：流坐标 (29, 405) 对应的页内坐标是 (53, 29)。
    let location = ir.hit_test(1, PointF::new(53.0, 29.0)).expect("hit");
    assert_eq!(location.page, 1);
    assert_eq!(location.line, 2);
    assert_eq!(location.byte, 0);

    // 直接用页内坐标喂 `line_at` 会命中**别的页**的行——这正是加 `hit_test` 要修的 bug。
    let wrong = ir
        .runs
        .line_at(PointF::new(53.0, 29.0))
        .expect("流坐标里确实有行");
    assert_ne!(wrong.rect, ir.runs.lines[2].rect);
    // 换算成流坐标才能命中第 2 页那一行。
    assert_eq!(
        ir.runs.line_at(PointF::new(29.0, 405.0)).map(|l| l.rect),
        Some(ir.runs.lines[2].rect)
    );

    // 页外（左上角负流坐标）不产生命中。
    assert!(ir.hit_test(1, PointF::new(0.0, 0.0)).is_none());
}

#[test]
fn anchor_at_and_location_of_round_trip() {
    let ir = result();
    for x in [15.0f32, 25.0, 55.0] {
        let point = PointF::new(x, 15.0);
        let location = ir.hit_test(0, point).expect("hit");
        let anchor = ir.anchor_at(0, point).expect("anchor");
        let back = ir.location_of(&anchor).expect("location");
        assert_eq!(back, location, "x = {x}");
    }
}

#[test]
fn location_of_clamps_past_the_end_of_a_line() {
    let ir = result();
    let anchor = Anchor {
        spine: 0,
        path: alloc::vec![0],
        offset: 11,
    };
    let location = ir.location_of(&anchor).expect("location");
    assert_eq!(location.page, 0);
    assert_eq!(location.line, 0);
    assert_eq!(location.byte, 4);

    // 别的 spine 不命中任何行。
    let other = Anchor {
        spine: 4,
        path: alloc::vec![0],
        offset: 0,
    };
    assert!(ir.location_of(&other).is_none());
}

#[test]
fn limits_defaults_are_all_non_zero_and_zero_means_default() {
    let limits = Limits::default();
    assert!(limits.max_nodes > 0);
    assert!(limits.max_depth > 0);
    assert!(limits.max_text_bytes > 0);
    assert!(limits.max_pages > 0);
    assert!(limits.max_lines > 0);
    assert!(limits.max_image_pixels > 0);

    let zero = Limits {
        max_nodes: 0,
        max_depth: 0,
        max_text_bytes: 0,
        max_pages: 0,
        max_lines: 0,
        max_image_pixels: 0,
    };
    assert_eq!(zero.resolved(), limits);

    let explicit = Limits {
        max_pages: 7,
        ..Limits::default()
    };
    assert_eq!(explicit.resolved().max_pages, 7);
}

// 测试里可以用 std，但构造 Vec 用 alloc 的写法与 crate 保持一致。
extern crate alloc;
