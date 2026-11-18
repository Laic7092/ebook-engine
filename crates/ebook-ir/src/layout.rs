use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::doc::{Anchor, Dir, ImageId, NodeId, Span};
use crate::font::FontId;
use crate::geom::{PointF, RectF, Rgba8, SidesF, SizeF};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrVersion {
    pub major: u16,
    pub minor: u16,
}

pub const IR_VERSION: IrVersion = IrVersion { major: 1, minor: 0 };

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
pub enum LinkTarget {
    Spine(u32),
    External(String),
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlyphRun {
    pub font: FontId,
    pub size: f32,
    pub color: Rgba8,
    pub origin: PointF,
    pub glyphs: Span,
    pub dir: Dir,
    pub rotate: Rotate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    GlyphRun(GlyphRun),
    Rect { rect: RectF, color: Rgba8 },
    Border(BorderBox),
    Image { id: ImageId, rect: RectF, fit: Fit },
    Unresolved(Unresolved),
    Link { rect: RectF, target: LinkTarget },
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
    pub ops: Span,
    pub lines: Span,
    pub anchor: Option<Anchor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineBox {
    pub rect: RectF,
    pub baseline: f32,
    pub dir: Dir,
    pub src: Span,
    pub anchor: Option<Anchor>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunMap {
    pub lines: Vec<LineBox>,
}

impl RunMap {
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
}

impl LayoutResult {
    pub fn page(&self, index: u32) -> Option<&Page> {
        self.pages.get(index as usize)
    }

    pub fn page_of_line(&self, line: u32) -> Option<&Page> {
        self.pages.iter().find(|p| line >= p.lines.start && line < p.lines.end())
    }

    pub fn anchor_at(&self, p: PointF) -> Option<&Anchor> {
        self.runs.line_at(p).and_then(|l| l.anchor.as_ref())
    }
}
