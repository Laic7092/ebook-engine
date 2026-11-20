//! 面向阅读器的查询外观：命中、行盒、页、进度、锚点、链接。
//!
//! 这一层只读 `LayoutResult`，不改排版结果，也不引入任何 I/O。越界与空文档的
//! 语义对齐 layx 的 "Out-of-range and empty results"：`Option` / 空切片 / 夹取后
//! 的值，绝不 panic，也不返回半个对象。
//!
//! 坐标分两种，别混：
//! - **页内坐标**：`Page` 左上角为原点，阅读器鼠标/触控给的坐标。
//! - **排版流坐标**：IR 里 op 与行盒的坐标。换算见 `Reader::to_flow`。

use alloc::string::String;
use alloc::vec::Vec;

use ebook_ir::{Anchor, LayoutResult, LineBox, Location, Op, PointF, RectF, StyledDoc};

use crate::select::{selection_rects, selection_text, Selection, SelectionRect};

pub struct Reader<'a> {
    ir: &'a LayoutResult,
}

impl<'a> Reader<'a> {
    pub fn new(ir: &'a LayoutResult) -> Self {
        Self { ir }
    }

    pub fn ir(&self) -> &'a LayoutResult {
        self.ir
    }

    /// 页内坐标 → 排版流坐标。与 `paint` 的平移互逆。
    fn to_flow(&self, page: u32, p: PointF) -> Option<PointF> {
        let page = self.ir.page(page)?;
        Some(PointF::new(
            p.x + page.origin.x - page.inset.left,
            p.y + page.origin.y - page.inset.top,
        ))
    }

    /// 命中行内最近的原文位置。
    ///
    /// 先走 `LayoutResult::hit_test`（严格落在行盒内才命中）；落空时退回本页最近的
    /// 行，把点夹到该行矩形内再算一次。这样点行间空白、页边距也能得到确定结果，
    /// 和 layx "HitTest 返回最近条目" 的行为一致；页不存在或页内没有行才返回 `None`。
    pub fn hit(&self, page: u32, p: PointF) -> Option<Location> {
        if let Some(loc) = self.ir.hit_test(page, p) {
            return Some(loc);
        }
        let page_ref = self.ir.page(page)?;
        let flow = self.to_flow(page, p)?;
        let line = nearest_line(self.ir.page_lines(page), flow)?;
        // 夹点留 0.01 的余量并取行中线：页内坐标与流坐标来回换算有一次浮点
        // 加减，正好落在矩形边界上会被舍入到边界外。
        let low = line.rect.x + 0.01;
        let high = (line.rect.right() - 0.01).max(low);
        let x = flow.x.clamp(low, high);
        let y = line.rect.y + line.rect.h * 0.5;
        let point = PointF::new(
            x - page_ref.origin.x + page_ref.inset.left,
            y - page_ref.origin.y + page_ref.inset.top,
        );
        self.ir.hit_test(page, point)
    }

    /// 全局行号 → 行盒。`Location.page` 与行号不一致时以行号为准。
    pub fn line_of(&self, loc: Location) -> Option<&LineBox> {
        self.ir.line(loc.line)
    }

    /// 行号 → 页号。
    pub fn page_of_line(&self, line: u32) -> Option<u32> {
        self.ir.page_of_line(line).map(|p| p.index)
    }

    /// 全书进度，0..=1。
    ///
    /// 用行号 + 行内字节占比，不用像素：同一份内容在任意页面尺寸下进度相同，
    /// 页边界处连续（上一行末尾的 1.0 与下一行的 0.0 相接），且单调不减。
    /// 越界位置夹到最近合法位置，所以 `progress(Location::default())` 在空文档外
    /// 也是 0。
    pub fn progress(&self, loc: Location) -> f32 {
        let total = self.ir.runs.lines.len();
        if total == 0 {
            return 0.0;
        }
        let clamped = self.clamp(loc);
        let line = clamped.line as f32;
        let len = self.ir.line(clamped.line).map(|l| l.src.len).unwrap_or(0);
        let frac = if len == 0 {
            0.0
        } else {
            (clamped.byte.min(len) as f32) / (len as f32)
        };
        ((line + frac) / total as f32).clamp(0.0, 1.0)
    }

    /// 页首的进度。页不存在或空文档返回 0。
    pub fn page_progress(&self, page: u32) -> f32 {
        let Some(page_ref) = self.ir.page(page) else {
            return 0.0;
        };
        self.progress(Location {
            page: page_ref.index,
            line: page_ref.lines.start,
            byte: 0,
        })
    }

    /// 页内坐标 → 原文锚点。
    pub fn anchor_at(&self, page: u32, p: PointF) -> Option<Anchor> {
        self.ir.anchor_at(page, p)
    }

    /// 原文锚点 → 页/行/字节位置。
    pub fn locate(&self, anchor: &Anchor) -> Option<Location> {
        self.ir.location_of(anchor)
    }

    /// 命中 `Op::Link` 时返回 href，否则 `None`。
    ///
    /// 返回的 `&str` 属于 IR（`'a`），不绑定 `Reader` 本身——宿主可以把
    /// href 直接交出去，不必先拷成 `String`。
    pub fn link_at(&self, page: u32, p: PointF) -> Option<&'a str> {
        let page_ref = self.ir.page(page)?;
        let flow = self.to_flow(page, p)?;
        for op in self.ir.display.op_slice(page_ref.ops) {
            if let Op::Link { rect, href } = op {
                if rect.contains(flow) {
                    return Some(href.as_str());
                }
            }
        }
        None
    }

    /// 越界的位置夹到最近合法位置：行号夹到 [0, 行数)，字节夹到该行 `src` 长度。
    ///
    /// `page` 由行号反查，保证返回的三元组自洽；空文档返回 `Location::default()`。
    pub fn clamp(&self, loc: Location) -> Location {
        let total = self.ir.runs.lines.len() as u32;
        if total == 0 {
            return Location::default();
        }
        let line = loc.line.min(total - 1);
        let Some(line_box) = self.ir.line(line) else {
            return Location::default();
        };
        let page = self.page_of_line(line).unwrap_or(loc.page);
        Location {
            page,
            line,
            byte: loc.byte.min(line_box.src.len),
        }
    }

    /// 两个位置之间的选区。`end` 可以在 `start` 之前，取矩形时会先 normalize。
    pub fn selection(&self, a: Location, b: Location) -> Selection {
        Selection::new(a, b)
    }

    /// 选区分页矩形（页内坐标）。空选区返回空 vec。
    pub fn selection_rects(&self, sel: &Selection) -> Vec<SelectionRect> {
        selection_rects(self, sel)
    }

    /// 选区原文。需要原文时由宿主传入 `StyledDoc`；空选区返回空串。
    pub fn selection_text(&self, doc: &StyledDoc, sel: &Selection) -> String {
        selection_text(self, doc, sel)
    }
}

fn nearest_line(lines: &[LineBox], p: PointF) -> Option<&LineBox> {
    lines
        .iter()
        .min_by(|a, b| distance(a.rect, p).total_cmp(&distance(b.rect, p)))
}

fn distance(r: RectF, p: PointF) -> f32 {
    let dx = if p.x < r.x {
        r.x - p.x
    } else if p.x > r.right() {
        p.x - r.right()
    } else {
        0.0
    };
    let dy = if p.y < r.y {
        r.y - p.y
    } else if p.y > r.bottom() {
        p.y - r.bottom()
    } else {
        0.0
    };
    dx * dx + dy * dy
}
