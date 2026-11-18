use alloc::vec::Vec;

use crate::doc::ImageId;
use crate::font::FontId;
use crate::geom::{RectF, Rgba8, SizeF};
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
    fn glyph_run(&mut self, run: &GlyphRun, glyphs: &[Glyph], res: &mut dyn Resources);
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
