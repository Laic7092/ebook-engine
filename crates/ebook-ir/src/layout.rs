use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::doc::{Anchor, Dir, ImageId, NodeId, Span};
use crate::font::FontId;
use crate::geom::{EdgesF, PointF, RectF, Rgba8, SidesF, SizeF};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrVersion {
    pub major: u16,
    pub minor: u16,
}

pub const IR_VERSION: IrVersion = IrVersion { major: 1, minor: 2 };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotate {
    #[default]
    None,
    Cw90,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    #[default]
    Contain,
    Cover,
    Fill,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnresolvedReason {
    #[default]
    ImagePending,
    FontMissing,
    SizeFromContent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unresolved {
    pub node: NodeId,
    pub rect: RectF,
    pub reason: UnresolvedReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BorderBox {
    pub rect: RectF,
    pub width: SidesF,
    pub color: Rgba8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Glyph {
    pub gid: u16,
    pub dx: f32,
    pub dy: f32,
    /// 该字形在所属行 `LineBox.src` 文本内的字节偏移（簇首字符）。
    /// 选区与命中测试靠它把 x 坐标换回原文位置。
    pub ch: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlyphRun {
    pub font: FontId,
    pub size: f32,
    pub color: Rgba8,
    pub origin: PointF,
    pub glyphs: Span,
    pub text: String,
    pub dir: Dir,
    pub rotate: Rotate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    GlyphRun(GlyphRun),
    Rect {
        rect: RectF,
        color: Rgba8,
    },
    Border(BorderBox),
    Image {
        id: ImageId,
        rect: RectF,
        fit: Fit,
    },
    Unresolved(Unresolved),
    Clip {
        rect: RectF,
        ops: Vec<Op>,
    },
    /// 链接热区。`rect` 是排版流坐标（与行盒矩形同形），`href` 是原文的属性值。
    Link {
        rect: RectF,
        href: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DisplayList {
    pub ops: Vec<Op>,
    pub glyphs: Vec<Glyph>,
}

impl DisplayList {
    pub fn op_slice(&self, span: Span) -> &[Op] {
        &self.ops[span.as_range()]
    }

    pub fn glyph_slice(&self, span: Span) -> &[Glyph] {
        &self.glyphs[span.as_range()]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub index: u32,
    pub origin: PointF,
    pub size: SizeF,
    pub inset: EdgesF,
    pub ops: Span,
    pub lines: Span,
    pub anchor: Option<Anchor>,
}

impl Page {
    pub fn box_size(&self) -> SizeF {
        SizeF::new(
            self.size.w + self.inset.horizontal(),
            self.size.h + self.inset.vertical(),
        )
    }

    pub fn content_origin(&self) -> PointF {
        PointF::new(self.inset.left, self.inset.top)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineBox {
    pub rect: RectF,
    pub baseline: f32,
    pub dir: Dir,
    pub src: Span,
    pub anchor: Option<Anchor>,
    /// 本行字形在 `DisplayList.glyphs` 里的区间。
    pub glyphs: Span,
    /// 本行属于哪个文本节点（链接定位与选区回写用）。
    pub node: NodeId,
}

/// 原文位置：页号 + 行号 + 该行 `LineBox.src` 内的字节偏移。
///
/// 行号是 `RunMap.lines` 的全局下标，不是页内下标。跨行选择 = 两个 `Location`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub page: u32,
    pub line: u32,
    pub byte: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueCode {
    #[default]
    Other,
    MissingFont,
    MissingGlyph,
    MissingImage,
    SizeFromContent,
    UnsupportedCss,
    LimitExceeded,
}

/// 排版过程中记录的一条可恢复问题。本轮只记录，不做拦截。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub node: NodeId,
    pub code: IssueCode,
    pub detail: Option<String>,
}

/// 单次布局的硬上限。字段为 0 表示使用 `Default` 里的值（与 layx 的 `Limits` 对齐）。
/// 本轮只定义类型，拦截由 wave-2 的缓存/限额任务接入。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_nodes: u32,
    pub max_depth: u16,
    pub max_text_bytes: u32,
    pub max_pages: u32,
    pub max_lines: u32,
    pub max_image_pixels: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_nodes: 1_048_576,
            max_depth: 256,
            max_text_bytes: 20_000_000,
            max_pages: 100_000,
            max_lines: 2_000_000,
            max_image_pixels: 67_108_864,
        }
    }
}

impl Limits {
    /// 把零值字段替换成默认值。拦截逻辑接入时统一走这里，避免每个调用点各写一份。
    pub fn resolved(self) -> Self {
        let d = Self::default();
        Self {
            max_nodes: if self.max_nodes == 0 {
                d.max_nodes
            } else {
                self.max_nodes
            },
            max_depth: if self.max_depth == 0 {
                d.max_depth
            } else {
                self.max_depth
            },
            max_text_bytes: if self.max_text_bytes == 0 {
                d.max_text_bytes
            } else {
                self.max_text_bytes
            },
            max_pages: if self.max_pages == 0 {
                d.max_pages
            } else {
                self.max_pages
            },
            max_lines: if self.max_lines == 0 {
                d.max_lines
            } else {
                self.max_lines
            },
            max_image_pixels: if self.max_image_pixels == 0 {
                d.max_image_pixels
            } else {
                self.max_image_pixels
            },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunMap {
    pub lines: Vec<LineBox>,
}

impl RunMap {
    /// 按**排版流坐标**命中行盒，不是页内坐标。
    ///
    /// 流坐标 = 页内坐标 + `Page.origin` - `Page.inset`（与渲染层的平移互逆）。
    /// 手里只有页内坐标时请用 `LayoutResult::hit_test`，它负责这次换算。
    pub fn line_at(&self, p: PointF) -> Option<&LineBox> {
        self.lines.iter().find(|l| l.rect.contains(p))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DirtyBlock {
    pub node: NodeId,
    pub rect: RectF,
    pub reason: UnresolvedReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutResult {
    pub version: IrVersion,
    pub spine: u32,
    pub pages: Vec<Page>,
    pub display: DisplayList,
    pub runs: RunMap,
    pub dirty: Vec<DirtyBlock>,
    /// 排版过程中发现的可恢复问题（缺字、缺图、...）。本轮只收集，不拦截。
    pub issues: Vec<Issue>,
}

impl LayoutResult {
    pub fn page(&self, index: u32) -> Option<&Page> {
        self.pages.get(index as usize)
    }

    pub fn page_of_line(&self, line: u32) -> Option<&Page> {
        self.pages
            .iter()
            .find(|p| line >= p.lines.start && line < p.lines.end())
    }

    /// 页内坐标 → 排版流坐标。逆着 `paint` 的平移：
    /// `页内 = 流 - origin + inset`，所以 `流 = 页内 + origin - inset`。
    fn flow_point(&self, page: u32, p: PointF) -> Option<PointF> {
        let page = self.page(page)?;
        Some(PointF::new(
            p.x + page.origin.x - page.inset.left,
            p.y + page.origin.y - page.inset.top,
        ))
    }

    /// 页内坐标命中行盒，再在行内找最近的字形，返回原文位置。
    ///
    /// 字形区间用 `Glyph.dx` 累计：点落在某个字形的前进宽度内就算命中该字形，
    /// 否则取左右最近的边界（左边界优先，保证结果确定）。
    pub fn hit_test(&self, page: u32, p: PointF) -> Option<Location> {
        let page_ref = self.page(page)?;
        let flow = self.flow_point(page, p)?;
        let start = page_ref.lines.start as usize;
        let end = page_ref.lines.end() as usize;
        let lines = self.runs.lines.get(start..end)?;
        let (offset, line) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.rect.contains(flow))?;
        let index = start + offset;
        let byte = self.nearest_byte(page_ref, line, flow.x);
        Some(Location {
            page: page_ref.index,
            line: index as u32,
            byte,
        })
    }

    /// 行内离 `x` 最近的字形在本行 `src` 文本里的字节偏移。
    fn nearest_byte(&self, page: &Page, line: &LineBox, x: f32) -> u32 {
        let mut best: Option<(f32, u32)> = None;
        for op in self.display.op_slice(page.ops) {
            let Op::GlyphRun(run) = op else { continue };
            if run.glyphs.start < line.glyphs.start || run.glyphs.end() > line.glyphs.end() {
                continue;
            }
            let mut pen = run.origin.x;
            for glyph in self.display.glyph_slice(run.glyphs) {
                let distance = if x < pen {
                    pen - x
                } else if x > pen + glyph.dx {
                    x - pen - glyph.dx
                } else {
                    0.0
                };
                let better = best.is_none_or(|(current, _)| distance < current);
                if better {
                    best = Some((distance, glyph.ch));
                }
                pen += glyph.dx;
                if distance == 0.0 {
                    break;
                }
            }
        }
        best.map(|(_, ch)| ch).unwrap_or(0)
    }

    /// 原文锚点 → 页/行/字节位置。
    ///
    /// 行盒的 `anchor` 记的是该行首字符的锚点（offset 相对文本节点起点），
    /// 所以同一 `spine + path` 下用 offset 落在哪一行的 `src` 长度内来判断。
    pub fn location_of(&self, a: &Anchor) -> Option<Location> {
        let mut last: Option<(u32, u32)> = None;
        for (index, line) in self.runs.lines.iter().enumerate() {
            let Some(anchor) = line.anchor.as_ref() else {
                continue;
            };
            if anchor.spine != a.spine || anchor.path != a.path {
                continue;
            }
            let byte = a.offset.saturating_sub(anchor.offset);
            if byte <= line.src.len {
                let page = self.page_of_line(index as u32)?;
                return Some(Location {
                    page: page.index,
                    line: index as u32,
                    byte,
                });
            }
            last = Some((index as u32, line.src.len));
        }
        let (line, byte) = last?;
        let page = self.page_of_line(line)?;
        Some(Location {
            page: page.index,
            line,
            byte,
        })
    }

    /// 页内坐标 → 原文锚点。字节偏移会累加进锚点，所以能和 `location_of` 往返。
    ///
    /// 跨多个文本节点的行只保留首节点路径；这是本轮的最小实现，多节点选区留给后续任务。
    pub fn anchor_at(&self, page: u32, p: PointF) -> Option<Anchor> {
        let location = self.hit_test(page, p)?;
        let line = self.runs.lines.get(location.line as usize)?;
        let anchor = line.anchor.as_ref()?;
        Some(anchor.with_offset(anchor.offset.saturating_add(location.byte)))
    }
}
