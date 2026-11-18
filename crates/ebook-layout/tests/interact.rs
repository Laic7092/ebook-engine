//! 交互层：命中、锚点、进度、链接、选区。
//!
//! 版面全是手搭的：12 段 4 字符的拉丁文本，页高 60、行高 20，所以一页 3 行、
//! 共 4 页；字宽 5.2（`Fixed` 的 0.52 × 10）、space 2.8、ascent 8、descent 2。
//! 写死的坐标来自下面这份 `build()`（PageConstraint 90×60、margin 0）：
//!
//! ```text
//! line i: rect (0, 20i, 20.8, 20)  src = [4i, 4i+4)  anchor path [0] offset 4i
//! page p: origin (0, 60p)  inset 0  lines [3p, 3p+3)
//! glyph j in a line: x in [5.2j, 5.2(j+1)), ch = j
//! ```
//!
//! margin 10 的变体只用于坐标换算：页内坐标 = 流坐标 + inset。

use ebook_ir::{
    Anchor, ComputedStyle, Dir, Display, FontId, FontSpec, LayoutResult, Len, LineHeight, Location,
    Node, NodeId, NodeKind, PointF, RectF, Rgba8, Span, StyledDoc, Tag,
};
use ebook_layout::{
    layout, LayoutEnv, NoImageSizes, PageConstraint, Reader, Selection, SelectionRect, TextMeasurer,
};

/// 字宽 5.2、space 2.8、ascent 8、descent 2：坐标全是 5.2 的整数倍，好对账。
struct Fixed;

impl TextMeasurer for Fixed {
    fn advance(&self, ch: char, _font: FontId, _size: f32) -> f32 {
        if ch == ' ' {
            2.8
        } else {
            5.2
        }
    }

    fn ascent(&self, _font: FontId, _size: f32) -> f32 {
        8.0
    }

    fn descent(&self, _font: FontId, _size: f32) -> f32 {
        2.0
    }
}

/// 不覆盖 'x'：字形被跳过、但前进宽度保留，于是在一行里留下可见的"洞"，
/// 用来验证多段选区与 RTL 视觉顺序。
struct Holey;

impl TextMeasurer for Holey {
    fn advance(&self, ch: char, _font: FontId, _size: f32) -> f32 {
        if ch == ' ' {
            2.8
        } else {
            5.2
        }
    }

    fn ascent(&self, _font: FontId, _size: f32) -> f32 {
        8.0
    }

    fn descent(&self, _font: FontId, _size: f32) -> f32 {
        2.0
    }

    fn covers(&self, ch: char, _font: FontId) -> bool {
        ch != 'x'
    }
}

fn block_style(dir: Dir) -> ComputedStyle {
    let mut style = ComputedStyle::default();
    style.display = Display::Block;
    style.font = FontSpec {
        font: FontId(0),
        size: 10.0,
        weight: 400,
        ..FontSpec::default()
    };
    style.line_height = LineHeight::Px(20.0);
    style.margin.top = Len::ZERO;
    style.margin.bottom = Len::ZERO;
    style.dir = dir;
    style
}

fn inline_style(dir: Dir) -> ComputedStyle {
    ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 10.0,
            weight: 400,
            ..FontSpec::default()
        },
        Rgba8::BLACK,
        dir,
    )
}

fn push(
    doc: &mut StyledDoc,
    kind: NodeKind,
    style: ComputedStyle,
    parent: Option<NodeId>,
) -> NodeId {
    doc.push_node(Node {
        kind,
        style,
        parent,
        first_child: 0,
        child_len: 0,
        src: Some(Anchor::default()),
        link: None,
    })
}

fn text(doc: &mut StyledDoc, body: &str, dir: Dir) -> NodeId {
    let start = doc.text.len() as u32;
    doc.text.push_str(body);
    push(
        doc,
        NodeKind::Text(Span::new(start, body.len() as u32)),
        inline_style(dir),
        None,
    )
}

fn set_kids(doc: &mut StyledDoc, parent: NodeId, kids: &[NodeId]) {
    doc.set_kids(parent, kids);
    for (index, &kid) in kids.iter().enumerate() {
        doc.node_mut(kid).parent = Some(parent);
        let offset = match doc.node(kid).kind {
            NodeKind::Text(span) => span.start,
            _ => 0,
        };
        doc.node_mut(kid).src = Some(Anchor {
            spine: 0,
            path: [index as u16].to_vec(),
            offset,
        });
    }
}

fn body(doc: &mut StyledDoc, dir: Dir) -> NodeId {
    let root = push(doc, NodeKind::Element(Tag::Body), block_style(dir), None);
    doc.root = root;
    root
}

/// 12 段、每段 4 字符；`dir` 决定行方向。段路径统一是 [0]（body 下第一棵子树），
/// 段内字节偏移靠 text 节点 `src.offset` 区分，所以行锚点 = path [0] + 原文偏移。
fn doc_12(dir: Dir) -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = body(&mut doc, dir);
    let letters = [
        "aaaa", "bbbb", "cccc", "dddd", "eeee", "ffff", "gggg", "hhhh", "iiii", "jjjj", "kkkk",
        "llll",
    ];
    let mut kids = Vec::new();
    for (index, word) in letters.iter().enumerate() {
        let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(dir), None);
        let t = text(&mut doc, word, dir);
        set_kids(&mut doc, p, &[t]);
        // 锚点路径按段唯一：path = [段号, 0]，offset 相对文本节点（行首字符）。
        doc.node_mut(t).src = Some(Anchor {
            spine: 0,
            path: vec![index as u16, 0],
            offset: 0,
        });
        kids.push(p);
    }
    set_kids(&mut doc, root, &kids);
    doc
}

/// 单段、可换行的文本；行锚点共用 path [0, 0]，靠 offset 区分行。
fn doc_wrap(body_text: &str, dir: Dir) -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = body(&mut doc, dir);
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(dir), None);
    let t = text(&mut doc, body_text, dir);
    set_kids(&mut doc, p, &[t]);
    doc.node_mut(t).src = Some(Anchor {
        spine: 0,
        path: vec![0, 0],
        offset: 0,
    });
    set_kids(&mut doc, root, &[p]);
    doc
}

/// 单段，文本由调用方给；用来测缺口与链接。
fn doc_one(body_text: &str, dir: Dir, link: Option<&str>) -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = body(&mut doc, dir);
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(dir), None);
    let a = push(
        &mut doc,
        NodeKind::Element(Tag::Span),
        inline_style(dir),
        None,
    );
    doc.node_mut(a).link = link.map(str::to_string);
    let t = text(&mut doc, body_text, dir);
    set_kids(&mut doc, a, &[t]);
    set_kids(&mut doc, p, &[a]);
    set_kids(&mut doc, root, &[p]);
    doc
}

/// 只有一个空 body 的文档。
fn doc_empty() -> StyledDoc {
    let mut doc = StyledDoc::default();
    body(&mut doc, Dir::Ltr);
    doc
}

fn build_with(doc: &StyledDoc, measurer: &dyn TextMeasurer, margin: f32) -> LayoutResult {
    let env = LayoutEnv {
        measurer,
        resources: &NoImageSizes,
    };
    let c = PageConstraint::new(90.0, 60.0).with_margin(ebook_ir::EdgesF::all(margin));
    layout(doc, &env, &c)
}

fn build(doc: &StyledDoc) -> LayoutResult {
    build_with(doc, &Fixed, 0.0)
}

fn loc(page: u32, line: u32, byte: u32) -> Location {
    Location { page, line, byte }
}

/// golden 断言：坐标写死，但按 f32 精度比较（15.6 与 20.8-5.2 的十进制写法不同）。
fn assert_rects(got: &[SelectionRect], want: &[(u32, RectF)]) {
    assert_eq!(got.len(), want.len(), "rect count: got {got:?}");
    for (g, (page, rect)) in got.iter().zip(want) {
        assert_eq!(g.page, *page, "page of {g:?}");
        let pairs = [
            (g.rect.x, rect.x, "x"),
            (g.rect.y, rect.y, "y"),
            (g.rect.w, rect.w, "w"),
            (g.rect.h, rect.h, "h"),
        ];
        for (a, b, name) in pairs {
            assert!((a - b).abs() < 1e-3, "{name}: got {a}, want {b} ({g:?})");
        }
    }
}

#[test]
fn layout_fixture_shape() {
    // 夹具自身的形状：4 页 × 3 行，坐标见文件头注释。后面的 golden 都依赖它。
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    assert_eq!(ir.pages.len(), 4);
    assert_eq!(ir.runs.lines.len(), 12);
    let line = &ir.runs.lines[0];
    assert_eq!(line.rect, RectF::new(0.0, 0.0, 20.8, 20.0));
    let page = ir.page(1).unwrap();
    assert_eq!(page.origin, PointF::new(0.0, 60.0));
    assert_eq!(page.lines, Span::new(3, 3));
}

#[test]
fn hit_page_start_and_line_start() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    // 页 0 左上角 = 行 0 行首。
    assert_eq!(reader.hit(0, PointF::new(0.0, 0.0)), Some(loc(0, 0, 0)));
    // 页 1 顶端 = 行 3 行首（页内坐标 y = 0）。
    assert_eq!(reader.hit(1, PointF::new(0.0, 0.0)), Some(loc(1, 3, 0)));
}

#[test]
fn hit_line_end_sticks_to_last_glyph() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    // 行盒右边缘落在最后一个字形的前进宽度内，`hit_test` 返回该字形（字节 3）。
    assert_eq!(reader.hit(0, PointF::new(20.8, 10.0)), Some(loc(0, 0, 3)));
    // 越过页/行范围时退回最近的行，但仍夹在本页，不会跳到下一页。
    assert_eq!(reader.hit(0, PointF::new(20.8, 59.9)), Some(loc(0, 2, 3)));
}

#[test]
fn hit_far_point_clamps_to_nearest_line() {
    // 页内坐标远在右下角（margin 10：内容 70×40，一页 2 行，换算有一次浮点加减）：
    // 仍命中本页最近的行。
    let doc = doc_12(Dir::Ltr);
    let ir = build_with(&doc, &Fixed, 10.0);
    assert_eq!(ir.page(0).unwrap().lines, Span::new(0, 2));
    let reader = Reader::new(&ir);
    assert_eq!(
        reader.hit(0, PointF::new(9999.0, 9999.0)),
        Some(loc(0, 1, 3))
    );
    assert_eq!(
        reader.hit(0, PointF::new(-9999.0, -9999.0)),
        Some(loc(0, 0, 0))
    );
}

#[test]
fn hit_out_of_range_is_none() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    assert_eq!(reader.hit(4, PointF::new(0.0, 0.0)), None);
    assert_eq!(reader.hit(1000, PointF::new(0.0, 0.0)), None);
    assert_eq!(reader.line_of(loc(0, 12, 0)), None);
    assert_eq!(reader.page_of_line(12), None);
    assert_eq!(reader.link_at(9, PointF::new(0.0, 0.0)), None);
}

#[test]
fn hit_uses_page_inset() {
    let doc = doc_12(Dir::Ltr);
    let ir = build_with(&doc, &Fixed, 10.0);
    let reader = Reader::new(&ir);
    // margin 10：行 0 在页内从 (10, 10) 开始，流坐标是 (0, 0)。
    assert_eq!(reader.hit(0, PointF::new(12.0, 14.0)), Some(loc(0, 0, 0)));
    let rects = reader.selection_rects(&reader.selection(loc(0, 0, 0), loc(0, 0, 1)));
    assert_rects(&rects, &[(0, RectF::new(10.0, 10.0, 5.2, 20.0))]);
}

#[test]
fn hit_anchor_locate_round_trip() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    // 行内。
    let hit = reader.hit(0, PointF::new(7.0, 10.0)).unwrap();
    assert_eq!(hit, loc(0, 0, 1));
    let anchor = reader.anchor_at(0, PointF::new(7.0, 10.0)).unwrap();
    assert_eq!(
        anchor,
        Anchor {
            spine: 0,
            path: vec![0, 0],
            offset: 1
        }
    );
    assert_eq!(reader.locate(&anchor), Some(hit));
    // 跨页：行 1 在页 0，行 3 在页 1。
    for (page, point, want) in [
        (0u32, PointF::new(12.0, 25.0), loc(0, 1, 2)),
        (1u32, PointF::new(12.0, 5.0), loc(1, 3, 2)),
    ] {
        let hit = reader.hit(page, point).unwrap();
        assert_eq!(hit, want);
        let anchor = reader.anchor_at(page, point).unwrap();
        assert_eq!(reader.locate(&anchor), Some(hit));
    }
}

#[test]
fn anchor_round_trip_across_lines_of_one_node() {
    // 30 字符、内容宽 90：一行 17 个字形（17 × 5.2 = 88.4），折成 2 行。
    let doc = doc_wrap("abcdefghijklmnopqrstuvwxyz0123", Dir::Ltr);
    let ir = build(&doc);
    assert_eq!(ir.runs.lines.len(), 2);
    let reader = Reader::new(&ir);
    assert_eq!(
        ir.runs.lines[0].anchor,
        Some(Anchor {
            spine: 0,
            path: vec![0, 0],
            offset: 0
        })
    );
    assert_eq!(
        ir.runs.lines[1].anchor,
        Some(Anchor {
            spine: 0,
            path: vec![0, 0],
            offset: 17
        })
    );
    // 第一行末 → 第二行首要能靠锚点区分开。
    let first = reader.anchor_at(0, PointF::new(88.0, 10.0)).unwrap();
    assert_eq!(
        reader.locate(&first),
        Some(reader.hit(0, PointF::new(88.0, 10.0)).unwrap())
    );
    // 第二行从原文第 17 个字节开始（一行 17 个字形）。断行处的绝对偏移有歧义
    // （行 0 末尾 == 行 1 行首），`location_of` 归到前一行末尾；行内位置则精确。
    let break_point = reader.anchor_at(0, PointF::new(2.0, 30.0)).unwrap();
    assert_eq!(break_point.offset, 17);
    assert_eq!(reader.locate(&break_point), Some(loc(0, 0, 17)));
    let second = reader.anchor_at(0, PointF::new(7.0, 30.0)).unwrap();
    assert_eq!(second.offset, 18);
    assert_eq!(reader.locate(&second), Some(loc(0, 1, 1)));
}

#[test]
fn empty_document_queries_are_empty() {
    let doc = doc_empty();
    let ir = build(&doc);
    assert!(ir.pages.is_empty());
    assert!(ir.runs.lines.is_empty());
    let reader = Reader::new(&ir);
    assert_eq!(reader.hit(0, PointF::new(0.0, 0.0)), None);
    assert_eq!(reader.progress(loc(0, 0, 0)), 0.0);
    assert_eq!(reader.page_progress(0), 0.0);
    assert_eq!(reader.clamp(loc(5, 5, 5)), Location::default());
    assert_eq!(reader.page_of_line(0), None);
    assert!(reader
        .selection_rects(&reader.selection(loc(0, 0, 0), loc(0, 0, 3)))
        .is_empty());
    let sel = Selection::new(loc(0, 0, 0), loc(0, 0, 3));
    assert_eq!(reader.selection_text(&doc, &sel), "");
    assert_eq!(reader.link_at(0, PointF::new(0.0, 0.0)), None);
    assert_eq!(reader.anchor_at(0, PointF::new(0.0, 0.0)), None);
    assert_eq!(reader.locate(&Anchor::default()), None);
}

#[test]
fn progress_is_monotone_and_continuous() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    let mut prev = -1.0f32;
    for (index, line) in ir.runs.lines.iter().enumerate() {
        let page = reader.page_of_line(index as u32).unwrap();
        for byte in 0..=line.src.len {
            let p = reader.progress(loc(page, index as u32, byte));
            assert!(
                p >= prev,
                "progress went backwards at line {index} byte {byte}: {p} < {prev}"
            );
            assert!((0.0..=1.0).contains(&p));
            prev = p;
        }
    }
    assert_eq!(prev, 1.0);
    // 页边界连续：上一页最后一行末尾 == 下一页第一行行首。
    for page in 1..ir.pages.len() as u32 {
        let lines = ir.page(page).unwrap().lines;
        let before = ir.page(page - 1).unwrap().lines;
        let end_of_before = reader.progress(loc(
            page - 1,
            before.end() - 1,
            ir.runs.lines[before.end() as usize - 1].src.len,
        ));
        let start_of_page = reader.progress(loc(page, lines.start, 0));
        assert_eq!(end_of_before, start_of_page);
        assert_eq!(start_of_page, reader.page_progress(page));
    }
    assert_eq!(reader.page_progress(0), 0.0);
    assert_eq!(reader.page_progress(99), 0.0);
    // 越界位置夹到最后：进度 1。
    assert_eq!(reader.progress(loc(99, 99, 999)), 1.0);
}

#[test]
fn clamp_pins_out_of_range_locations() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    assert_eq!(reader.clamp(loc(0, 100, 999)), loc(3, 11, 4));
    assert_eq!(reader.clamp(loc(0, 2, 99)), loc(0, 2, 4));
    // page 与 line 不一致时以行号为准。
    assert_eq!(reader.clamp(loc(3, 1, 0)), loc(0, 1, 0));
}

#[test]
fn selection_single_glyph_and_half_glyph_snaps() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    let one = reader.selection_rects(&reader.selection(loc(0, 0, 1), loc(0, 0, 2)));
    assert_rects(&one, &[(0, RectF::new(5.2, 0.0, 5.2, 20.0))]);
    // 字节 1..3 只覆盖字形 1、2；半选的字形整体计入。
    let half = reader.selection_rects(&reader.selection(loc(0, 0, 1), loc(0, 0, 3)));
    assert_rects(&half, &[(0, RectF::new(5.2, 0.0, 10.4, 20.0))]);
}

#[test]
fn selection_cross_line_and_cross_page() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    let cross_line = reader.selection_rects(&reader.selection(loc(0, 0, 2), loc(0, 1, 2)));
    assert_rects(
        &cross_line,
        &[
            (0, RectF::new(10.4, 0.0, 10.4, 20.0)),
            (0, RectF::new(0.0, 20.0, 10.4, 20.0)),
        ],
    );
    // 行 1 在页 0，行 7 在页 2：逐页返回页内坐标。
    let cross_page = reader.selection_rects(&reader.selection(loc(0, 1, 1), loc(2, 7, 3)));
    assert_rects(
        &cross_page,
        &[
            (0, RectF::new(5.2, 20.0, 15.6, 20.0)), // 行 1，字节 1..4
            (0, RectF::new(0.0, 40.0, 20.8, 20.0)), // 行 2，整行
            (1, RectF::new(0.0, 0.0, 20.8, 20.0)),  // 行 3，整行
            (1, RectF::new(0.0, 20.0, 20.8, 20.0)), // 行 4
            (1, RectF::new(0.0, 40.0, 20.8, 20.0)), // 行 5
            (2, RectF::new(0.0, 0.0, 20.8, 20.0)),  // 行 6
            (2, RectF::new(0.0, 20.0, 15.6, 20.0)), // 行 7，字节 0..3
        ],
    );
    // 整段（本例一段就是一行）＝整行盒。
    let whole = reader.selection_rects(&reader.selection(loc(0, 0, 0), loc(0, 0, 4)));
    assert_rects(&whole, &[(0, RectF::new(0.0, 0.0, 20.8, 20.0))]);
}

#[test]
fn selection_reversed_and_empty() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    let forward = reader.selection_rects(&reader.selection(loc(0, 0, 1), loc(1, 3, 2)));
    let backward = reader.selection_rects(&reader.selection(loc(1, 3, 2), loc(0, 0, 1)));
    assert_eq!(forward, backward);
    let sel = Selection::new(loc(0, 2, 4), loc(0, 0, 0));
    assert_eq!(sel.normalize(), (loc(0, 0, 0), loc(0, 2, 4)));
    assert!(!sel.is_empty());
    assert!(Selection::new(loc(0, 0, 1), loc(0, 0, 1)).is_empty());
    // 反向且同行：normalize 后两端相同 → 空。
    assert!(Selection::new(loc(0, 0, 1), loc(0, 0, 1)).is_empty());
    assert!(reader
        .selection_rects(&reader.selection(loc(0, 0, 2), loc(0, 0, 2)))
        .is_empty());
    // 越界的终点夹到最后一行行首，该行字节区间为空 → 不产生矩形，只覆盖前 11 行。
    let sel = reader.selection(loc(0, 0, 0), loc(9, 99, 0));
    assert_eq!(reader.selection_rects(&sel).len(), 11);
}

#[test]
fn selection_text_is_logical_bytes() {
    let doc = doc_12(Dir::Ltr);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    let one_line = reader.selection(loc(0, 0, 1), loc(0, 0, 3));
    assert_eq!(reader.selection_text(&doc, &one_line), "aa");
    let two_line = reader.selection(loc(0, 0, 2), loc(0, 1, 1));
    assert_eq!(reader.selection_text(&doc, &two_line), "aa\nb");
    let whole = reader.selection(loc(0, 0, 0), loc(0, 11, 4));
    assert_eq!(
        reader.selection_text(&doc, &whole),
        "aaaa\nbbbb\ncccc\ndddd\neeee\nffff\ngggg\nhhhh\niiii\njjjj\nkkkk\nllll"
    );
}

#[test]
fn selection_text_snaps_to_char_boundaries() {
    // "中文" 各 3 字节：Location.byte 落在字符中间时向外扩，别丢字。
    let doc = doc_one("中文", Dir::Ltr, None);
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    let sel = reader.selection(loc(0, 0, 1), loc(0, 0, 4));
    assert_eq!(reader.selection_text(&doc, &sel), "中文");
    assert_rects(
        &reader.selection_rects(&sel),
        &[(0, RectF::new(0.0, 0.0, 10.4, 20.0))],
    );
}

#[test]
fn selection_gap_and_rtl_visual_order() {
    // "axax" + 不覆盖 'x' 的度量：字形在 x=0 与 x=10.4，中间是洞。
    let doc = doc_one("axax", Dir::Ltr, None);
    let ir = build_with(&doc, &Holey, 0.0);
    let reader = Reader::new(&ir);
    let sel = reader.selection(loc(0, 0, 0), loc(0, 0, 4));
    assert_rects(
        &reader.selection_rects(&sel),
        &[
            (0, RectF::new(0.0, 0.0, 5.2, 20.0)),
            (0, RectF::new(10.4, 0.0, 5.2, 20.0)),
        ],
    );
    assert_eq!(reader.selection_text(&doc, &sel), "axax");

    // 同一份文本换 RTL：矩形按视觉从右到左返回，Location.byte 仍是逻辑序。
    let doc = doc_one("axax", Dir::Rtl, None);
    let ir = build_with(&doc, &Holey, 0.0);
    let reader = Reader::new(&ir);
    assert_eq!(ir.runs.lines[0].dir, Dir::Rtl);
    let sel = reader.selection(loc(0, 0, 0), loc(0, 0, 4));
    assert_rects(
        &reader.selection_rects(&sel),
        &[
            (0, RectF::new(10.4, 0.0, 5.2, 20.0)),
            (0, RectF::new(0.0, 0.0, 5.2, 20.0)),
        ],
    );
    // 字节 2 是逻辑序的第三个字符（第二个 'a'），落在右侧那段。
    let tail = reader.selection(loc(0, 0, 2), loc(0, 0, 3));
    assert_rects(
        &reader.selection_rects(&tail),
        &[(0, RectF::new(10.4, 0.0, 5.2, 20.0))],
    );
    assert_eq!(reader.selection_text(&doc, &tail), "a");
}

#[test]
fn link_at_hits_link_rect_only() {
    let doc = doc_one("aaaa", Dir::Ltr, Some("ch2.xhtml#n"));
    let ir = build(&doc);
    let reader = Reader::new(&ir);
    let links: Vec<RectF> = ir
        .display
        .ops
        .iter()
        .filter_map(|op| match op {
            ebook_ir::Op::Link { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(links, vec![RectF::new(0.0, 0.0, 20.8, 20.0)]);
    assert_eq!(
        reader.link_at(0, PointF::new(10.0, 10.0)),
        Some("ch2.xhtml#n")
    );
    // 行 0 之外（y = 30）没有链接。
    assert_eq!(reader.link_at(0, PointF::new(10.0, 30.0)), None);
    let plain = doc_one("aaaa", Dir::Ltr, None);
    let ir = build(&plain);
    let reader = Reader::new(&ir);
    assert_eq!(reader.link_at(0, PointF::new(10.0, 10.0)), None);
}
