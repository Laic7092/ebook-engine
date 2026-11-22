use alloc::vec::Vec;

use crate::doc::ImageId;
use crate::font::FontId;
use crate::geom::{PointF, RectF, Rgba8, SizeF};
use crate::layout::{BorderBox, Fit, Glyph, GlyphRun, LayoutResult, Unresolved};

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBitmap {
    pub w: u16,
    pub h: u16,
    pub left: i16,
    pub top: i16,
    pub alpha: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageData {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

pub trait Resources {
    fn glyph_alpha(&mut self, font: FontId, gid: u16, px: f32) -> Option<GlyphBitmap>;
    fn glyph_alpha_at(
        &mut self,
        font: FontId,
        gid: u16,
        px: f32,
        dx: f32,
        dy: f32,
    ) -> Option<GlyphBitmap> {
        let _ = (dx, dy);
        self.glyph_alpha(font, gid, px)
    }

    fn image(&mut self, id: ImageId) -> Option<ImageData>;
}

pub struct NoResources;

impl Resources for NoResources {
    fn glyph_alpha(&mut self, _font: FontId, _gid: u16, _px: f32) -> Option<GlyphBitmap> {
        None
    }

    fn image(&mut self, _id: ImageId) -> Option<ImageData> {
        None
    }
}

pub trait Surface {
    fn size(&self) -> SizeF;

    /// 设备像素比例：`size()` 是设备像素，`1.0` 表示 `size()` 就是排版坐标。
    ///
    /// `paint_window` 用它把 surface 高换算回排版流坐标，从而只画窗口内的 op。
    /// 装饰型 surface（先乘比例再转给后端）应该覆盖它；光栅后端本身是 1:1。
    fn scale(&self) -> f32 {
        1.0
    }

    fn glyph_run(&mut self, run: &GlyphRun, glyphs: &[Glyph], res: &mut dyn Resources);

    /// 与 [`Surface::glyph_run`] 相同，但先把 `run.origin` 平移 `shift`。
    ///
    /// 默认实现会 clone `GlyphRun`（含 `text`），只是兼容性兑底；热路径上的
    /// 后端应该覆盖它，直接在绘制时加上平移，避免每帧为每个 run 复制字符串。
    fn glyph_run_shifted(
        &mut self,
        run: &GlyphRun,
        glyphs: &[Glyph],
        shift: PointF,
        res: &mut dyn Resources,
    ) {
        let mut moved = run.clone();
        moved.origin = run.origin.shifted(shift.x, shift.y);
        self.glyph_run(&moved, glyphs, res);
    }

    fn rect(&mut self, rect: RectF, color: Rgba8);
    fn border(&mut self, border: &BorderBox);
    fn image(&mut self, id: ImageId, rect: RectF, fit: Fit, res: &mut dyn Resources);
    fn unresolved(&mut self, unresolved: &Unresolved);
    /// 链接热区。默认什么都不画：光栅后端保持透明，SVG 后端输出 `<a>`。
    fn link(&mut self, _rect: RectF, _href: &str) {}
    fn push_clip(&mut self, _rect: RectF) {}
    fn pop_clip(&mut self) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    NoSuchPage(u32),
    EmptyDocument,
}

pub trait Renderer {
    fn render(
        &mut self,
        ir: &LayoutResult,
        page: u32,
        surface: &mut dyn Surface,
        res: &mut dyn Resources,
    ) -> Result<(), RenderError>;
}
