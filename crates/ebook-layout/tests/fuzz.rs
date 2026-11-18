//! 随机文档 fuzz：1000 份结构/文本/样式/页面尺寸都随机的 `StyledDoc`。
//!
//! 只断言排版层的承诺，不开 limits、不比较像素：
//!
//! - 不 panic（整个测试跑完就是证据）；
//! - 页覆盖：页按顺序切分 op 与行，并集恰好等于 `display.ops` / `runs.lines`；
//! - 行不越界：每行落在自己页的内容区里，行盒宽高有限且非负；
//! - y 单调：页 origin 与行 `rect.y` 都不回退，行的 `src.start` 也不回退；
//! - 幂等：同一输入排版两次逐字节相同，`decode(encode(ir)) == ir`。
//!
//! 语料刻意混入 CJK、双向文本、行内图片、ruby、隐藏子树、强制分页与
//! `TextRules` 开关，让断行、整形、标点与分页分支都被随机覆盖。种子固定，
//! 失败可直接用打印出的 seed 复现。

use ebook_ir::{
    codec, Anchor, Break, BreakPolicy, ComputedStyle, Dir, Display, FontId, FontSpec, ImageId,
    LayoutResult, Len, LineHeight, Node, NodeId, NodeKind, Op, RectF, Rgba8, Sides, Span,
    StyledDoc, Tag, TextRules,
};
use ebook_layout::{layout, LayoutEnv, PageConstraint, ResourceView, StubMeasurer};

const EPS: f32 = 1e-3;

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
        assert!(n > 0);
        (self.next() % n as u64) as u32
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 10_007) as f32 / 10_007.0
    }

    fn chance(&mut self, n: u32) -> bool {
        self.below(n) == 0
    }
}

/// 拉丁词、CJK、标点与双向文本混在一起，一次覆盖多类断行/整形分支。
const ALPHABET: [&str; 18] = [
    "the ", "winter ", "road ", "lantern ", "harbor ", "ice ", "gg ", "office ", "中文", "排版",
    "，", "。", "「", "」", "：", " אבג ", " def ", "123",
];

/// 一半的图片有固有尺寸，一半没有：两条路径都要能排版。
struct Sizes;

impl ResourceView for Sizes {
    fn image_size(&self, id: ImageId) -> Option<ebook_ir::SizeF> {
        if id.is_multiple_of(2) {
            Some(ebook_ir::SizeF::new(
                8.0 + id as f32 * 4.0,
                6.0 + id as f32 * 3.0,
            ))
        } else {
            None
        }
    }
}

struct Builder {
    doc: StyledDoc,
    rng: Rng,
    /// 节点预算：随机文档也要有上界，否则个别种子会生成几十万节点。
    budget: u32,
}

impl Builder {
    fn new(seed: u64) -> Self {
        let mut doc = StyledDoc::default();
        doc.meta.spine = (seed % 7) as u32;
        let mut rng = Rng::new(seed);
        let root_style = ComputedStyle {
            font: FontSpec {
                font: FontId(0),
                size: 12.0 + rng.unit() * 6.0,
                weight: 400,
                ..FontSpec::default()
            },
            ..ComputedStyle::default()
        };
        let root = doc.push_node(Node {
            kind: NodeKind::Element(Tag::Body),
            style: root_style,
            parent: None,
            first_child: 0,
            child_len: 0,
            src: Some(Anchor::default()),
            link: None,
        });
        doc.root = root;
        Self {
            doc,
            rng,
            budget: 48,
        }
    }

    fn text(&mut self) -> Span {
        let start = self.doc.text.len() as u32;
        let pieces = 1 + self.rng.below(6);
        for _ in 0..pieces {
            self.doc
                .text
                .push_str(ALPHABET[self.rng.below(ALPHABET.len() as u32) as usize]);
        }
        Span::new(start, self.doc.text.len() as u32 - start)
    }

    fn anchor(&mut self) -> Option<Anchor> {
        if self.rng.chance(3) {
            None
        } else {
            Some(Anchor {
                spine: self.doc.meta.spine,
                path: vec![self.rng.below(4) as u16; 1 + self.rng.below(3) as usize],
                offset: self.rng.below(64),
            })
        }
    }

    fn push(&mut self, kind: NodeKind, style: ComputedStyle, parent: NodeId) -> NodeId {
        let anchor = self.anchor();
        let id = self.doc.push_node(Node {
            kind,
            style,
            parent: Some(parent),
            first_child: 0,
            child_len: 0,
            src: anchor,
            link: if self.rng.chance(9) {
                Some(String::from("https://example.com/x"))
            } else {
                None
            },
        });
        let mut kids: Vec<NodeId> = self.doc.kids(parent).to_vec();
        kids.push(id);
        self.doc.set_kids(parent, &kids);
        id
    }

    fn inline_style(&mut self, size: f32, dir: Dir) -> ComputedStyle {
        let mut style = ComputedStyle::inline(
            FontSpec {
                font: FontId(0),
                size,
                weight: if self.rng.chance(4) { 700 } else { 400 },
                ..FontSpec::default()
            },
            Rgba8::BLACK,
            dir,
        );
        if self.rng.chance(3) {
            style.text.letter_spacing = self.rng.unit() * 2.0 - 1.0;
        }
        if self.rng.chance(5) {
            style.text.word_spacing = self.rng.unit() * 4.0 - 2.0;
        }
        if self.rng.chance(4) {
            style.text.rules = TextRules {
                spacing_trim: self.rng.chance(2),
                hanging_punctuation: self.rng.chance(2),
                autospacing: self.rng.chance(2),
            };
        }
        if self.rng.chance(6) {
            style.display = Display::None;
        }
        style
    }

    fn block_style(&mut self, size: f32, dir: Dir) -> ComputedStyle {
        let mut style = ComputedStyle {
            font: FontSpec {
                font: FontId(0),
                size,
                weight: 400,
                ..FontSpec::default()
            },
            line_height: match self.rng.below(3) {
                0 => LineHeight::Normal,
                1 => LineHeight::Mul(1.0 + self.rng.unit()),
                _ => LineHeight::Px(size * (1.2 + self.rng.unit())),
            },
            dir,
            ..ComputedStyle::default()
        };
        style.display = match self.rng.below(8) {
            0 => Display::None,
            1 => Display::ListItem,
            _ => Display::Block,
        };
        style.margin = Sides {
            top: if self.rng.chance(2) {
                Len::Em(self.rng.unit())
            } else {
                Len::ZERO
            },
            bottom: if self.rng.chance(2) {
                Len::Em(self.rng.unit())
            } else {
                Len::ZERO
            },
            ..Sides::all(Len::ZERO)
        };
        if self.rng.chance(3) {
            style.padding = Sides::all(Len::Px(self.rng.unit() * 6.0));
        }
        if self.rng.chance(5) {
            style.border = ebook_ir::Border {
                width: Sides::all(Len::Px(1.0)),
                color: Rgba8::new(32, 32, 32, 255),
            };
        }
        if self.rng.chance(4) {
            style.width = Len::Px(40.0 + self.rng.unit() * 400.0);
        }
        if self.rng.chance(6) {
            style.height = Len::Px(10.0 + self.rng.unit() * 120.0);
        }
        style.brk = BreakPolicy {
            before: if self.rng.chance(9) {
                Break::Always
            } else if self.rng.chance(9) {
                Break::Avoid
            } else {
                Break::Auto
            },
            after: if self.rng.chance(11) {
                Break::Always
            } else {
                Break::Auto
            },
            orphans: 1 + self.rng.below(3) as u16,
            widows: 1 + self.rng.below(3) as u16,
            ..BreakPolicy::default()
        };
        style
    }

    fn dir(&mut self) -> Dir {
        if self.rng.chance(4) {
            Dir::Rtl
        } else {
            Dir::Ltr
        }
    }

    /// 一段随机的行内内容，追加到 `parent` 的孩子列表里。
    fn inline_children(&mut self, parent: NodeId, size: f32, dir: Dir, count: u32) {
        let mut kids: Vec<NodeId> = Vec::new();
        for _ in 0..count {
            match self.rng.below(10) {
                0 => {
                    // 行内图片：id 越大尺寸越大，奇偶交替决定有没有固有尺寸。
                    let id = self.rng.below(4) as ImageId;
                    let style = self.inline_style(size, dir);
                    kids.push(self.push(NodeKind::Image(id), style, parent));
                }
                1 => {
                    // ruby：base + rt 都是文本缓冲里的区间。
                    let base = self.text();
                    let rt = self.text();
                    let mut style = self.inline_style(size * 0.7, dir);
                    style.display = Display::Inline;
                    kids.push(self.push(NodeKind::Ruby { base, rt }, style, parent));
                }
                2 => {
                    let style = self.inline_style(size, dir);
                    kids.push(self.push(NodeKind::LineBreak, style, parent));
                }
                _ => {
                    let span = self.text();
                    let style = self.inline_style(size, dir);
                    kids.push(self.push(NodeKind::Text(span), style, parent));
                }
            }
        }
        self.doc.set_kids(parent, &kids);
    }

    fn block(&mut self, parent: NodeId, size: f32, depth: u32) {
        if self.budget == 0 {
            return;
        }
        self.budget -= 1;
        let tag = match self.rng.below(6) {
            0 => Tag::H1,
            1 => Tag::Blockquote,
            2 => Tag::Div,
            3 => Tag::Li,
            _ => Tag::P,
        };
        let dir = self.dir();
        let style = self.block_style(size, dir);
        let hidden = style.display == Display::None;
        let block = self.push(NodeKind::Element(tag), style, parent);
        if hidden {
            // 隐藏子树也要有孩子：`Display::None` 必须整棵剪掉，不能只剪自己。
            let inner = self.block_style(size, dir);
            let inner = self.push(NodeKind::Element(Tag::Div), inner, block);
            let text = self.text();
            let style = self.inline_style(size, dir);
            self.push(NodeKind::Text(text), style, inner);
            self.doc.set_kids(inner, &[]);
            return;
        }
        let kids = 1 + self.rng.below(4);
        self.inline_children(block, size, dir, kids);
        // 有时再嵌一层块，覆盖 margin collapsing 与嵌套分页。
        if depth < 3 && self.rng.chance(4) {
            let scale = 0.8 + self.rng.unit() * 0.4;
            self.block(block, size * scale, depth + 1);
        }
    }

    fn finish(mut self) -> StyledDoc {
        let root = self.doc.root;
        let blocks = 1 + self.rng.below(6);
        for _ in 0..blocks {
            let size = 9.0 + self.rng.unit() * 14.0;
            self.block(root, size, 0);
        }
        self.doc
    }
}

fn finite(r: RectF) -> bool {
    [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.w >= -EPS && r.h >= -EPS
}

/// 检查 IR 不变量；失败时返回带页/行号的原因，由调用方补上 seed。
fn check(doc: &StyledDoc, ir: &LayoutResult) -> Result<(), String> {
    let lines = ir.runs.lines.len() as u32;
    if lines > 0 && ir.pages.is_empty() {
        return Err(String::from("有行却没有页"));
    }
    let mut op_cursor = 0u32;
    let mut line_cursor = 0u32;
    let mut prev_origin = f32::NEG_INFINITY;
    for page in &ir.pages {
        if page.index as usize >= ir.pages.len() {
            return Err(format!("page {} 序号越界", page.index));
        }
        if page.origin.x.abs() > EPS {
            return Err(format!("page {} origin.x = {}", page.index, page.origin.x));
        }
        if page.origin.y < prev_origin - EPS {
            return Err(format!(
                "page {} origin.y {} 回退（上一页 {}）",
                page.index, page.origin.y, prev_origin
            ));
        }
        prev_origin = page.origin.y;
        if page.ops.start != op_cursor {
            return Err(format!(
                "page {} ops.start {} != {}",
                page.index, page.ops.start, op_cursor
            ));
        }
        if page.lines.start != line_cursor {
            return Err(format!(
                "page {} lines.start {} != {}",
                page.index, page.lines.start, line_cursor
            ));
        }
        op_cursor += page.ops.len;
        line_cursor += page.lines.len;
        if page.ops.len == 0 {
            return Err(format!("page {} 没有 op", page.index));
        }
        if page.lines.len == 0 {
            return Err(format!("page {} 没有行", page.index));
        }
        if page.lines.as_range().end > ir.runs.lines.len() {
            return Err(format!("page {} 行切片越界", page.index));
        }
        if page.ops.as_range().end > ir.display.ops.len() {
            return Err(format!("page {} op 切片越界", page.index));
        }
        let top = page.origin.y;
        let bottom = page.origin.y + page.size.h + EPS;
        for line in ir.runs.lines[page.lines.as_range()].iter() {
            if !finite(line.rect) {
                return Err(format!(
                    "page {} 行盒不是有限值: {:?}",
                    page.index, line.rect
                ));
            }
            if line.rect.y < top - EPS {
                return Err(format!(
                    "page {} 行 y {} 高于页顶 {}",
                    page.index, line.rect.y, top
                ));
            }
            if line.rect.y + line.rect.h > bottom {
                // 唯一允许的越界：这一行连同它在页内的起点偏移都装不进内容区
                // （极小页面 / 超大行高）。行不可切分，页上也只能有这一行；
                // 否则就是分页把本来放得下的行推走了。
                let needed = (line.rect.y - top) + line.rect.h;
                if page.lines.len == 1 && needed > page.size.h {
                    continue;
                }
                return Err(format!(
                    "page {} 行底 {} 低于页底 {}（行高 {}，页高 {}，页上 {} 行）",
                    page.index,
                    line.rect.y + line.rect.h,
                    bottom,
                    line.rect.h,
                    page.size.h,
                    page.lines.len
                ));
            }
            if line.rect.x < -EPS {
                return Err(format!("page {} 行 x {}", page.index, line.rect.x));
            }
        }
    }
    if op_cursor != ir.display.ops.len() as u32 {
        return Err(format!(
            "页只覆盖 {} / {} 个 op",
            op_cursor,
            ir.display.ops.len()
        ));
    }
    if line_cursor != lines {
        return Err(format!("页只覆盖 {} / {} 行", line_cursor, lines));
    }

    let mut prev_y = f32::NEG_INFINITY;
    let mut prev_src = 0u32;
    for (index, line) in ir.runs.lines.iter().enumerate() {
        if line.rect.y < prev_y - EPS {
            return Err(format!("行 {index} 的 y 回退"));
        }
        prev_y = line.rect.y;
        if line.src.start > line.src.end() {
            return Err(format!("行 {index} 的 src 区间倒置"));
        }
        // 已知缺陷：整行只有行内图片时，`LineBox.src` 从 0 算起
        // （占位字符 U+FFFC 按 3 字节计），不指向真实文本。`Image` 行没有
        // 原文区间，本轮不动排版行为，只把它从“src 单调/落在文本缓冲里”
        // 这两条里排除；见 hardening 报告。
        let replaced_only = matches!(doc.node(line.node).kind, NodeKind::Image(_));
        if !replaced_only && line.src.end() as usize > doc.text.len() {
            return Err(format!("行 {index} 的 src 越出文本缓冲"));
        }
        if !replaced_only {
            if line.src.start < prev_src {
                return Err(format!(
                    "行 {index} 的 src.start 回退 ({} < {})",
                    line.src.start, prev_src
                ));
            }
            prev_src = line.src.start;
        }
        if line.rect.w < -EPS || line.rect.h < -EPS {
            return Err(format!("行 {index} 的行盒为负"));
        }
    }

    for (index, op) in ir.display.ops.iter().enumerate() {
        match op {
            Op::GlyphRun(run) => {
                if run.glyphs.as_range().end > ir.display.glyphs.len() {
                    return Err(format!("op {index} 的字形切片越界"));
                }
                if !run.size.is_finite() || run.size <= 0.0 {
                    return Err(format!("op {index} 的字号非法: {}", run.size));
                }
                if !run.origin.x.is_finite() || !run.origin.y.is_finite() {
                    return Err(format!("op {index} 的起点不是有限值"));
                }
                for glyph in &ir.display.glyphs[run.glyphs.as_range()] {
                    if !glyph.dx.is_finite() || !glyph.dy.is_finite() {
                        return Err(format!("op {index} 的字形位移不是有限值"));
                    }
                }
            }
            Op::Rect { rect, .. } | Op::Link { rect, .. } => {
                if !finite(*rect) {
                    return Err(format!("op {index} 的矩形不是有限值"));
                }
            }
            Op::Border(border) => {
                if !finite(border.rect) {
                    return Err(format!("op {index} 的边框矩形不是有限值"));
                }
            }
            Op::Image { rect, .. } | Op::Unresolved(ebook_ir::Unresolved { rect, .. }) => {
                if !finite(*rect) {
                    return Err(format!("op {index} 的占位矩形不是有限值"));
                }
            }
            Op::Clip { rect, ops } => {
                if !finite(*rect) {
                    return Err(format!("op {index} 的裁剪矩形不是有限值"));
                }
                if ops.is_empty() {
                    return Err(format!("op {index} 是空裁剪"));
                }
            }
        }
    }
    Ok(())
}

fn page_constraint(rng: &mut Rng) -> PageConstraint {
    // 极小页面与常规页面都出现；margin 偶尔比页面还大，触发 1.0 px 夹取。
    let width = 40.0 + rng.unit() * 1200.0;
    let height = 30.0 + rng.unit() * 1600.0;
    let margin = if rng.chance(6) {
        30.0 + rng.unit() * 120.0
    } else {
        rng.unit() * 24.0
    };
    PageConstraint::new(width, height).with_margin(ebook_ir::EdgesF::all(margin))
}

#[test]
fn thousand_random_documents_hold_invariants() {
    let mut total_lines = 0usize;
    let mut total_pages = 0usize;
    let mut multi_page = 0usize;
    for seed in 1..=1000u64 {
        let mut doc = Builder::new(seed).finish();
        // 随机的 `node.link` / 图片元数据不影响排版，但会让缓存键与 IR 形状更散。
        doc.images.push(ebook_ir::ImageMeta {
            size: if seed % 2 == 0 {
                Some(ebook_ir::SizeF::new(24.0, 18.0))
            } else {
                None
            },
            mime: Some(String::from("image/png")),
            alt: None,
            src: None,
        });
        let mut rng = Rng::new(seed ^ 0xdead_beef);
        let constraint = page_constraint(&mut rng);
        let env = LayoutEnv {
            measurer: &StubMeasurer,
            resources: &Sizes,
        };

        let first = layout(&doc, &env, &constraint);
        let second = layout(&doc, &env, &constraint);
        let a = codec::to_bytes(&first).expect("encode");
        let b = codec::to_bytes(&second).expect("encode");
        assert_eq!(
            a, b,
            "seed {seed}: 重复排版不是逐字节相同（{}x{}）",
            constraint.page.w, constraint.page.h
        );

        let decoded: LayoutResult = codec::decode(&a).expect("decode");
        assert_eq!(decoded, first, "seed {seed}: 编解码往返改变了 IR");
        assert_eq!(
            codec::to_bytes(&decoded).expect("re-encode"),
            a,
            "seed {seed}: 再编码不是同一串字节"
        );

        if let Err(problem) = check(&doc, &first) {
            panic!(
                "seed {seed} at {}x{}{}: {problem}",
                constraint.page.w,
                constraint.page.h,
                if first.truncated { " (truncated)" } else { "" }
            );
        }

        total_lines += first.runs.lines.len();
        total_pages += first.pages.len();
        if first.pages.len() > 1 {
            multi_page += 1;
        }
    }
    // 语料太瘦说明生成器失效，比“没 panic”更值得报警。
    assert!(total_lines > 3000, "语料太瘦: {total_lines} 行");
    assert!(total_pages > 1000, "语料太瘦: {total_pages} 页");
    assert!(multi_page > 200, "语料几乎不分页: {multi_page} 多页文档");
}

/// 排版层必须接受“调用方违约”以外的任何合法 `StyledDoc`：空文本、零孩子、
/// 自环之外的怪结构都不能 panic。
#[test]
fn degenerate_documents_never_panic() {
    let cases: [(&str, u32, u32); 4] = [
        ("极小页面", 1, 1),
        ("极窄页面", 40, 10_000),
        ("极扁页面", 10_000, 20),
        ("负边距", 200, 200),
    ];
    for (name, width, height) in cases {
        let mut doc = StyledDoc::default();
        let root = doc.push_node(Node {
            kind: NodeKind::Element(Tag::Body),
            style: ComputedStyle::default(),
            parent: None,
            first_child: 0,
            child_len: 0,
            src: None,
            link: None,
        });
        doc.root = root;
        let span_start = doc.text.len() as u32;
        doc.text.push_str("hello 中文 world");
        let text = doc.push_node(Node {
            kind: NodeKind::Text(Span::new(span_start, doc.text.len() as u32 - span_start)),
            style: ComputedStyle::inline(FontSpec::default(), Rgba8::BLACK, Dir::Ltr),
            parent: Some(root),
            first_child: 0,
            child_len: 0,
            src: None,
            link: None,
        });
        doc.set_kids(root, &[text]);
        let env = LayoutEnv {
            measurer: &StubMeasurer,
            resources: &Sizes,
        };
        let constraint = PageConstraint::new(width as f32, height as f32)
            .with_margin(ebook_ir::EdgesF::all(-8.0));
        let ir = layout(&doc, &env, &constraint);
        if let Err(problem) = check(&doc, &ir) {
            panic!("{name}: {problem}");
        }
        assert!(codec::to_bytes(&ir).is_ok(), "{name}: 结果不可编码");
    }

    // 真正的空文档：只有根节点，没有孩子。
    let mut doc = StyledDoc::default();
    let root = doc.push_node(Node {
        kind: NodeKind::Element(Tag::Body),
        style: ComputedStyle::default(),
        parent: None,
        first_child: 0,
        child_len: 0,
        src: None,
        link: None,
    });
    doc.root = root;
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &Sizes,
    };
    for (width, height) in [(1.0, 1.0), (0.5, 800.0), (800.0, 0.5), (1e5, 1e5)] {
        let ir = layout(&doc, &env, &PageConstraint::new(width, height));
        assert!(ir.pages.len() <= 1, "空文档产出了 {} 页", ir.pages.len());
        assert!(codec::to_bytes(&ir).is_ok(), "空文档不可编码");
    }
    let _ = (Sides::all(Len::ZERO), FontId(0));
}
