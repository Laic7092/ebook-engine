use core::fmt::Write;

use ebook_ir::{
    BorderBox, Fit, Glyph, GlyphRun, ImageId, LayoutResult, RectF, RenderError, Renderer,
    Resources, Rgba8, SizeF, Surface, Unresolved,
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
    next_id: u32,
}

impl SvgSurface {
    pub fn new(size: SizeF) -> Self {
        Self {
            out: alloc::string::String::new(),
            size,
            next_id: 0,
        }
    }

    fn take_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
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

    fn image(&mut self, id: ImageId, rect: RectF, _fit: Fit, res: &mut dyn Resources) {
        if rect.is_empty() {
            return;
        }
        if res.image(id).is_some() {
            let _ = write!(
                self.out,
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"#dddddd\" stroke=\"#999999\"/>",
                rect.x, rect.y, rect.w, rect.h
            );
            return;
        }
        let pattern = self.take_id();
        let _ = write!(
            self.out,
            "<defs><pattern id=\"placeholder{pattern}\" width=\"8\" height=\"8\" patternUnits=\"userSpaceOnUse\"><path d=\"M0 8L8 0\" stroke=\"#d0d0d0\" stroke-width=\"1\" fill=\"none\"/></pattern></defs>"
        );
        let _ = write!(
            self.out,
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"url(#placeholder{pattern})\" stroke=\"#999999\" stroke-dasharray=\"4 4\"/>",
            rect.x, rect.y, rect.w, rect.h
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

    /// 链接热区：输出 `<a href>` 包一个透明矩形，可见像素不变，但能在浏览器里点到。
    fn link(&mut self, rect: RectF, href: &str) {
        if rect.is_empty() {
            return;
        }
        self.out.push_str("<a href=\"");
        push_escaped(&mut self.out, href);
        let _ = write!(
            self.out,
            "\"><rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"none\" pointer-events=\"all\"/></a>",
            rect.x, rect.y, rect.w, rect.h
        );
    }

    fn push_clip(&mut self, rect: RectF) {
        let id = self.take_id();
        let _ = write!(
            self.out,
            "<defs><clipPath id=\"clip{id}\"><rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\"/></clipPath></defs><g clip-path=\"url(#clip{id})\">",
            rect.x, rect.y, rect.w, rect.h
        );
    }

    fn pop_clip(&mut self) {
        self.out.push_str("</g>");
    }
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
