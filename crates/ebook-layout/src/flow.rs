use alloc::vec::Vec;
use ebook_ir::{
    Anchor, Break, ComputedStyle, Dir, Display, DirtyBlock, Fit, FontId, Glyph, ImageId, NodeId,
    NodeKind, Op, PointF, RectF, Rotate, Span, TextAlign, Unresolved, UnresolvedReason,
    WhiteSpace, WordBreak,
};

use crate::measure::is_wide;
use crate::resolve;
use crate::{LayoutEnv, PageConstraint};

#[derive(Clone, Copy, Debug)]
pub(crate) struct LaidRun {
    pub(crate) font: FontId,
    pub(crate) size: f32,
    pub(crate) color: ebook_ir::Rgba8,
    pub(crate) dir: Dir,
    pub(crate) rotate: Rotate,
    pub(crate) origin: PointF,
    pub(crate) glyphs: Span,
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

struct FlowCtx<'a> {
    doc: &'a ebook_ir::StyledDoc,
    env: &'a LayoutEnv<'a>,
    content_w: f32,
    flow: Flow,
    depth: u32,
    next_block: u32,
}

pub(crate) fn flow(
    doc: &ebook_ir::StyledDoc,
    env: &LayoutEnv<'_>,
    c: &PageConstraint,
) -> Flow {
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
    ctx.block(doc.root, 0.0, &mut y, root_w, 0.0);
    ctx.flow
}

impl<'a> FlowCtx<'a> {
    fn is_inline_node(&self, id: NodeId) -> bool {
        let node = self.doc.node(id);
        node.kind.is_text() || node.style.is_inline()
    }

    fn block(&mut self, id: NodeId, x: f32, y: &mut f32, avail_w: f32, prev_mb: f32) -> f32 {
        self.depth += 1;
        if self.depth > 128 {
            self.depth -= 1;
            return 0.0;
        }

        let node = self.doc.node(id);
        let style = node.style;
        let kind = node.kind.clone();
        let em = style.font.size;
        let margin = resolve::sides(&style.margin, avail_w, em);
        let padding = resolve::sides(&style.padding, avail_w, em);
        let border = resolve::sides(&style.border.width, avail_w, em);

        *y += margin.top.max(prev_mb);

        let mut cy = *y + border.top + padding.top;
        let inner_x = x + margin.left + border.left + padding.left;
        let inner_w = match style.width.resolve(avail_w, em) {
            Some(w) => w.max(0.0),
            None => (avail_w - margin.horizontal() - border.horizontal() - padding.horizontal())
                .max(0.0),
        };

        match kind {
            NodeKind::Text(_) => {
                self.inline_from(&[id], inner_x, &mut cy, inner_w, style);
            }
            NodeKind::LineBreak => {}
            NodeKind::Image(img) => {
                self.image(id, img, style, inner_x, &mut cy, inner_w);
            }
            NodeKind::Ruby { base, .. } => {
                let mut pieces = Vec::new();
                pieces.push(Piece {
                    style,
                    span: base,
                    node: id,
                    kind: PieceKind::Text,
                });
                self.break_lines(&pieces, inner_x, &mut cy, inner_w, style);
            }
            NodeKind::Element(_) => {
                let child_len = self.doc.node(id).child_len;
                let first_child = self.doc.node(id).first_child;
                let mut buf: Vec<NodeId> = Vec::new();
                let mut last_mb = 0.0;
                for i in 0..child_len {
                    let kid = self.doc.children[(first_child + i) as usize];
                    let kid_style = self.doc.node(kid).style;
                    if kid_style.display == Display::None {
                        continue;
                    }
                    if self.is_inline_node(kid) {
                        buf.push(kid);
                        continue;
                    }
                    if !buf.is_empty() {
                        self.inline_from(&buf, inner_x, &mut cy, inner_w, style);
                        buf.clear();
                        last_mb = 0.0;
                    }
                    last_mb = self.block(kid, inner_x, &mut cy, inner_w, last_mb);
                }
                if !buf.is_empty() {
                    self.inline_from(&buf, inner_x, &mut cy, inner_w, style);
                }
            }
        }

        *y = cy + border.bottom + padding.bottom;
        self.depth -= 1;
        margin.bottom
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
        let margin = resolve::sides(&style.margin, avail_w, em);
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

    fn image_size(&self, img: ImageId, style: &ComputedStyle, avail_w: f32) -> Option<ebook_ir::SizeF> {
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
                    out.push(Ch { c: '\n', off: 0, piece: pi as u16 });
                    prev_space = false;
                }
                PieceKind::Text => {
                    let text = self.doc.text_of(piece.span);
                    let collapse = matches!(
                        piece.style.text.white_space,
                        WhiteSpace::Normal | WhiteSpace::Nowrap
                    );
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
                        out.push(Ch { c, off: off as u32, piece: pi as u16 });
                    }
                }
            }
        }
    }

    fn advance(&self, ch: &Ch, pieces: &[Piece]) -> f32 {
        let style = &pieces[ch.piece as usize].style;
        let base = self.env.measurer.advance(ch.c, style.font.font, style.font.size);
        let spacing = style.text.letter_spacing
            + if ch.c == ' ' { style.text.word_spacing } else { 0.0 };
        (base + spacing).max(0.0)
    }

    fn can_break(&self, chars: &[Ch], i: usize, pieces: &[Piece]) -> bool {
        let a = chars[i];
        let b = chars[i + 1];
        if b.c == '\n' {
            return true;
        }
        if pieces[a.piece as usize].style.text.word_break == WordBreak::BreakAll {
            return true;
        }
        if a.c == ' ' {
            return true;
        }
        if b.c == ' ' {
            return false;
        }
        if is_wide(a.c) || is_wide(b.c) {
            return !no_start(b.c) && !no_end(a.c);
        }
        matches!(a.c, '-' | '/' | '\u{2013}' | '\u{2014}')
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

        let mut line_start = 0usize;
        let mut last_break: Option<usize> = None;
        let mut used = indent;
        let mut i = 0usize;
        let mut first = true;

        while i < chars.len() {
            if chars[i].c == '\n' {
                let pad = if first { indent } else { 0.0 };
                self.emit_line(pieces, &chars[line_start..i], x0, y, w, pad, base_style, block, &mut order);
                first = false;
                line_start = i + 1;
                used = 0.0;
                last_break = None;
                i += 1;
                continue;
            }
            let adv = self.advance(&chars[i], pieces);
            if used + adv > w + 0.001 && i > line_start {
                let brk = last_break.filter(|&b| b > line_start && b <= i).unwrap_or(i);
                let pad = if first { indent } else { 0.0 };
                self.emit_line(pieces, &chars[line_start..brk], x0, y, w, pad, base_style, block, &mut order);
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
            if i + 1 < chars.len() && self.can_break(&chars, i, pieces) {
                last_break = Some(i + 1);
            }
            i += 1;
        }

        if line_start < chars.len() {
            let pad = if first { indent } else { 0.0 };
            self.emit_line(pieces, &chars[line_start..], x0, y, w, pad, base_style, block, &mut order);
        }

        let count = (self.flow.items.len() - item_start).min(u16::MAX as usize) as u16;
        if count == 0 {
            return;
        }
        for item in &mut self.flow.items[item_start..] {
            if let Item::Line(line) = item {
                line.len = count;
            }
        }
        if let Some(Item::Line(line)) = self.flow.items.last_mut() {
            line.avoid_after = container.brk.after == Break::Avoid;
        }
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

        let mut max_size = 0.0f32;
        let mut max_ascent = 0.0f32;
        let mut max_descent = 0.0f32;
        let mut line_w = 0.0f32;
        for ch in line {
            let style = pieces[ch.piece as usize].style;
            max_size = max_size.max(style.font.size);
            max_ascent = max_ascent.max(self.env.measurer.ascent(style.font.font, style.font.size));
            max_descent =
                max_descent.max(self.env.measurer.descent(style.font.font, style.font.size));
            line_w += self.advance(ch, pieces);
        }

        let line_h = resolve::line_px(base_style.line_height, max_size)
            .max(max_ascent + max_descent)
            .max(1.0);
        let align_pad = match base_style.text.align {
            TextAlign::Center => (w - line_w) * 0.5,
            TextAlign::End => w - line_w,
            _ => 0.0,
        }
        .max(0.0);

        let start_x = x0 + indent + align_pad;
        let top = *y;
        let baseline = top + max_ascent;
        let mut cx = start_x;
        let mut runs: Vec<LaidRun> = Vec::new();
        let mut k = 0usize;

        while k < line.len() {
            let piece_id = line[k].piece;
            let style = pieces[piece_id as usize].style;
            let glyph_start = self.flow.glyphs.len() as u32;
            let run_x = cx;
            while k < line.len() && line[k].piece == piece_id {
                let adv = self.advance(&line[k], pieces);
                self.flow.glyphs.push(Glyph {
                    gid: glyph_id(line[k].c),
                    dx: adv,
                    dy: 0.0,
                });
                cx += adv;
                k += 1;
            }
            runs.push(LaidRun {
                font: style.font.font,
                size: style.font.size,
                color: style.color,
                dir: style.dir,
                rotate: Rotate::None,
                origin: PointF::new(run_x, baseline),
                glyphs: Span::new(glyph_start, self.flow.glyphs.len() as u32 - glyph_start),
            });
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

fn glyph_id(c: char) -> u16 {
    let v = c as u32;
    if v <= 0xFFFF {
        v as u16
    } else {
        0xFFFD
    }
}

fn no_start(c: char) -> bool {
    matches!(
        c,
        '\u{3001}' | '\u{3002}' | '\u{FF0C}' | '\u{FF0E}' | '\u{FF1A}' | '\u{FF1B}' | '\u{FF1F}'
            | '\u{FF01}' | '\u{FF09}' | '\u{300D}' | '\u{300F}' | '\u{3011}' | '\u{300B}'
            | '\u{3009}' | '\u{201D}' | '\u{2019}' | '\u{2026}' | '\u{2014}' | '\u{00B7}'
            | '\u{FF05}' | '\u{FF3D}' | '\u{FF5D}'
    )
}

fn no_end(c: char) -> bool {
    matches!(
        c,
        '\u{FF08}' | '\u{300C}' | '\u{300E}' | '\u{3010}' | '\u{300A}' | '\u{3008}' | '\u{201C}'
            | '\u{2018}' | '\u{FF3B}' | '\u{FF5B}'
    )
}
