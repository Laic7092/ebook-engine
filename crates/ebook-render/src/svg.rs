use core::fmt::Write;

use ebook_ir::{
    BorderBox, Fit, Glyph, GlyphRun, ImageId, LayoutResult, LinkTarget, RectF, RenderError,
    Renderer, Resources, Rgba8, SizeF, Surface, Unresolved,
};

pub use crate::paint::paint;

pub struct SvgRenderer;

impl Renderer for SvgRenderer {
    fn render(
        &mut self,
        ir: &LayoutResult,
        page: u32,
        surface: &mut dyn Surface,
        res: &mut dyn Resources,
    ) -> Result<(), RenderError> {
        crate::paint(ir, page, surface, res)
    }
}

pub struct SvgSurface {
    out: alloc::string::String,
    size: SizeF,
}

impl SvgSurface {
    pub fn new(size: SizeF) -> Self {
        Self {
            out: alloc::string::String::new(),
            size,
        }
    }

    pub fn finish(self) -> alloc::string::String {
        let mut out = alloc::string::String::new();
        let _ = write!(
            out,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">",
            w = self.size.w,
            h = self.size.h
        );
        out.push_str(&self.out);
        out.push_str("</svg>");
        out
    }

    fn push_color(&mut self, color: Rgba8) {
        if color.a < 255 {
            let _ = write!(
                self.out,
                " fill=\"#{:02x}{:02x}{:02x}\" fill-opacity=\"{:.3}\"",
                color.r,
                color.g,
                color.b,
                color.a as f32 / 255.0
            );
        } else {
            let _ = write!(
                self.out,
                " fill=\"#{:02x}{:02x}{:02x}\"",
                color.r, color.g, color.b
            );
        }
    }
}

impl Surface for SvgSurface {
    fn size(&self) -> SizeF {
        self.size
    }

    fn glyph_run(&mut self, run: &GlyphRun, _glyphs: &[Glyph], _res: &mut dyn Resources) {
        if run.text.is_empty() {
            return;
        }
        let _ = write!(
            self.out,
            "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"{:.2}\"",
            run.origin.x, run.origin.y, run.size
        );
        self.push_color(run.color);
        self.out.push_str(" font-family=\"sans-serif\">");
        push_escaped(&mut self.out, &run.text);
        self.out.push_str("</text>");
    }

    fn rect(&mut self, rect: RectF, color: Rgba8) {
        let _ = write!(
            self.out,
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\"",
            rect.x, rect.y, rect.w, rect.h
        );
        self.push_color(color);
        self.out.push_str("/>");
    }

    fn border(&mut self, border: &BorderBox) {
        let r = border.rect;
        let w = border.width;
        let parts = [
            RectF::new(r.x, r.y, r.w, w.top),
            RectF::new(r.x, r.bottom() - w.bottom, r.w, w.bottom),
            RectF::new(r.x, r.y, w.left, r.h),
            RectF::new(r.right() - w.right, r.y, w.right, r.h),
        ];
        for part in parts {
            if !part.is_empty() {
                self.rect(part, border.color);
            }
        }
    }

    fn image(&mut self, _id: ImageId, rect: RectF, _fit: Fit, res: &mut dyn Resources) {
        let exists = res.image(_id).is_some();
        let _ = write!(
            self.out,
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"{}\" stroke=\"#999999\"/>",
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            if exists { "#dddddd" } else { "#f5f5f5" }
        );
    }

    fn unresolved(&mut self, unresolved: &Unresolved) {
        let r = unresolved.rect;
        let _ = write!(
            self.out,
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"none\" stroke=\"#cc0000\" stroke-dasharray=\"4 4\"/>",
            r.x, r.y, r.w, r.h
        );
    }

    fn link(&mut self, _rect: RectF, _target: &LinkTarget) {}
}

fn push_escaped(out: &mut alloc::string::String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
}
