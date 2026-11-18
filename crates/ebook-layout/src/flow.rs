use alloc::string::String;
use alloc::vec::Vec;
use ebook_ir::{
    Anchor, Break, ComputedStyle, Dir, DirtyBlock, Display, Fit, FontId, Glyph, ImageId, NodeId,
    NodeKind, Op, PointF, RectF, Rotate, Span, TextAlign, TextTransform, Unresolved,
    UnresolvedReason, VerticalAlign, WhiteSpace,
};

use crate::resolve;
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
    pub(crate) runs: Vec<LaidRun>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum PieceKind {
    Text,
    Break,
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
}

#[derive(Clone, Copy, Default)]
struct CollapsedMargin {
    positive: f32,
    negative: f32,
}

impl CollapsedMargin {
    const ZERO: Self = Self { positive: 0.0, negative: 0.0 };

    fn new(value: f32) -> Self {
        Self { positive: value.max(0.0), negative: value.min(0.0) }
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
        if !matches!(node.style.text.white_space, WhiteSpace::Normal | WhiteSpace::Nowrap) {
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
            return BlockFlow { margin: CollapsedMargin::ZERO, empty: true };
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
            NodeKind::Ruby { base, .. } => {
                let mut pieces = Vec::new();
                pieces.push(Piece {
                    style,
                    span: base,
                    node: id,
                    kind: PieceKind::Text,
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
        BlockFlow { margin: margin_bottom, empty: false }
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
            NodeKind::Image(_) | NodeKind::Ruby { .. } => {}
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

    fn flatten(&self, pieces: &[Piece], out: &mut Vec<Ch>) {
        let mut prev_space = true;
        for (pi, piece) in pieces.iter().enumerate() {
            match piece.kind {
                PieceKind::Break => {
                    out.push(Ch {
                        c: '\n',
                        off: 0,
                        piece: pi as u16,
                    });
                    prev_space = false;
                }
                PieceKind::Text => {
                    let text = self.doc.text_of(piece.span);
                    let collapse = matches!(
                        piece.style.text.white_space,
                        WhiteSpace::Normal | WhiteSpace::Nowrap
                    );
                    let transform = piece.style.text.transform;
                    for (off, raw) in text.char_indices() {
                        let c = match raw {
                            '\n' | '\r' | '\t' => ' ',
                            c => c,
                        };
                        if collapse {
                            if c == ' ' {
                                if prev_space {
                                    continue;
                                }
                                prev_space = true;
                            } else {
                                prev_space = false;
                            }
                        }
                        match transform {
                            TextTransform::None => out.push(Ch {
                                c,
                                off: off as u32,
                                piece: pi as u16,
                            }),
                            TextTransform::Uppercase => {
                                for t in c.to_uppercase() {
                                    out.push(Ch {
                                        c: t,
                                        off: off as u32,
                                        piece: pi as u16,
                                    });
                                }
                            }
                            TextTransform::Lowercase => {
                                for t in c.to_lowercase() {
                                    out.push(Ch {
                                        c: t,
                                        off: off as u32,
                                        piece: pi as u16,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn advance(&self, ch: &Ch, pieces: &[Piece]) -> f32 {
        let style = &pieces[ch.piece as usize].style;
        let base = self
            .env
            .measurer
            .advance(ch.c, style.font.font, style.font.size);
        let spacing = style.text.letter_spacing
            + if ch.c == ' ' {
                style.text.word_spacing
            } else {
                0.0
            };
        (base + spacing).max(0.0)
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
        let block = self.next_block;
        self.next_block += 1;
        let item_start = self.flow.items.len();
        let mut order: u16 = 0;

        let base_style = container;
        let em = base_style.font.size;
        let indent = base_style
            .text
            .indent
            .resolve(w, em)
            .unwrap_or(0.0)
            .clamp(0.0, w);

        let nowrap = base_style.text.white_space == WhiteSpace::Nowrap
            || pieces
                .iter()
                .any(|p| p.style.text.white_space == WhiteSpace::Nowrap);
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
                    x0,
                    y,
                    w,
                    pad,
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
            let adv = self.advance(&chars[i], pieces);
            if !nowrap && used + adv > w + 0.001 && i > line_start {
                let brk = last_break
                    .filter(|&b| b > line_start && b <= i)
                    .unwrap_or(i);
                let pad = if first { indent } else { 0.0 };
                self.emit_line(
                    pieces,
                    &chars[line_start..brk],
                    x0,
                    y,
                    w,
                    pad,
                    base_style,
                    block,
                    &mut order,
                );
                first = false;
                line_start = brk;
                while line_start < chars.len() && chars[line_start].c == ' ' {
                    line_start += 1;
                }
                i = line_start;
                used = 0.0;
                last_break = None;
                continue;
            }
            used += adv;
            if i + 1 < chars.len()
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
                x0,
                y,
                w,
                pad,
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
        let (runs, line_x, line_w) = match &self.flow.items[index] {
            Item::Line(line) => (line.runs.clone(), line.x, line.w),
            _ => return,
        };
        let target = (w - (line_x - x0)).max(0.0);
        let extra = target - line_w;
        if extra <= 0.001 || runs.is_empty() {
            return;
        }
        let has_space = runs.iter().any(|run| run.text.contains(' '));
        let last_glyph = runs.last().map(|run| run.glyphs.end() as usize);
        let mut eligible: Vec<usize> = Vec::new();
        for run in &runs {
            if has_space {
                for (offset, ch) in run.text.chars().enumerate() {
                    if ch == ' ' {
                        eligible.push(run.glyphs.start as usize + offset);
                    }
                }
            } else {
                for glyph in run.glyphs.as_range() {
                    if Some(glyph + 1) != last_glyph {
                        eligible.push(glyph);
                    }
                }
            }
        }
        if eligible.is_empty() {
            return;
        }
        let n = eligible.len() as f32;
        let each = extra / n;
        let mut origins = Vec::with_capacity(runs.len());
        let mut shift = 0.0f32;
        let mut next = 0usize;
        for run in &runs {
            origins.push(run.origin.x + shift);
            for glyph in run.glyphs.as_range() {
                if next < eligible.len() && eligible[next] == glyph {
                    let delta = if next + 1 == eligible.len() {
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
        let ascent = self.env.measurer.ascent(style.font.font, size);
        let descent = self.env.measurer.descent(style.font.font, size);
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
                self.env.measurer.x_height(parent.font.font, parent.font.size) * 0.5
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

    fn emit_line(
        &mut self,
        pieces: &[Piece],
        line: &[Ch],
        x0: f32,
        y: &mut f32,
        w: f32,
        indent: f32,
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
            line_w += self.advance(ch, pieces);
        }

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
        let mut missing: Vec<(NodeId, RectF)> = Vec::new();
        let mut k = 0usize;

        while k < line.len() {
            let piece_id = line[k].piece;
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
            let covered = self.env.measurer.covers(line[k].c, style.font.font);
            let glyph_start = self.flow.glyphs.len() as u32;
            let run_x = cx;
            let mut text = String::new();
            while k < line.len()
                && line[k].piece == piece_id
                && self.env.measurer.covers(line[k].c, style.font.font) == covered
            {
                let adv = self.advance(&line[k], pieces);
                if covered {
                    self.flow.glyphs.push(Glyph {
                        gid: self.env.measurer.glyph_id(line[k].c, style.font.font),
                        dx: adv,
                        dy: 0.0,
                    });
                    text.push(line[k].c);
                }
                cx += adv;
                k += 1;
            }
            if covered {
                runs.push(LaidRun {
                    font: style.font.font,
                    size: style.font.size,
                    color: style.color,
                    dir: style.dir,
                    rotate: Rotate::None,
                    origin: PointF::new(run_x, run_baseline),
                    glyphs: Span::new(glyph_start, self.flow.glyphs.len() as u32 - glyph_start),
                    text,
                });
            } else {
                missing.push((
                    piece.node,
                    RectF::new(run_x, top, (cx - run_x).max(0.0), line_h),
                ));
            }
        }

        let first = line[0];
        let last = line[line.len() - 1];
        let first_piece = &pieces[first.piece as usize];
        let last_piece = &pieces[last.piece as usize];
        let src_start = first_piece.span.start + first.off;
        let src_end = last_piece.span.start + last.off + last.c.len_utf8() as u32;
        let anchor = self
            .doc
            .node(first_piece.node)
            .src
            .as_ref()
            .map(|a| a.with_offset(first.off));

        let mut unresolved = Vec::new();
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

        self.flow.items.push(Item::Line(LaidLine {
            y: top,
            x: start_x,
            w: cx - start_x,
            h: line_h,
            baseline,
            dir: base_style.dir,
            src: Span::new(src_start, src_end.saturating_sub(src_start)),
            anchor,
            runs,
            unresolved,
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
