use alloc::string::String;
use alloc::vec::Vec;
use ebook_ir::{
    Anchor, Break, ComputedStyle, Dir, DirtyBlock, Display, Fit, FontId, Glyph, ImageId, NodeId,
    NodeKind, Op, PointF, RectF, Rotate, Span, TextAlign, TextTransform, Unresolved,
    UnresolvedReason, VerticalAlign, WhiteSpace,
};

use crate::resolve;
use crate::shape::{Cluster, ShapeGlyph};
use crate::{LayoutEnv, PageConstraint};

#[derive(Clone, Debug)]
pub(crate) struct LaidRun {
    pub(crate) font: FontId,
    pub(crate) size: f32,
    pub(crate) color: ebook_ir::Rgba8,
    pub(crate) dir: Dir,
    pub(crate) rotate: Rotate,
    pub(crate) origin: PointF,
    pub(crate) glyphs: Span,
    pub(crate) text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct LaidLine {
    pub(crate) y: f32,
    pub(crate) x: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
    pub(crate) baseline: f32,
    pub(crate) dir: Dir,
    pub(crate) src: Span,
    pub(crate) anchor: Option<Anchor>,
    pub(crate) glyphs: Span,
    pub(crate) node: NodeId,
    pub(crate) link: Option<String>,
    pub(crate) runs: Vec<LaidRun>,
    /// 两端对齐可以在哪些字形之后加余量（全局字形下标）。
    pub(crate) stretch: Vec<u32>,
    pub(crate) unresolved: Vec<Op>,
    pub(crate) block: u32,
    pub(crate) order: u16,
    pub(crate) len: u16,
    pub(crate) orphans: u16,
    pub(crate) widows: u16,
    pub(crate) force: bool,
    pub(crate) avoid_before: bool,
    pub(crate) avoid_after: bool,
}

pub(crate) enum Item {
    Line(LaidLine),
    Op(Op),
}

#[derive(Default)]
pub(crate) struct Flow {
    pub(crate) items: Vec<Item>,
    pub(crate) glyphs: Vec<Glyph>,
    pub(crate) dirty: Vec<DirtyBlock>,
}

#[derive(Clone, Copy, PartialEq)]
enum PieceKind {
    Text,
    Break,
    /// 行内图片：宽度/高度已知时按资源尺寸，未知时留 1em × 1.5em 的占位。
    Image {
        id: ImageId,
        size: Option<ebook_ir::SizeF>,
    },
    /// 注音：`span` 是基文，`rt` 是注音文本；rt 不参与断行宽度。
    Ruby {
        rt: Span,
    },
}

#[derive(Clone, Copy)]
struct Piece {
    style: ComputedStyle,
    span: Span,
    node: NodeId,
    kind: PieceKind,
}

#[derive(Clone, Copy)]
struct Ch {
    c: char,
    off: u32,
    piece: u16,
    /// 所属整形簇；换行符没有簇，用 [`NO_CLUSTER`] 标记。
    cluster: u32,
    /// 簇首字符：只有它贡献行宽，也只有它产字形。
    head: bool,
    /// UAX #9 的嵌入级：偶数 LTR、奇数 RTL。
    level: u8,
    /// 行内图片：这个"字符"实际是一个替换元素。
    image: Option<ImageId>,
}

const NO_CLUSTER: u32 = u32::MAX;

/// CSS `tab-size` 的初始值：制表符展开到下一个 8 列制表位。
const TAB_SIZE: usize = 8;

#[derive(Default)]
struct Shaped {
    glyphs: Vec<ShapeGlyph>,
    clusters: Vec<Cluster>,
}

#[derive(Clone, Copy, Default)]
struct CollapsedMargin {
    positive: f32,
    negative: f32,
}

impl CollapsedMargin {
    const ZERO: Self = Self {
        positive: 0.0,
        negative: 0.0,
    };

    fn new(value: f32) -> Self {
        Self {
            positive: value.max(0.0),
            negative: value.min(0.0),
        }
    }

    fn add(self, value: f32) -> Self {
        Self {
            positive: self.positive.max(value.max(0.0)),
            negative: self.negative.min(value.min(0.0)),
        }
    }

    fn merge(self, other: Self) -> Self {
        Self {
            positive: self.positive.max(other.positive),
            negative: self.negative.min(other.negative),
        }
    }

    fn value(self) -> f32 {
        self.positive + self.negative
    }
}

#[derive(Clone, Copy)]
struct BlockFlow {
    margin: CollapsedMargin,
    empty: bool,
}

#[derive(Clone, Copy)]
struct LineBoxEntry {
    owner: Option<NodeId>,
    align: VerticalAlign,
    shift: f32,
    top_dist: f32,
    bottom_dist: f32,
}

struct FlowCtx<'a> {
    doc: &'a ebook_ir::StyledDoc,
    env: &'a LayoutEnv<'a>,
    content_w: f32,
    flow: Flow,
    depth: u32,
    next_block: u32,
}

pub(crate) fn flow(doc: &ebook_ir::StyledDoc, env: &LayoutEnv<'_>, c: &PageConstraint) -> Flow {
    let content = c.content_size();
    let mut ctx = FlowCtx {
        doc,
        env,
        content_w: content.w.max(1.0),
        flow: Flow::default(),
        depth: 0,
        next_block: 0,
    };
    let mut y = 0.0;
    let root_w = ctx.content_w;
    let _ = ctx.block(doc.root, 0.0, &mut y, root_w, CollapsedMargin::ZERO);
    ctx.flow
}

impl<'a> FlowCtx<'a> {
    fn is_inline_node(&self, id: NodeId) -> bool {
        let node = self.doc.node(id);
        node.kind.is_text() || node.style.is_inline()
    }

    fn text_is_collapsible_blank(&self, id: NodeId) -> bool {
        let node = self.doc.node(id);
        let NodeKind::Text(span) = node.kind else {
            return false;
        };
        if !matches!(
            node.style.text.white_space,
            WhiteSpace::Normal | WhiteSpace::Nowrap
        ) {
            return false;
        }
        self.doc
            .text_of(span)
            .chars()
            .all(|c| matches!(c, ' ' | '\n' | '\r' | '\t'))
    }

    fn first_child_is_block(&self, id: NodeId) -> bool {
        let node = self.doc.node(id);
        for index in 0..node.child_len {
            let kid = self.doc.children[(node.first_child + index) as usize];
            let style = self.doc.node(kid).style;
            if style.display == Display::None || self.text_is_collapsible_blank(kid) {
                continue;
            }
            return matches!(style.display, Display::Block | Display::ListItem);
        }
        false
    }

    fn block(
        &mut self,
        id: NodeId,
        x: f32,
        y: &mut f32,
        avail_w: f32,
        prev_mb: CollapsedMargin,
    ) -> BlockFlow {
        self.depth += 1;
        if self.depth > 128 {
            self.depth -= 1;
            return BlockFlow {
                margin: CollapsedMargin::ZERO,
                empty: true,
            };
        }

        let node = self.doc.node(id);
        let style = node.style;
        let kind = node.kind.clone();
        let em = style.font.size;
        let margin = resolve::margins(&style.margin, avail_w, em);
        let padding = resolve::sides(&style.padding, avail_w, em);
        let border = resolve::sides(&style.border.width, avail_w, em);
        let can_collapse_top = border.top <= 0.0 && padding.top <= 0.0;
        let can_collapse_bottom =
            border.bottom <= 0.0 && padding.bottom <= 0.0 && style.height.is_auto();
        let deferred_top = can_collapse_top && self.first_child_is_block(id);
        let mut last_mb;
        let mut applied_top = CollapsedMargin::ZERO;
        if deferred_top {
            last_mb = prev_mb.add(margin.top);
        } else {
            applied_top = prev_mb.add(margin.top);
            *y += applied_top.value();
            last_mb = CollapsedMargin::ZERO;
        }

        let mut cy = *y + border.top + padding.top;
        let inner_x = x + margin.left + border.left + padding.left;
        let inner_w = match style.width.resolve(avail_w, em) {
            Some(w) => w.max(0.0),
            None => (avail_w - margin.horizontal() - border.horizontal() - padding.horizontal())
                .max(0.0),
        };
        let mut content_empty = true;

        match kind {
            NodeKind::Text(_) => {
                let before = self.flow.items.len();
                self.inline_from(&[id], inner_x, &mut cy, inner_w, style);
                if self.flow.items.len() > before {
                    content_empty = false;
                }
            }
            NodeKind::LineBreak => {}
            NodeKind::Image(img) => {
                self.image(id, img, style, inner_x, &mut cy, inner_w);
                content_empty = false;
            }
            NodeKind::Ruby { base, rt } => {
                let mut pieces = Vec::new();
                pieces.push(Piece {
                    style,
                    span: base,
                    node: id,
                    kind: PieceKind::Ruby { rt },
                });
                let before = self.flow.items.len();
                self.break_lines(&pieces, inner_x, &mut cy, inner_w, style);
                if self.flow.items.len() > before {
                    content_empty = false;
                }
            }
            NodeKind::Element(_) => {
                let child_len = node.child_len;
                let first_child = node.first_child;
                let mut buf: Vec<NodeId> = Vec::new();
                for index in 0..child_len {
                    let kid = self.doc.children[(first_child + index) as usize];
                    let kid_style = self.doc.node(kid).style;
                    if kid_style.display == Display::None {
                        continue;
                    }
                    if self.is_inline_node(kid) {
                        buf.push(kid);
                        continue;
                    }
                    if !buf.is_empty() {
                        let pending = last_mb;
                        cy += pending.value();
                        last_mb = CollapsedMargin::ZERO;
                        let before = self.flow.items.len();
                        self.inline_from(&buf, inner_x, &mut cy, inner_w, style);
                        if self.flow.items.len() > before {
                            content_empty = false;
                        } else {
                            cy -= pending.value();
                            last_mb = pending;
                        }
                        buf.clear();
                    }
                    let child = self.block(kid, inner_x, &mut cy, inner_w, last_mb);
                    last_mb = child.margin;
                    if !child.empty {
                        content_empty = false;
                    }
                }
                if !buf.is_empty() {
                    let pending = last_mb;
                    cy += pending.value();
                    last_mb = CollapsedMargin::ZERO;
                    let before = self.flow.items.len();
                    self.inline_from(&buf, inner_x, &mut cy, inner_w, style);
                    if self.flow.items.len() > before {
                        content_empty = false;
                    } else {
                        cy -= pending.value();
                        last_mb = pending;
                    }
                }
            }
        }

        let empty =
            content_empty && can_collapse_top && can_collapse_bottom && style.height.is_auto();
        if empty {
            if !deferred_top {
                *y -= applied_top.value();
            }
            let carried = if deferred_top {
                last_mb
            } else {
                prev_mb.add(margin.top).merge(last_mb)
            };
            self.depth -= 1;
            return BlockFlow {
                margin: carried.add(margin.bottom),
                empty: true,
            };
        }

        let margin_bottom = if can_collapse_bottom {
            last_mb.add(margin.bottom)
        } else {
            cy += last_mb.value();
            CollapsedMargin::new(margin.bottom)
        };
        *y = cy + border.bottom + padding.bottom;
        self.depth -= 1;
        BlockFlow {
            margin: margin_bottom,
            empty: false,
        }
    }

    fn image(
        &mut self,
        node: NodeId,
        img: ImageId,
        style: ComputedStyle,
        x: f32,
        y: &mut f32,
        avail_w: f32,
    ) {
        let em = style.font.size;
        let margin = resolve::margins(&style.margin, avail_w, em);
        let size = self.image_size(img, &style, avail_w);
        let rect = match size {
            Some(s) => RectF::new(x + margin.left, *y, s.w, s.h),
            None => RectF::new(x + margin.left, *y, avail_w.max(1.0), em * 1.5),
        };
        match size {
            Some(_) => self.flow.items.push(Item::Op(Op::Image {
                id: img,
                rect,
                fit: Fit::Contain,
            })),
            None => {
                self.flow.items.push(Item::Op(Op::Unresolved(Unresolved {
                    node,
                    rect,
                    reason: UnresolvedReason::ImagePending,
                })));
                self.flow.dirty.push(DirtyBlock {
                    node,
                    rect,
                    reason: UnresolvedReason::ImagePending,
                });
            }
        }
        *y += rect.h + margin.bottom;
    }

    fn image_size(
        &self,
        img: ImageId,
        style: &ComputedStyle,
        avail_w: f32,
    ) -> Option<ebook_ir::SizeF> {
        let em = style.font.size;
        let w = style.width.resolve(avail_w, em);
        let h = style.height.resolve(0.0, em);
        if let (Some(w), Some(h)) = (w, h) {
            return Some(ebook_ir::SizeF::new(w, h));
        }
        if let Some(size) = self.doc.images.get(img as usize).and_then(|m| m.size) {
            return Some(size);
        }
        self.env.resources.image_size(img)
    }

    /// 行内图片尺寸：显式宽高 > 文档里的固有尺寸 > 资源查询，都没有就是未知。
    fn inline_image_size(&self, img: ImageId, style: &ComputedStyle) -> Option<ebook_ir::SizeF> {
        let em = style.font.size;
        let w = style.width.resolve(em, em);
        let h = style.height.resolve(em, em);
        if let (Some(w), Some(h)) = (w, h) {
            if w > 0.0 && h > 0.0 {
                return Some(ebook_ir::SizeF::new(w, h));
            }
        }
        if let Some(size) = self.doc.images.get(img as usize).and_then(|m| m.size) {
            return Some(size);
        }
        self.env.resources.image_size(img)
    }

    fn inline_from(
        &mut self,
        nodes: &[NodeId],
        x: f32,
        y: &mut f32,
        w: f32,
        container: ComputedStyle,
    ) {
        let mut pieces = Vec::new();
        for &id in nodes {
            self.collect(id, &mut pieces);
        }
        if pieces.is_empty() {
            return;
        }
        self.break_lines(&pieces, x, y, w, container);
    }

    fn collect(&self, id: NodeId, out: &mut Vec<Piece>) {
        let node = self.doc.node(id);
        if node.style.display == Display::None {
            return;
        }
        match node.kind {
            NodeKind::Text(span) => out.push(Piece {
                style: node.style,
                span,
                node: id,
                kind: PieceKind::Text,
            }),
            NodeKind::LineBreak => out.push(Piece {
                style: node.style,
                span: Span::EMPTY,
                node: id,
                kind: PieceKind::Break,
            }),
            NodeKind::Image(img) => out.push(Piece {
                style: node.style,
                span: Span::EMPTY,
                node: id,
                kind: PieceKind::Image {
                    id: img,
                    size: self.inline_image_size(img, &node.style),
                },
            }),
            NodeKind::Ruby { base, rt } => out.push(Piece {
                style: node.style,
                span: base,
                node: id,
                kind: PieceKind::Ruby { rt },
            }),
            NodeKind::Element(_) => {
                let child_len = node.child_len;
                let first_child = node.first_child;
                for i in 0..child_len {
                    let kid = self.doc.children[(first_child + i) as usize];
                    self.collect(kid, out);
                }
            }
        }
    }

    /// 把一个字符按 `text-transform` 展开成若干 `Ch`，返回实际写入数量。
    fn push_ch(out: &mut Vec<Ch>, pi: usize, c: char, off: u32, transform: TextTransform) -> usize {
        let before = out.len();
        let mut push = |c: char| {
            out.push(Ch {
                c,
                off,
                piece: pi as u16,
                cluster: NO_CLUSTER,
                head: true,
                level: 0,
                image: None,
            });
        };
        match transform {
            TextTransform::None => push(c),
            TextTransform::Uppercase => {
                for t in c.to_uppercase() {
                    push(t);
                }
            }
            TextTransform::Lowercase => {
                for t in c.to_lowercase() {
                    push(t);
                }
            }
        }
        out.len() - before
    }

    fn flatten(&self, pieces: &[Piece], out: &mut Vec<Ch>) {
        // 折叠状态跨 piece 保留：CSS 的空白折叠不受内联元素边界影响，
        // 前一个 piece 行尾的空格会让后一个 piece 行首的空格继续被折叠。
        let mut prev_space = true;
        // 制表位按当前行已有多少列计算；硬换行与 <br> 复位，图片也算一列。
        let mut col = 0usize;
        for (pi, piece) in pieces.iter().enumerate() {
            match piece.kind {
                PieceKind::Break => {
                    out.push(Ch {
                        c: '\n',
                        off: 0,
                        piece: pi as u16,
                        cluster: NO_CLUSTER,
                        head: true,
                        level: 0,
                        image: None,
                    });
                    prev_space = false;
                    col = 0;
                }
                PieceKind::Image { id, .. } => {
                    out.push(Ch {
                        c: '\u{FFFC}',
                        off: 0,
                        piece: pi as u16,
                        cluster: NO_CLUSTER,
                        head: true,
                        level: 0,
                        image: Some(id),
                    });
                    prev_space = false;
                    col = col.saturating_add(1);
                }
                PieceKind::Text | PieceKind::Ruby { .. } => {
                    let text = self.doc.text_of(piece.span);
                    let white_space = piece.style.text.white_space;
                    // PreWrap 与 BreakSpaces 在这里都保留空格并允许软换行。
                    // 规范差异有两点：BreakSpaces 的行末空格不悬挂、也不裁掉，
                    // 且每个空格之后都是断行机会。当前实现从简：UAX #14 的
                    // Space 规则本来就在空格后给断点，`emit_line` 也不裁剪行末
                    // 空格，所以两者共享同一条路径，没有额外的悬挂宽度调整。
                    // Normal / Nowrap / PreLine 折叠空格序列。
                    let collapse_spaces = matches!(
                        white_space,
                        WhiteSpace::Normal | WhiteSpace::Nowrap | WhiteSpace::PreLine
                    );
                    // Normal / Nowrap 之外的模式都把换行保留成硬换行。
                    let preserve_breaks =
                        !matches!(white_space, WhiteSpace::Normal | WhiteSpace::Nowrap);
                    // pre 系模式按 tab-size 展开；Normal / Nowrap 折成一个空格。
                    let expand_tabs = preserve_breaks;
                    let transform = piece.style.text.transform;
                    let mut it = text.char_indices().peekable();
                    while let Some((off, mut raw)) = it.next() {
                        // CRLF / 单独的 CR 都算一个硬换行；Normal / Nowrap 下
                        // 仍然折成一个空格，与改动前一致。
                        if raw == '\r' {
                            if matches!(it.peek(), Some((_, '\n'))) {
                                it.next();
                            }
                            raw = '\n';
                        }
                        match raw {
                            '\n' => {
                                if preserve_breaks {
                                    // 断行循环已经把 '\n' 当硬换行，这里只负责保留它。
                                    Self::push_ch(out, pi, '\n', off as u32, transform);
                                    prev_space = true;
                                    col = 0;
                                } else if !prev_space {
                                    Self::push_ch(out, pi, ' ', off as u32, transform);
                                    prev_space = true;
                                    col = col.saturating_add(1);
                                }
                            }
                            '\t' => {
                                if expand_tabs {
                                    let count = TAB_SIZE - col % TAB_SIZE;
                                    for _ in 0..count {
                                        Self::push_ch(out, pi, ' ', off as u32, transform);
                                    }
                                    col = col.saturating_add(count);
                                    prev_space = true;
                                } else if !prev_space {
                                    Self::push_ch(out, pi, ' ', off as u32, transform);
                                    prev_space = true;
                                    col = col.saturating_add(1);
                                }
                            }
                            ' ' if collapse_spaces => {
                                if !prev_space {
                                    Self::push_ch(out, pi, ' ', off as u32, transform);
                                    prev_space = true;
                                    col = col.saturating_add(1);
                                }
                            }
                            c => {
                                let count = Self::push_ch(out, pi, c, off as u32, transform);
                                prev_space = c == ' ';
                                col = col.saturating_add(count);
                            }
                        }
                    }
                }
            }
        }
    }

    fn font_for(&self, ch: &Ch, pieces: &[Piece]) -> FontId {
        let style = &pieces[ch.piece as usize].style;
        self.env.font_for(&style.font, ch.c)
    }

    /// 按"同字体同字号"分组整形，字距/词距在整形之后叠加。
    ///
    /// 分组键是 `(piece, font, covers)`：同一段文本里字体切换、缺字都会切开 run，
    /// 后面的断行与绘制都直接复用这里的簇宽，保证量到的和画出来的是一份数据。
    fn shape_chars(&self, pieces: &[Piece], chars: &mut [Ch], shaped: &mut Shaped) {
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i].c == '\n' {
                chars[i].cluster = NO_CLUSTER;
                chars[i].head = true;
                i += 1;
                continue;
            }
            if chars[i].image.is_some() {
                // 行内图片是一个原子替换元素：一个簇、没有字形、宽度就是它的宽度。
                let style = pieces[chars[i].piece as usize].style;
                let em = style.font.size;
                let width = match pieces[chars[i].piece as usize].kind {
                    PieceKind::Image { size, .. } => size.map(|s| s.w).unwrap_or(em),
                    _ => em,
                };
                let advance = width.max(0.0);
                shaped.clusters.push(Cluster {
                    glyph_start: shaped.glyphs.len() as u32,
                    glyph_len: 0,
                    advance,
                    ch: 0,
                    gap: 0.0,
                });
                let cluster = (shaped.clusters.len() - 1) as u32;
                chars[i].cluster = cluster;
                chars[i].head = true;
                i += 1;
                continue;
            }
            let piece_id = chars[i].piece;
            let style = pieces[piece_id as usize].style;
            let font = self.font_for(&chars[i], pieces);
            let covered = self.env.measurer.covers(chars[i].c, font);
            let level = chars[i].level;
            let mut j = i + 1;
            while j < chars.len()
                && chars[j].c != '\n'
                && chars[j].piece == piece_id
                && chars[j].level == level
            {
                let next = self.font_for(&chars[j], pieces);
                if next != font || self.env.measurer.covers(chars[j].c, next) != covered {
                    break;
                }
                j += 1;
            }

            let mut text = String::new();
            let mut abs: Vec<u32> = Vec::with_capacity(j - i);
            for ch in &chars[i..j] {
                text.push(ch.c);
                abs.push(pieces[ch.piece as usize].span.start + ch.off);
            }

            let base = shaped.clusters.len() as u32;
            let mut map: Vec<u32> = Vec::with_capacity(j - i);
            if covered {
                crate::shape::shape_run(
                    self.env.measurer,
                    font,
                    style.font.size,
                    &text,
                    &abs,
                    &mut shaped.glyphs,
                    &mut shaped.clusters,
                    &mut map,
                );
                // 字距按字符累加到所属簇；簇宽以"整形 + 字距"为准。
                let cluster_count = shaped.clusters.len() - base as usize;
                let mut spacing: Vec<f32> = alloc::vec![0.0; cluster_count];
                for (k, ch) in chars[i..j].iter().enumerate() {
                    let extra = style.text.letter_spacing
                        + if ch.c == ' ' {
                            style.text.word_spacing
                        } else {
                            0.0
                        };
                    spacing[(map[k] - base) as usize] += extra;
                }
                for (local, &extra) in spacing.iter().enumerate() {
                    let index = base as usize + local;
                    let cluster = shaped.clusters[index];
                    let advance = (cluster.advance + extra).max(0.0);
                    shaped.clusters[index].advance = advance;
                    let start = cluster.glyph_start as usize;
                    let end = start + cluster.glyph_len as usize;
                    crate::shape::pin_cluster(&mut shaped.glyphs[start..end], advance);
                }
            } else {
                // 缺字不猜：仍然占位（宽度取 measurer 的 advance），但不产字形。
                for ch in &chars[i..j] {
                    let advance = self
                        .env
                        .measurer
                        .advance(ch.c, font, style.font.size)
                        .max(0.0);
                    shaped.clusters.push(Cluster {
                        glyph_start: shaped.glyphs.len() as u32,
                        glyph_len: 0,
                        advance,
                        ch: pieces[ch.piece as usize].span.start + ch.off,
                        gap: 0.0,
                    });
                }
                map.extend((0..(j - i) as u32).map(|local| base + local));
            }

            for (k, ch) in chars[i..j].iter_mut().enumerate() {
                ch.cluster = map[k];
                ch.head = k == 0 || map[k] != map[k - 1];
            }
            i = j;
        }
    }

    /// 给一个簇减半宽：改簇宽，并把差值写回末字形，量到的和画出来的同步。
    fn trim_cluster(shaped: &mut Shaped, cluster: u32, delta: f32) {
        let slot = &mut shaped.clusters[cluster as usize];
        let delta = delta.min(slot.advance).max(0.0);
        if delta <= 0.0 {
            return;
        }
        slot.advance -= delta;
        let start = slot.glyph_start as usize;
        let end = start + slot.glyph_len as usize;
        crate::shape::pin_cluster(&mut shaped.glyphs[start..end], slot.advance);
    }

    /// 成对标点挤压：相邻两个全角标点之间收半宽，规则表在 `punct.rs`。
    ///
    /// 只压拥有空白半边的那一个（开标点压左、闭标点压右），所以一对最多少
    /// 半宽；这里不挪字形墨迹，`Glyph.dx` 只表达前进宽度。
    fn apply_trim(&self, chars: &[Ch], shaped: &mut Shaped, em: f32) {
        let half = em * 0.5;
        for pair in chars.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            if !left.head || !right.head || left.c == '\n' || right.c == '\n' {
                continue;
            }
            let Some(target) = crate::punct::trim_target(left.c, right.c) else {
                continue;
            };
            let trimmed = match target {
                crate::punct::Trim::Left => left,
                crate::punct::Trim::Right => right,
            };
            Self::trim_cluster(shaped, trimmed.cluster, half);
        }
    }

    /// 中西文自动间距：在表意文字与拉丁字母/数字之间加 1/4 em。
    ///
    /// 空隙挂在**左**字符的簇上，所以行首永远不会有空隙；行末的空隙在
    /// `emit_line` 里收回，保证"不插在行首行尾"。
    fn apply_autospacing(&self, chars: &[Ch], shaped: &mut Shaped, em: f32) {
        let gap = em * 0.25;
        for pair in chars.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            if !left.head || !right.head || left.c == '\n' || right.c == '\n' {
                continue;
            }
            if !crate::punct::needs_autospacing(left.c, right.c) {
                continue;
            }
            let slot = &mut shaped.clusters[left.cluster as usize];
            slot.advance += gap;
            slot.gap += gap;
            let start = slot.glyph_start as usize;
            let end = start + slot.glyph_len as usize;
            crate::shape::pin_cluster(&mut shaped.glyphs[start..end], slot.advance);
        }
    }

    /// 找出可断字的词内断点，并给词内每个字符记一个"预留连字符宽度"。
    ///
    /// 预留让断行时不必回头重排：只要在词内断行，行末的 `-` 一定放得下。
    fn hyphen_points(
        &self,
        pieces: &[Piece],
        chars: &[Ch],
        hyphenator: &dyn crate::Hyphenator,
        points: &mut Vec<usize>,
        reserve: &mut [f32],
    ) {
        let mut word = String::new();
        let mut word_start = 0usize;
        let mut i = 0usize;
        let mut breaks: Vec<u16> = Vec::new();
        loop {
            let inside = i < chars.len() && chars[i].c.is_ascii_alphabetic();
            if inside {
                if word.is_empty() {
                    word_start = i;
                }
                word.push(chars[i].c);
                i += 1;
                continue;
            }
            if !word.is_empty() {
                if word.len() >= 5 {
                    hyphenator.breaks(&word, &mut breaks);
                    if !breaks.is_empty() {
                        let style = pieces[chars[word_start].piece as usize].style;
                        let font = self.font_for(&chars[word_start], pieces);
                        let dash = self
                            .env
                            .measurer
                            .advance('-', font, style.font.size)
                            .max(0.0);
                        if dash > 0.0 {
                            for slot in &mut reserve[word_start..i] {
                                *slot = dash;
                            }
                        }
                        for at in &breaks {
                            let index = word_start + *at as usize;
                            if index < i && chars[index].head {
                                points.push(index);
                            }
                        }
                    }
                }
                word.clear();
            }
            if i >= chars.len() {
                break;
            }
            i += 1;
        }
    }

    /// 在 `line_start..=want` 里找最靠后、且"文本 + 连字符"放得下的断字点。
    ///
    /// 连字符的宽度取词内预留值（`reserve`），它是按词首字符的样式量的。
    fn fit_hyphen(
        chars: &[Ch],
        shaped: &Shaped,
        points: &[usize],
        reserve: &[f32],
        line_start: usize,
        want: usize,
        w: f32,
    ) -> Option<usize> {
        let mut candidates: Vec<usize> = Vec::new();
        candidates.push(want);
        for &point in points.iter().rev() {
            if point < want && point > line_start {
                candidates.push(point);
            }
        }
        for at in candidates {
            let mut width = 0.0f32;
            for ch in &chars[line_start..at] {
                if ch.head {
                    width += shaped.clusters[ch.cluster as usize].advance;
                }
            }
            let dash = reserve.get(at.saturating_sub(1)).copied().unwrap_or(0.0);
            if width + dash <= w + 0.001 {
                return Some(at);
            }
        }
        None
    }

    fn break_lines(
        &mut self,
        pieces: &[Piece],
        x0: f32,
        y: &mut f32,
        w: f32,
        container: ComputedStyle,
    ) {
        let mut chars: Vec<Ch> = Vec::new();
        self.flatten(pieces, &mut chars);
        if chars.is_empty() {
            return;
        }
        self.resolve_bidi(&mut chars, container.dir);
        let mut shaped = Shaped::default();
        self.shape_chars(pieces, &mut chars, &mut shaped);
        // 一个簇必须同级：跨级簇在重排时会被拆散，取簇首的级并灌回整簇。
        let mut index = 0usize;
        while index < chars.len() {
            let head = chars[index].level;
            let cluster = chars[index].cluster;
            index += 1;
            while index < chars.len() && chars[index].cluster == cluster {
                chars[index].level = head;
                index += 1;
            }
        }

        let block = self.next_block;
        self.next_block += 1;
        let item_start = self.flow.items.len();
        let mut order: u16 = 0;

        let base_style = container;
        let em = base_style.font.size;

        // 中文排版政策是段落级的：块上没有就取任意一个内联片段上打开的开关
        // （政策是文档级设置，宿主可能挂在 body 也可能挂在 span 上）。
        let mut rules = base_style.text.rules;
        for piece in pieces {
            let piece_rules = piece.style.text.rules;
            rules.spacing_trim |= piece_rules.spacing_trim;
            rules.hanging_punctuation |= piece_rules.hanging_punctuation;
            rules.autospacing |= piece_rules.autospacing;
        }
        if rules.spacing_trim {
            self.apply_trim(&chars, &mut shaped, em);
        }
        if rules.autospacing {
            self.apply_autospacing(&chars, &mut shaped, em);
        }

        // 断字：只有 hyphens:auto 且宿主注入了词典才找断点。断点落在字符边界、
        // 不拆簇；词典是入参，所以同一份输入必然得到同一组断点。
        let hyphens_on = (base_style.text.hyphens
            || pieces.iter().any(|piece| piece.style.text.hyphens))
            && self.env.measurer.hyphenator().is_some();
        let mut hyphen_points: Vec<usize> = Vec::new();
        let mut hyphen_reserve: Vec<f32> = alloc::vec![0.0; chars.len()];
        if hyphens_on {
            if let Some(hyphenator) = self.env.measurer.hyphenator() {
                self.hyphen_points(
                    pieces,
                    &chars,
                    hyphenator,
                    &mut hyphen_points,
                    &mut hyphen_reserve,
                );
            }
        }
        let hyphen_points = hyphen_points;
        let indent = base_style
            .text
            .indent
            .resolve(w, em)
            .unwrap_or(0.0)
            .clamp(0.0, w);

        let nowrap = matches!(
            base_style.text.white_space,
            WhiteSpace::Nowrap | WhiteSpace::Pre
        ) || pieces.iter().any(|p| {
            matches!(
                p.style.text.white_space,
                WhiteSpace::Nowrap | WhiteSpace::Pre
            )
        });
        // pre 系模式保留行首空格；Normal / Nowrap / PreLine 在软换行后跳过行首空白。
        let preserve_spaces = matches!(
            base_style.text.white_space,
            WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::BreakSpaces
        ) || pieces.iter().any(|p| {
            matches!(
                p.style.text.white_space,
                WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::BreakSpaces
            )
        });
        let mut line_start = 0usize;
        let mut last_break: Option<usize> = None;
        let mut used = indent;
        let mut i = 0usize;
        let mut first = true;

        while i < chars.len() {
            if chars[i].c == '\n' {
                let pad = if first { indent } else { 0.0 };
                self.emit_line(
                    pieces,
                    &chars[line_start..i],
                    &shaped,
                    x0,
                    y,
                    w,
                    pad,
                    false,
                    base_style,
                    block,
                    &mut order,
                );
                first = false;
                line_start = i + 1;
                used = 0.0;
                last_break = None;
                i += 1;
                continue;
            }
            let adv = if chars[i].head {
                shaped.clusters[chars[i].cluster as usize].advance
            } else {
                0.0
            };
            // 行末悬挂：闭标点最多探出右边界半宽，只影响"放没放下"，
            // 字形坐标不动，所以这里只放宽 fit 判定。
            let mut fits = used + adv <= w + 0.001;
            if !fits && rules.hanging_punctuation && i > line_start && chars[i].head {
                if let Some(hang) = crate::punct::hang_amount(chars[i].c, adv) {
                    if used + adv - hang <= w + 0.001 {
                        fits = true;
                    }
                }
            }
            if !nowrap && !fits && i > line_start {
                let normal = last_break.filter(|&b| b > line_start && b <= i);
                let hyphen = hyphen_points
                    .iter()
                    .rev()
                    .copied()
                    .find(|&b| b > line_start && b <= i);
                let (brk, hyphenated) = match (normal, hyphen) {
                    (Some(n), Some(h)) if h > n => {
                        match Self::fit_hyphen(
                            &chars,
                            &shaped,
                            &hyphen_points,
                            &hyphen_reserve,
                            line_start,
                            h,
                            w,
                        ) {
                            Some(at) => (at, true),
                            None => (n, false),
                        }
                    }
                    (Some(n), _) => (n, false),
                    (None, Some(h)) => {
                        match Self::fit_hyphen(
                            &chars,
                            &shaped,
                            &hyphen_points,
                            &hyphen_reserve,
                            line_start,
                            h,
                            w,
                        ) {
                            Some(at) => (at, true),
                            None => (i, false),
                        }
                    }
                    (None, None) => (i, false),
                };
                let pad = if first { indent } else { 0.0 };
                self.emit_line(
                    pieces,
                    &chars[line_start..brk],
                    &shaped,
                    x0,
                    y,
                    w,
                    pad,
                    hyphenated,
                    base_style,
                    block,
                    &mut order,
                );
                first = false;
                line_start = brk;
                if !preserve_spaces {
                    while line_start < chars.len() && chars[line_start].c == ' ' {
                        line_start += 1;
                    }
                }
                i = line_start;
                used = 0.0;
                last_break = None;
                continue;
            }
            used += adv;
            if i + 1 < chars.len()
                && chars[i].cluster != chars[i + 1].cluster
                && crate::linebreak::can_break(
                    chars[i].c,
                    chars[i + 1].c,
                    pieces[chars[i].piece as usize].style.text.word_break,
                )
            {
                last_break = Some(i + 1);
            }
            i += 1;
        }

        if line_start < chars.len() {
            let pad = if first { indent } else { 0.0 };
            self.emit_line(
                pieces,
                &chars[line_start..],
                &shaped,
                x0,
                y,
                w,
                pad,
                false,
                base_style,
                block,
                &mut order,
            );
        }

        let count = self.flow.items[item_start..]
            .iter()
            .filter(|item| matches!(item, Item::Line(_)))
            .count()
            .min(u16::MAX as usize) as u16;
        if count == 0 {
            return;
        }
        for item in &mut self.flow.items[item_start..] {
            if let Item::Line(line) = item {
                line.len = count;
            }
        }
        if let Some(Item::Line(line)) = self.flow.items[item_start..]
            .iter_mut()
            .rev()
            .find(|item| matches!(item, Item::Line(_)))
        {
            line.avoid_after = container.brk.after == Break::Avoid;
        }
        if base_style.text.align == TextAlign::Justify {
            let lines: Vec<usize> = (item_start..self.flow.items.len())
                .filter(|&index| matches!(self.flow.items[index], Item::Line(_)))
                .collect();
            let last = lines.last().copied();
            for index in lines {
                if Some(index) != last {
                    self.justify_line(index, x0, w);
                }
            }
        }
    }

    fn justify_line(&mut self, index: usize, x0: f32, w: f32) {
        let (runs, line_x, line_w, stretch) = match &self.flow.items[index] {
            Item::Line(line) => (line.runs.clone(), line.x, line.w, line.stretch.clone()),
            _ => return,
        };
        let target = (w - (line_x - x0)).max(0.0);
        let extra = target - line_w;
        if extra <= 0.001 || stretch.is_empty() {
            return;
        }
        let n = stretch.len() as f32;
        let each = extra / n;
        let mut origins = Vec::with_capacity(runs.len());
        let mut shift = 0.0f32;
        let mut next = 0usize;
        for run in &runs {
            origins.push(run.origin.x + shift);
            for glyph in run.glyphs.as_range() {
                if next < stretch.len() && stretch[next] as usize == glyph {
                    let delta = if next + 1 == stretch.len() {
                        extra - each * (n - 1.0)
                    } else {
                        each
                    };
                    self.flow.glyphs[glyph].dx += delta;
                    shift += delta;
                    next += 1;
                }
            }
        }
        if let Item::Line(line) = &mut self.flow.items[index] {
            for (run, origin) in line.runs.iter_mut().zip(origins) {
                run.origin.x = origin;
            }
            line.w = target;
        }
    }

    fn metrics(&self, style: &ComputedStyle) -> (f32, f32, f32, f32) {
        let size = style.font.size;
        let font = self.env.measurer.select(&style.font);
        let ascent = self.env.measurer.ascent(font, size);
        let descent = self.env.measurer.descent(font, size);
        let line_height = resolve::line_px(style.line_height, size);
        let half = (line_height - ascent - descent) * 0.5;
        (ascent, descent, ascent + half, descent + half)
    }

    fn relative_shift(
        &self,
        style: &ComputedStyle,
        parent: &ComputedStyle,
        align: VerticalAlign,
    ) -> f32 {
        match align {
            VerticalAlign::Baseline | VerticalAlign::Top | VerticalAlign::Bottom => 0.0,
            VerticalAlign::Len(len) => {
                let line_height = resolve::line_px(style.line_height, style.font.size);
                -len.resolve(line_height, style.font.size).unwrap_or(0.0)
            }
            VerticalAlign::Super => -parent.font.size / 3.0,
            VerticalAlign::Sub => parent.font.size / 5.0,
            VerticalAlign::Middle => {
                let (ascent, descent, _, _) = self.metrics(style);
                self.env
                    .measurer
                    .x_height(parent.font.font, parent.font.size)
                    * 0.5
                    + (ascent - descent) * 0.5
            }
            VerticalAlign::TextTop => {
                let (ascent, _, _, _) = self.metrics(style);
                let (parent_ascent, _, _, _) = self.metrics(parent);
                ascent - parent_ascent
            }
            VerticalAlign::TextBottom => {
                let (_, descent, _, _) = self.metrics(style);
                let (_, parent_descent, _, _) = self.metrics(parent);
                parent_descent - descent
            }
        }
    }

    fn owner_id(&self, id: NodeId) -> Option<NodeId> {
        let mut current = Some(id);
        while let Some(node_id) = current {
            let node = self.doc.node(node_id);
            if !node.kind.is_text() && node.style.display == Display::Inline {
                return Some(node_id);
            }
            current = node.parent;
        }
        None
    }

    fn owner_chain(&self, owner: NodeId) -> Vec<NodeId> {
        let mut chain = Vec::new();
        let mut current = Some(owner);
        while let Some(id) = current {
            let node = self.doc.node(id);
            chain.push(id);
            current = node.parent.filter(|&parent| {
                let node = self.doc.node(parent);
                !node.kind.is_text() && node.style.display == Display::Inline
            });
        }
        chain.reverse();
        chain
    }

    /// 该行的链接目标：按字符顺序找到第一个带 `link` 的内联祖先。
    ///
    /// 取最先出现字符的链上最内层链接（`<a>` 嵌套时内层赢），结果只依赖
    /// 行内容本身，所以同一份输入必然得到同样的 op。本轮**不合并**相邻行：
    /// 一行一个 `Op::Link`，便于逐行命中。
    fn line_link(&self, pieces: &[Piece], line: &[Ch]) -> Option<String> {
        for ch in line {
            let node = pieces[ch.piece as usize].node;
            let Some(owner) = self.owner_id(node) else {
                continue;
            };
            for id in self.owner_chain(owner).into_iter().rev() {
                if let Some(link) = self.doc.node(id).link.as_ref() {
                    return Some(link.clone());
                }
            }
        }
        None
    }

    fn line_entries(
        &self,
        pieces: &[Piece],
        line: &[Ch],
        base_style: ComputedStyle,
    ) -> Vec<LineBoxEntry> {
        let (_, _, top_dist, bottom_dist) = self.metrics(&base_style);
        let mut entries = Vec::new();
        entries.push(LineBoxEntry {
            owner: None,
            align: VerticalAlign::Baseline,
            shift: 0.0,
            top_dist,
            bottom_dist,
        });

        let mut owners: Vec<NodeId> = Vec::new();
        for ch in line {
            if let Some(owner) = self.owner_id(pieces[ch.piece as usize].node) {
                for id in self.owner_chain(owner) {
                    if !owners.contains(&id) {
                        owners.push(id);
                    }
                }
            }
        }

        for owner in owners {
            let mut parent_style = base_style;
            let mut parent_shift = 0.0;
            let mut entry = None;
            for id in self.owner_chain(owner) {
                let style = self.doc.node(id).style;
                let align = style.text.vertical_align;
                let (_, _, top_dist, bottom_dist) = self.metrics(&style);
                let shift = parent_shift + self.relative_shift(&style, &parent_style, align);
                let current = LineBoxEntry {
                    owner: Some(id),
                    align,
                    shift,
                    top_dist,
                    bottom_dist,
                };
                parent_style = style;
                parent_shift = shift;
                entry = Some(current);
            }
            if let Some(entry) = entry {
                if entry.owner == Some(owner) {
                    entries.push(entry);
                }
            }
        }

        entries
    }

    /// 按 UAX #9 的级给整段文本定方向（L1 的行尾空白复位由库负责）。
    ///
    /// 段落方向来自 `Dir`，逐字符的级存进 `Ch.level`，后面的整形分组和
    /// 行内重排都只看这一个值，不再回头看样式。
    fn resolve_bidi(&self, chars: &mut [Ch], base: Dir) {
        if chars.is_empty() {
            return;
        }
        let mut text = String::with_capacity(chars.len());
        let mut starts: Vec<usize> = Vec::with_capacity(chars.len());
        for ch in chars.iter() {
            starts.push(text.len());
            text.push(ch.c);
        }
        let level = if base == Dir::Rtl {
            unicode_bidi::Level::rtl()
        } else {
            unicode_bidi::Level::ltr()
        };
        let info = unicode_bidi::BidiInfo::new(&text, Some(level));
        for (index, ch) in chars.iter_mut().enumerate() {
            ch.level = info
                .levels
                .get(starts[index])
                .map(|level| level.number())
                .unwrap_or(level.number());
        }
    }

    /// 行的绘制顺序：逻辑下标按 UAX #9 L2 重排成视觉顺序。
    ///
    /// 重排以**簇**为单位，所以合字/组合字符不会被拆开；展开时簇内仍按逻辑
    /// 顺序，`Glyph.dx` 依次累加就是行内视觉位置。
    fn visual_order(&self, line: &[Ch]) -> Vec<usize> {
        let mut heads: Vec<(usize, unicode_bidi::Level)> = Vec::new();
        for (index, ch) in line.iter().enumerate() {
            if ch.head {
                let level = unicode_bidi::Level::new(ch.level)
                    .unwrap_or_else(|_| unicode_bidi::Level::ltr());
                heads.push((index, level));
            }
        }
        if heads.len() <= 1 {
            return (0..line.len()).collect();
        }
        let levels: Vec<unicode_bidi::Level> = heads.iter().map(|(_, level)| *level).collect();
        let order = unicode_bidi::BidiInfo::reorder_visual(&levels);
        let mut out = Vec::with_capacity(line.len());
        for &unit in &order {
            let head = heads[unit].0;
            let cluster = line[head].cluster;
            let mut index = head;
            while index < line.len() && line[index].cluster == cluster {
                out.push(index);
                index += 1;
            }
        }
        out
    }

    /// 注音排版：基文正常排，rt 用 0.5em 画在基线上方。
    ///
    /// rt 不参与断行宽度（`line_w` 里只有基文），字形宽度按基文簇宽平均分配，
    /// 所以注音始终对在基文上方；`Glyph.dy` 是相对基线的 -0.5em。
    fn ruby_run(
        &mut self,
        piece: &Piece,
        line: &[Ch],
        shaped: &Shaped,
        rt: Span,
        logical: &[usize],
        src_start: u32,
        baseline: f32,
        run_x: f32,
    ) -> Option<LaidRun> {
        let rt_text = self.doc.text_of(rt);
        if rt_text.is_empty() || logical.is_empty() {
            return None;
        }
        let size = piece.style.font.size * 0.5;
        if !(size > 0.0) {
            return None;
        }
        let rt_chars: Vec<char> = rt_text.chars().collect();
        let mut heads: Vec<(u32, f32)> = Vec::new();
        for &at in logical {
            if !line[at].head {
                continue;
            }
            let cluster = shaped.clusters[line[at].cluster as usize];
            heads.push((cluster.ch, cluster.advance));
        }
        if heads.is_empty() {
            return None;
        }
        let glyph_start = self.flow.glyphs.len() as u32;
        let mut count = 0u32;
        for (index, (ch, advance)) in heads.iter().enumerate() {
            let lo = index * rt_chars.len() / heads.len();
            let hi = (index + 1) * rt_chars.len() / heads.len();
            if hi <= lo {
                continue;
            }
            let each = advance / (hi - lo) as f32;
            for &c in &rt_chars[lo..hi] {
                let font = self.env.font_for(&piece.style.font, c);
                if !self.env.measurer.covers(c, font) {
                    continue;
                }
                self.flow.glyphs.push(Glyph {
                    gid: self.env.measurer.glyph_id(c, font),
                    dx: each,
                    dy: -size,
                    ch: ch.saturating_sub(src_start),
                });
                count += 1;
            }
        }
        if count == 0 {
            return None;
        }
        Some(LaidRun {
            font: self.env.measurer.select(&piece.style.font),
            size,
            color: piece.style.color,
            dir: Dir::Ltr,
            rotate: Rotate::None,
            origin: PointF::new(run_x, baseline),
            glyphs: Span::new(glyph_start, count),
            text: rt_text.into(),
        })
    }

    fn emit_line(
        &mut self,
        pieces: &[Piece],
        line: &[Ch],
        shaped: &Shaped,
        x0: f32,
        y: &mut f32,
        w: f32,
        indent: f32,
        hyphen: bool,
        base_style: ComputedStyle,
        block: u32,
        order: &mut u16,
    ) {
        if line.is_empty() {
            return;
        }
        let index = *order;
        *order = index.saturating_add(1);
        let brk = base_style.brk;

        let mut line_w = 0.0f32;
        for ch in line {
            if ch.head {
                line_w += shaped.clusters[ch.cluster as usize].advance;
            }
        }
        // 自动间距不插在行尾：最后一个簇的空隙在行宽里收回。
        let end_cluster = line.iter().rev().find(|ch| ch.head).map(|ch| ch.cluster);
        let gap_cut = end_cluster
            .map(|cluster| shaped.clusters[cluster as usize].gap)
            .unwrap_or(0.0);
        line_w -= gap_cut;

        // 行末连字符：独立一个 run，跟在最后一个字形后面。
        let last_char = line[line.len() - 1];
        let hyphen_style = pieces[last_char.piece as usize].style;
        let hyphen_font = self.font_for(&last_char, pieces);
        let hyphen_advance = if hyphen {
            self.env
                .measurer
                .advance('-', hyphen_font, hyphen_style.font.size)
                .max(0.0)
        } else {
            0.0
        };
        line_w += hyphen_advance;

        let mut entries = self.line_entries(pieces, line, base_style);
        let mut base_top = f32::INFINITY;
        let mut base_bottom = f32::NEG_INFINITY;
        let mut tallest = 0.0f32;
        for entry in &entries {
            if matches!(entry.align, VerticalAlign::Top | VerticalAlign::Bottom) {
                tallest = tallest.max(entry.top_dist + entry.bottom_dist);
            } else {
                base_top = base_top.min(entry.shift - entry.top_dist);
                base_bottom = base_bottom.max(entry.shift + entry.bottom_dist);
            }
        }
        // 行内图片按基线摆放：底边贴基线，太高的会把行盒往上顶。
        let mut image_top = 0.0f32;
        for ch in line {
            if ch.image.is_some() {
                if let PieceKind::Image { size, .. } = pieces[ch.piece as usize].kind {
                    let height = size.map(|s| s.h).unwrap_or(base_style.font.size * 1.5);
                    image_top = image_top.min(-height);
                }
            }
        }
        base_top = base_top.min(image_top);
        let line_h = (base_bottom - base_top).max(tallest).max(1.0);
        let line_top = base_top;
        let line_bottom = line_top + line_h;
        for entry in &mut entries {
            entry.shift = match entry.align {
                VerticalAlign::Top => line_top + entry.top_dist,
                VerticalAlign::Bottom => line_bottom - entry.bottom_dist,
                _ => entry.shift,
            };
        }

        let align_pad = match base_style.text.align {
            TextAlign::Center => (w - line_w) * 0.5,
            TextAlign::End => w - line_w,
            _ => 0.0,
        }
        .max(0.0);

        let start_x = x0 + indent + align_pad;
        let top = *y;
        let baseline = top - line_top;
        let mut cx = start_x;
        let mut runs: Vec<LaidRun> = Vec::new();
        let mut inline_ops: Vec<Op> = Vec::new();
        let mut missing: Vec<(NodeId, RectF)> = Vec::new();
        // 每个簇的末字形下标 + 是否空格簇，两端对齐按它在字形之间分余量。
        let mut ends: Vec<(u32, bool)> = Vec::new();

        let first = line[0];
        let last = line[line.len() - 1];
        let first_piece = &pieces[first.piece as usize];
        let last_piece = &pieces[last.piece as usize];
        let src_start = first_piece.span.start + first.off;
        let src_end = last_piece.span.start + last.off + last.c.len_utf8() as u32;
        let glyph_start_all = self.flow.glyphs.len() as u32;

        let visual = self.visual_order(line);
        let mut k = 0usize;
        while k < visual.len() {
            let at = visual[k];
            let piece_id = line[at].piece;
            let piece = &pieces[piece_id as usize];
            let style = piece.style;
            let shift = self
                .owner_id(piece.node)
                .and_then(|owner| {
                    entries
                        .iter()
                        .find(|entry| entry.owner == Some(owner))
                        .map(|entry| entry.shift)
                })
                .unwrap_or(0.0);
            let run_baseline = baseline + shift;
            if let Some(id) = line[at].image {
                // 行内图片：底边贴基线，宽度就是断行时算进去的那个宽度。
                let cluster = shaped.clusters[line[at].cluster as usize];
                let size = match piece.kind {
                    PieceKind::Image { size, .. } => size,
                    _ => None,
                };
                let height = size.map(|s| s.h).unwrap_or(base_style.font.size * 1.5);
                let rect = RectF::new(cx, run_baseline - height, cluster.advance, height);
                match size {
                    Some(_) => inline_ops.push(Op::Image {
                        id,
                        rect,
                        fit: Fit::Contain,
                    }),
                    None => {
                        // 尺寸依赖内容：发占位并记诊断，行高保持可预测。
                        inline_ops.push(Op::Unresolved(Unresolved {
                            node: piece.node,
                            rect,
                            reason: UnresolvedReason::SizeFromContent,
                        }));
                        self.flow.dirty.push(DirtyBlock {
                            node: piece.node,
                            rect,
                            reason: UnresolvedReason::SizeFromContent,
                        });
                    }
                }
                cx += cluster.advance;
                k += 1;
                continue;
            }
            let font = self.font_for(&line[at], pieces);
            let covered = self.env.measurer.covers(line[at].c, font);
            let rtl = line[at].level % 2 == 1;
            let glyph_start = self.flow.glyphs.len() as u32;
            let run_x = cx;
            let mut logical: Vec<usize> = Vec::new();
            while k < visual.len() {
                let current = visual[k];
                if line[current].piece != piece_id {
                    break;
                }
                let next_font = self.font_for(&line[current], pieces);
                let next_covered = self.env.measurer.covers(line[current].c, next_font);
                if next_font != font
                    || next_covered != covered
                    || (line[current].level % 2 == 1) != rtl
                {
                    break;
                }
                if line[current].head {
                    let cluster = shaped.clusters[line[current].cluster as usize];
                    let cut = if Some(line[current].cluster) == end_cluster {
                        gap_cut
                    } else {
                        0.0
                    };
                    if covered {
                        let start = cluster.glyph_start as usize;
                        let end = start + cluster.glyph_len as usize;
                        for glyph in &shaped.glyphs[start..end] {
                            self.flow.glyphs.push(Glyph {
                                gid: glyph.gid,
                                dx: glyph.dx,
                                dy: glyph.dy,
                                ch: cluster.ch.saturating_sub(src_start),
                            });
                        }
                        if cluster.glyph_len > 0 {
                            ends.push((self.flow.glyphs.len() as u32 - 1, line[current].c == ' '));
                            if cut != 0.0 {
                                if let Some(last) = self.flow.glyphs.last_mut() {
                                    last.dx -= cut;
                                }
                            }
                        }
                    }
                    cx += cluster.advance - cut;
                }
                logical.push(current);
                k += 1;
            }
            logical.sort_unstable();
            let mut text = String::new();
            for &current in &logical {
                text.push(line[current].c);
            }
            if covered {
                runs.push(LaidRun {
                    font,
                    size: style.font.size,
                    color: style.color,
                    dir: if rtl { Dir::Rtl } else { Dir::Ltr },
                    rotate: Rotate::None,
                    origin: PointF::new(run_x, run_baseline),
                    glyphs: Span::new(glyph_start, self.flow.glyphs.len() as u32 - glyph_start),
                    text,
                });
                if let PieceKind::Ruby { rt } = piece.kind {
                    if let Some(run) = self.ruby_run(
                        piece,
                        line,
                        shaped,
                        rt,
                        &logical,
                        src_start,
                        run_baseline,
                        run_x,
                    ) {
                        runs.push(run);
                    }
                }
            } else {
                missing.push((
                    piece.node,
                    RectF::new(run_x, top, (cx - run_x).max(0.0), line_h),
                ));
            }
        }

        if hyphen {
            let piece = &pieces[last_char.piece as usize];
            let shift = self
                .owner_id(piece.node)
                .and_then(|owner| {
                    entries
                        .iter()
                        .find(|entry| entry.owner == Some(owner))
                        .map(|entry| entry.shift)
                })
                .unwrap_or(0.0);
            let run_x = cx;
            let covered = self.env.measurer.covers('-', hyphen_font);
            if covered {
                let glyph_start = self.flow.glyphs.len() as u32;
                // 连字符不属于原文：`ch` 停在行末最后一个字符上，选区不会越界。
                let ch = (src_end - src_start).saturating_sub(1);
                self.flow.glyphs.push(Glyph {
                    gid: self.env.measurer.glyph_id('-', hyphen_font),
                    dx: hyphen_advance,
                    dy: 0.0,
                    ch,
                });
                runs.push(LaidRun {
                    font: hyphen_font,
                    size: hyphen_style.font.size,
                    color: hyphen_style.color,
                    dir: Dir::Ltr,
                    rotate: Rotate::None,
                    origin: PointF::new(run_x, baseline + shift),
                    glyphs: Span::new(glyph_start, 1),
                    text: String::from("-"),
                });
            } else {
                missing.push((piece.node, RectF::new(run_x, top, hyphen_advance, line_h)));
            }
            cx += hyphen_advance;
        }

        let anchor = self
            .doc
            .node(first_piece.node)
            .src
            .as_ref()
            .map(|a| a.with_offset(first.off));
        let glyphs = Span::new(
            glyph_start_all,
            self.flow.glyphs.len() as u32 - glyph_start_all,
        );
        let link = self.line_link(pieces, line);

        let mut unresolved: Vec<Op> = inline_ops;
        for (node, rect) in missing {
            unresolved.push(Op::Unresolved(Unresolved {
                node,
                rect,
                reason: UnresolvedReason::FontMissing,
            }));
            self.flow.dirty.push(DirtyBlock {
                node,
                rect,
                reason: UnresolvedReason::FontMissing,
            });
        }

        // 两端对齐的余量：有空格就分给空格，没有空格就把字间隙拉开（末字形除外）。
        let has_space = ends.iter().any(|(_, space)| *space);
        let stretch: Vec<u32> = if has_space {
            ends.iter()
                .filter(|(_, space)| *space)
                .map(|(glyph, _)| *glyph)
                .collect()
        } else {
            let last = ends.last().map(|(glyph, _)| *glyph);
            ends.iter()
                .map(|(glyph, _)| *glyph)
                .filter(|glyph| Some(*glyph) != last)
                .collect()
        };

        self.flow.items.push(Item::Line(LaidLine {
            y: top,
            x: start_x,
            w: cx - start_x,
            h: line_h,
            baseline,
            dir: base_style.dir,
            src: Span::new(src_start, src_end.saturating_sub(src_start)),
            anchor,
            glyphs,
            node: first_piece.node,
            link,
            runs,
            unresolved,
            stretch,
            block,
            order: index,
            len: 0,
            orphans: brk.orphans,
            widows: brk.widows,
            force: index == 0 && brk.before == Break::Always,
            avoid_before: index == 0 && brk.before == Break::Avoid,
            avoid_after: false,
        }));
        *y = top + line_h;
    }
}
