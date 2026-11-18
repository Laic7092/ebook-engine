use ebook_ir::{
    codec, Break, BreakPolicy, ComputedStyle, Dir, Display, FontId, LayoutResult, Len,
    LineHeight, Node, NodeKind, Rgba8, Sides, Span, StyledDoc, Tag,
};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(0x5851_f42d_4c95_7f2d)
            .wrapping_add(0x1405_7b7e_f767_814f);
        self.0 >> 17
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 9973) as f32 / 9973.0
    }

    fn pick(&mut self) -> f32 {
        self.unit()
    }
}

const WORDS: [&str; 12] = [
    "the", "winter", "road", "lantern", "ice", "harbor", "brass", "letter", "shelf", "stove",
    "ridge", "wind",
];

struct Builder {
    doc: StyledDoc,
    rng: Rng,
}

impl Builder {
    fn new(seed: u64) -> Self {
        let mut doc = StyledDoc::default();
        doc.meta.spine = (seed % 7) as u32;
        let root = doc.push_node(Node {
            kind: NodeKind::Element(Tag::Body),
            style: ComputedStyle::default(),
            parent: None,
            first_child: 0,
            child_len: 0,
            src: None,
        });
        doc.root = root;
        Self { doc, rng: Rng::new(seed) }
    }

    fn span(&mut self) -> Span {
        let start = self.doc.text.len() as u32;
        let count = 3 + self.rng.below(9);
        for index in 0..count {
            if index > 0 {
                self.doc.text.push(' ');
            }
            self.doc.text.push_str(WORDS[self.rng.below(WORDS.len() as u32) as usize]);
        }
        Span::new(start, self.doc.text.len() as u32 - start)
    }

    fn inline(&mut self, parent: u32, size: f32, weight: u16) {
        let span = self.span();
        let style = ComputedStyle::inline(
            ebook_ir::doc::FontSpec { font: FontId(0), size, weight, ..Default::default() },
            Rgba8::BLACK,
            Dir::Ltr,
        );
        let id = self.doc.push_node(Node {
            kind: NodeKind::Text(span),
            style,
            parent: Some(parent),
            first_child: 0,
            child_len: 0,
            src: None,
        });
        let mut kids: Vec<u32> = self.doc.kids(parent).to_vec();
        kids.push(id);
        self.doc.set_kids(parent, &kids);
    }

    fn block(&mut self, parent: u32, tag: Tag) -> u32 {
        let size = 10.0 + self.rng.unit() * 12.0;
        let mut style = ComputedStyle::default();
        style.display = Display::Block;
        style.font = ebook_ir::doc::FontSpec { font: FontId(0), size, weight: 400, ..Default::default() };
        style.line_height = if self.rng.below(2) == 0 {
            LineHeight::Normal
        } else {
            LineHeight::Mul(1.1 + self.rng.unit())
        };
        style.margin.top = Len::Em(self.rng.pick());
        style.margin.bottom = Len::Em(self.rng.pick());
        style.brk.orphans = 1 + self.rng.below(4) as u16;
        style.brk.widows = 1 + self.rng.below(4) as u16;
        if self.rng.below(5) == 0 {
            style.brk.before = Break::Always;
        }
        if self.rng.below(9) == 0 {
            style.height = Len::Px(20.0 + self.rng.unit() * 200.0);
        }
        let id = self.doc.push_node(Node {
            kind: NodeKind::Element(tag),
            style,
            parent: Some(parent),
            first_child: 0,
            child_len: 0,
            src: None,
        });
        let mut kids: Vec<u32> = self.doc.kids(parent).to_vec();
        kids.push(id);
        self.doc.set_kids(parent, &kids);
        if self.rng.below(8) == 0 {
            let mut span_style = ComputedStyle::default();
            span_style.display = Display::None;
            let hidden = self.doc.push_node(Node {
                kind: NodeKind::Element(Tag::Div),
                style: span_style,
                parent: Some(id),
                first_child: 0,
                child_len: 0,
                src: None,
            });
            self.doc.set_kids(id, &[hidden]);
            return id;
        }
        let pieces = 1 + self.rng.below(4);
        for _ in 0..pieces {
            self.inline(id, size, 400);
        }
        id
    }

    fn finish(mut self) -> StyledDoc {
        let root = self.doc.root;
        let blocks = 1 + self.rng.below(6);
        for index in 0..blocks {
            let tag = if index == 0 && self.rng.below(2) == 0 { Tag::H1 } else { Tag::P };
            self.block(root, tag);
        }
        self.doc
    }
}

fn check(doc: &StyledDoc, ir: &LayoutResult) -> Result<(), String> {
    let lines = ir.runs.lines.len() as u32;
    if lines > 0 && ir.pages.is_empty() {
        return Err(String::from("lines without pages"));
    }
    let mut op_cursor = 0;
    let mut line_cursor = 0;
    let mut prev_origin = f32::NEG_INFINITY;
    for page in &ir.pages {
        if page.origin.y < prev_origin - 1e-3 {
            return Err(format!("page {} origin went backwards", page.index));
        }
        prev_origin = page.origin.y;
        if page.ops.start != op_cursor {
            return Err(format!("page {} ops at {} not {}", page.index, page.ops.start, op_cursor));
        }
        op_cursor += page.ops.len;
        if page.lines.start != line_cursor {
            return Err(format!(
                "page {} lines at {} not {}",
                page.index, page.lines.start, line_cursor
            ));
        }
        line_cursor += page.lines.len;
        if page.lines.len == 0 {
            return Err(format!("page {} carries no lines", page.index));
        }
        if page.origin.x.abs() > 1e-3 {
            return Err(format!("page {} origin x {}", page.index, page.origin.x));
        }
        let top = page.origin.y;
        let bottom = page.origin.y + page.size.h + 1e-3;
        for line in &ir.runs.lines[page.lines.start as usize..][..page.lines.len as usize] {
            if line.rect.y < top - 1e-3 {
                return Err(format!("page {} line above top {} < {}", page.index, line.rect.y, top));
            }
            if line.rect.y + line.rect.h > bottom {
                return Err(format!(
                    "page {} line below bottom {} > {}",
                    page.index,
                    line.rect.y + line.rect.h,
                    bottom
                ));
            }
            if line.rect.x < -1e-3 {
                return Err(format!("page {} line x {}", page.index, line.rect.x));
            }
        }
    }
    if op_cursor != ir.display.ops.len() as u32 {
        return Err(format!("pages cover {} of {} ops", op_cursor, ir.display.ops.len()));
    }
    if line_cursor != lines {
        return Err(format!("pages cover {} of {} lines", line_cursor, lines));
    }
    let mut prev = f32::NEG_INFINITY;
    for line in &ir.runs.lines {
        if line.rect.y < prev - 1e-3 {
            return Err(String::from("line order went backwards"));
        }
        prev = line.rect.y;
    }
    for window in ir.runs.lines.windows(2) {
        if window[0].src.start > window[1].src.start {
            return Err(String::from("source order went backwards"));
        }
    }
    if doc.nodes.is_empty() {
        return Err(String::from("doc lost its nodes"));
    }
    for op in &ir.display.ops {
        let bytes = codec::to_bytes(op).map_err(|_| String::from("op encode"))?;
        if bytes.len() < codec::HEADER_LEN {
            return Err(String::from("op encoded short"));
        }
    }
    Ok(())
}

#[test]
fn random_documents_hold_invariants() {
    let sizes = [(320.0, 480.0), (600.0, 800.0), (900.0, 1200.0)];
    let mut total_lines = 0;
    let mut total_pages = 0;
    let mut multi_page = 0;
    for seed in 1..60u64 {
        let doc = Builder::new(seed).finish();
        for (width, height) in sizes {
            let constraint = PageConstraint::new(width, height).with_margin(ebook_ir::EdgesF::all(24.0));
            let env = LayoutEnv { measurer: &StubMeasurer, resources: &NoImageSizes };
            let first = layout(&doc, &env, &constraint);
            let second = layout(&doc, &env, &constraint);
            let a = codec::to_bytes(&first).expect("encode");
            let b = codec::to_bytes(&second).expect("encode");
            assert_eq!(a, b, "layout is not deterministic for seed {seed}");
            let decoded: LayoutResult = codec::decode(&a).expect("decode");
            assert_eq!(decoded, first, "codec round trip changed the layout for seed {seed}");
            if let Err(problem) = check(&doc, &first) {
                panic!("seed {seed} at {width}x{height}: {problem}");
            }
            total_lines += first.runs.lines.len();
            total_pages += first.pages.len();
            if first.pages.len() > 1 {
                multi_page += 1;
            }
        }
    }
    assert!(total_lines > 500, "corpus too thin: {total_lines} lines");
    assert!(multi_page > 20, "corpus never paginates: {multi_page} multi-page cases");
    assert!(total_pages > 100, "corpus too thin: {total_pages} pages");
}

#[test]
fn degenerate_documents_stay_sane() {
    let mut doc = StyledDoc::default();
    let root = doc.push_node(Node {
        kind: NodeKind::Element(Tag::Root),
        style: ComputedStyle::default(),
        parent: None,
        first_child: 0,
        child_len: 0,
        src: None,
    });
    doc.root = root;
    let env = LayoutEnv { measurer: &StubMeasurer, resources: &NoImageSizes };
    for size in [(1.0, 1.0), (0.5, 400.0), (400.0, 0.5), (1e5, 1e5)] {
        let constraint = PageConstraint::new(size.0, size.1);
        let ir = layout(&doc, &env, &constraint);
        assert!(ir.pages.len() <= 1, "empty doc produced {} pages", ir.pages.len());
        assert!(codec::to_bytes(&ir).is_ok());
    }
    let _ = (BreakPolicy::default(), Sides::all(Len::ZERO));
}
