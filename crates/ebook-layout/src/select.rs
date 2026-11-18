//! 选区：两个 `Location` → 分页矩形 / 原文片段。
//!
//! 矩形来自字形前进宽度，不来自像素：行内按 `Glyph.ch` 把字节区间切成字形区间，
//! 行首/行尾半选夹到字形边界（整字形选中）。跨页时逐页返回，rect 是**页内坐标**。
//!
//! `LineBox.dir == Rtl` 时同一行的矩形按视觉顺序（从右到左）返回，但
//! `Location.byte` 始终是逻辑序（行内 `src` 的字节偏移）。

use alloc::string::String;
use alloc::vec::Vec;

use ebook_ir::{Dir, GlyphRun, LayoutResult, LineBox, Location, RectF, StyledDoc};

use crate::hit::Reader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub start: Location,
    pub end: Location,
}

impl Selection {
    pub fn new(start: Location, end: Location) -> Self {
        Self { start, end }
    }

    /// 按 (行号, 字节) 排成 (前, 后)。反向选择先过这一层。
    pub fn normalize(&self) -> (Location, Location) {
        let (a, b) = (self.start, self.end);
        if (a.line, a.byte) <= (b.line, b.byte) {
            (a, b)
        } else {
            (b, a)
        }
    }

    pub fn is_empty(&self) -> bool {
        let (a, b) = self.normalize();
        a.line == b.line && a.byte == b.byte
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionRect {
    pub page: u32,
    pub rect: RectF,
}

pub(crate) fn selection_rects(reader: &Reader<'_>, sel: &Selection) -> Vec<SelectionRect> {
    let ir = reader.ir();
    let mut out = Vec::new();
    if sel.is_empty() {
        return out;
    }
    let (a, b) = sel.normalize();
    let a = reader.clamp(a);
    let b = reader.clamp(b);
    if a.line == b.line && a.byte == b.byte {
        return out;
    }

    let mut cached_page: Option<u32> = None;
    let mut cached_runs: Vec<&GlyphRun> = Vec::new();
    for index in a.line..=b.line {
        let Some(line) = ir.line(index) else { break };
        let Some(page) = ir.page_of_line(index) else {
            continue;
        };
        if cached_page != Some(page.index) {
            cached_runs = glyph_runs(ir, page.index);
            cached_page = Some(page.index);
        }
        let lo = if index == a.line { a.byte } else { 0 }.min(line.src.len);
        let hi = if index == b.line {
            b.byte
        } else {
            line.src.len
        }
        .min(line.src.len);
        if hi <= lo {
            continue;
        }

        let cells = line_cells(ir, line, &cached_runs);
        let mut segments = selected_segments(&cells, line, lo, hi);
        if segments.is_empty() {
            // 整行缺字形（缺字/字体缺失）时退回行盒，让选区仍然可见且确定。
            segments.push((line.rect.x, line.rect.right()));
        }
        if line.dir == Dir::Rtl {
            segments.sort_by(|p, q| q.0.total_cmp(&p.0));
        }
        for (x0, x1) in segments {
            let x = x0.min(x1) - page.origin.x + page.inset.left;
            let y = line.rect.y - page.origin.y + page.inset.top;
            let rect = RectF::new(x, y, (x1 - x0).abs(), line.rect.h);
            if rect.w > 0.0 {
                out.push(SelectionRect {
                    page: page.index,
                    rect,
                });
            }
        }
    }
    out
}

pub(crate) fn selection_text(reader: &Reader<'_>, doc: &StyledDoc, sel: &Selection) -> String {
    let ir = reader.ir();
    let mut out = String::new();
    if sel.is_empty() {
        return out;
    }
    let (a, b) = sel.normalize();
    let a = reader.clamp(a);
    let b = reader.clamp(b);
    for index in a.line..=b.line {
        let Some(line) = ir.line(index) else { break };
        let lo = if index == a.line { a.byte } else { 0 }.min(line.src.len);
        let hi = if index == b.line {
            b.byte
        } else {
            line.src.len
        }
        .min(line.src.len);
        if hi <= lo {
            continue;
        }
        let start = (line.src.start + lo) as usize;
        let end = (line.src.start + hi) as usize;
        let Some(fragment) = doc.text.get(start..end) else {
            continue;
        };
        if fragment.is_empty() {
            continue;
        }
        // 行与行之间源码里没有硬换行（是排版时断开的），用 '\n' 还原可读性。
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(fragment);
    }
    out
}

/// 页内所有字形运行（按 op 顺序）。
fn glyph_runs(ir: &LayoutResult, page: u32) -> Vec<&GlyphRun> {
    let Some(page_ref) = ir.page(page) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    for op in ir.display.op_slice(page_ref.ops) {
        if let ebook_ir::Op::GlyphRun(run) = op {
            runs.push(run);
        }
    }
    runs
}

/// 行内字形单元格，逻辑序：(行内字节偏移, 起点 x, 终点 x)，流坐标。
fn line_cells(ir: &LayoutResult, line: &LineBox, runs: &[&GlyphRun]) -> Vec<(u32, f32, f32)> {
    let mut cells = Vec::new();
    for run in runs {
        if run.glyphs.start < line.glyphs.start || run.glyphs.end() > line.glyphs.end() {
            continue;
        }
        let mut pen = run.origin.x;
        for glyph in ir.display.glyph_slice(run.glyphs) {
            let dx = glyph.dx.max(0.0);
            cells.push((glyph.ch, pen, pen + dx));
            pen += glyph.dx;
        }
    }
    cells
}

/// 把 [lo, hi) 字节区间换成若干段连续 x 区间。相邻字形无缝时合并。
fn selected_segments(
    cells: &[(u32, f32, f32)],
    line: &LineBox,
    lo: u32,
    hi: u32,
) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::new();
    let mut prev_end: Option<f32> = None;
    for (i, &(ch, x0, x1)) in cells.iter().enumerate() {
        let next = cells.get(i + 1).map(|c| c.0).unwrap_or(line.src.len);
        // 字形覆盖 [ch, next)；与选区有非空交集才计入（整字形选中）。
        if ch >= hi || next <= lo || next <= ch {
            prev_end = None;
            continue;
        }
        let joined = match (out.last_mut(), prev_end) {
            (Some(_), Some(pe)) => (pe - x0).abs() <= 0.01,
            _ => false,
        };
        if joined {
            if let Some(last) = out.last_mut() {
                last.1 = x1;
            }
        } else {
            out.push((x0, x1));
        }
        prev_end = Some(x1);
    }
    out
}
