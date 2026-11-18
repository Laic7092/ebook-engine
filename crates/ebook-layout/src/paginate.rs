use alloc::vec::Vec;
use ebook_ir::{EdgesF, Op, Page, PointF, SizeF};

#[derive(Clone, Copy)]
pub(crate) struct Unit {
    pub(crate) y: f32,
    pub(crate) h: f32,
    pub(crate) line: bool,
    pub(crate) block: u32,
    pub(crate) order: u16,
    pub(crate) len: u16,
    pub(crate) orphans: u16,
    pub(crate) widows: u16,
    pub(crate) force: bool,
    pub(crate) avoid_before: bool,
    pub(crate) avoid_after: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PageStart {
    pub(crate) unit: u32,
    pub(crate) y: f32,
}

pub(crate) fn plan(units: &[Unit], page_h: f32) -> Vec<PageStart> {
    let mut starts = Vec::new();
    starts.push(PageStart { unit: 0, y: 0.0 });
    let n = units.len();
    if n == 0 {
        return starts;
    }
    let h = page_h.max(1.0);
    let mut s = 0usize;
    let mut top = 0.0f32;

    while s < n {
        let limit = top + h + 0.001;
        let mut e = s;
        while e < n {
            if e > s && units[e].force {
                break;
            }
            if units[e].y + units[e].h > limit {
                if e == s {
                    e += 1;
                }
                break;
            }
            e += 1;
        }
        if e >= n {
            break;
        }

        let cut = resolve(units, s, e).clamp(s + 1, n);
        if cut >= n {
            break;
        }
        top = units[cut].y.max(top + 1.0);
        starts.push(PageStart { unit: cut as u32, y: top });
        s = cut;
    }

    starts
}

fn resolve(units: &[Unit], s: usize, greedy: usize) -> usize {
    let mut cut = greedy;
    while cut > s + 1 {
        let next = step(units, s, cut);
        if next >= cut {
            break;
        }
        cut = next.max(s + 1);
    }
    cut
}

fn step(units: &[Unit], s: usize, cut: usize) -> usize {
    let prev = units[cut - 1];
    let next = units[cut];
    if prev.line && next.line && prev.block == next.block && prev.len > 0 {
        return inside(units, s, cut, &prev);
    }
    if (prev.line && prev.avoid_after) || (next.line && next.avoid_before) {
        return (cut - 1).max(s + 1);
    }
    cut
}

fn inside(units: &[Unit], s: usize, cut: usize, prev: &Unit) -> usize {
    let total = prev.len as usize;
    let before = prev.order as usize + 1;
    if before > total {
        return cut;
    }
    let first = block_first(units, s, cut, prev.block);
    let on_page = cut - first;
    let carried = before.saturating_sub(on_page);
    let orphans = prev.orphans.max(1) as usize;
    let widows = prev.widows.max(1) as usize;

    let mut keep = before;
    if total - keep < widows {
        keep = total.saturating_sub(widows);
    }
    if keep < carried + orphans {
        keep = 0;
    }
    if keep == before {
        return cut;
    }
    if keep == 0 {
        return if first > s { first } else { cut };
    }
    let moved = back_lines(units, s, cut, before - keep);
    if moved > s {
        moved
    } else {
        cut
    }
}

fn block_first(units: &[Unit], s: usize, cut: usize, block: u32) -> usize {
    let mut first = cut;
    while first > s && units[first - 1].line && units[first - 1].block == block {
        first -= 1;
    }
    first
}

fn back_lines(units: &[Unit], s: usize, cut: usize, count: usize) -> usize {
    let mut c = cut;
    let mut left = count;
    while c > s + 1 && left > 0 {
        c -= 1;
        if units[c].line {
            left -= 1;
        }
    }
    c
}

pub(crate) fn page_of(starts: &[PageStart], index: usize) -> u32 {
    let found = starts.partition_point(|s| s.unit as usize <= index);
    (found.max(1) - 1) as u32
}

pub(crate) struct OpenPage {
    pub(crate) index: u32,
    pub(crate) top: f32,
    pub(crate) op_start: u32,
    pub(crate) line_start: u32,
    pub(crate) anchor: Option<ebook_ir::Anchor>,
}

pub(crate) struct PageBuilder {
    pub(crate) pages: Vec<Page>,
    pub(crate) page_size: SizeF,
    inset: EdgesF,
    open: Option<OpenPage>,
}

impl PageBuilder {
    pub(crate) fn new(page_size: SizeF, inset: EdgesF) -> Self {
        Self { pages: Vec::new(), page_size, inset, open: None }
    }

    pub(crate) fn sync(&mut self, page: u32, top: f32, ops_len: u32, lines_len: u32) {
        let same = self.open.as_ref().is_some_and(|o| o.index == page);
        if same {
            return;
        }
        self.close(ops_len, lines_len);
        self.open = Some(OpenPage {
            index: page,
            top,
            op_start: ops_len,
            line_start: lines_len,
            anchor: None,
        });
    }

    pub(crate) fn set_anchor(&mut self, anchor: Option<&ebook_ir::Anchor>) {
        if let Some(o) = self.open.as_mut() {
            if o.anchor.is_none() {
                o.anchor = anchor.cloned();
            }
        }
    }

    pub(crate) fn close(&mut self, ops_len: u32, lines_len: u32) {
        let Some(o) = self.open.take() else { return };
        let op_len = ops_len.saturating_sub(o.op_start);
        let line_len = lines_len.saturating_sub(o.line_start);
        if op_len == 0 && line_len == 0 {
            return;
        }
        self.pages.push(Page {
            index: o.index,
            origin: PointF::new(0.0, o.top),
            size: self.page_size,
            inset: self.inset,
            ops: ebook_ir::Span::new(o.op_start, op_len),
            lines: ebook_ir::Span::new(o.line_start, line_len),
            anchor: o.anchor,
        });
    }
}

pub(crate) fn op_bounds(op: &Op) -> (f32, f32) {
    match op {
        Op::GlyphRun(run) => (run.origin.y - run.size, run.size * 1.4),
        Op::Rect { rect, .. } => (rect.y, rect.h),
        Op::Border(border) => (border.rect.y, border.rect.h),
        Op::Image { rect, .. } => (rect.y, rect.h),
        Op::Unresolved(u) => (u.rect.y, u.rect.h),
        Op::Clip { rect, .. } => (rect.y, rect.h),
        // 链接热区与行盒同形；它总是跟着 `Item::Line` 一起进 ops，不会单独成为分页单元。
        Op::Link { rect, .. } => (rect.y, rect.h),
    }
}
