//! CLI 专用 SVG 后端。
//!
//! 为什么不用 `ebook-render::SvgSurface`：那个后端只画图片占位（参考实现不关心
//! 像素），而命令行需要能直接看的真实图片。`Surface` / `paint()` 是 IR 层的公共
//! 接口，这里只实现“把 op 画成 SVG”，坐标仍然由 `paint()` 平移到页内。
//!
//! 图片以 `data:` URI 内联，不依赖任何外部路径：`--svg` 的输出单文件可移动。

use core::fmt::Write;

use ebook_ir::{
    BorderBox, Fit, Glyph, GlyphRun, ImageId, RectF, Resources, Rgba8, SizeF, Surface, Unresolved,
};

pub struct CliSvg {
    out: String,
    size: SizeF,
    next_id: u32,
    /// `(image id, mime, data URI)`。按 id 升序，查找是线性扫描（一页图很少）。
    images: Vec<(ImageId, String, String)>,
}

impl CliSvg {
    pub fn new(size: SizeF, images: Vec<(ImageId, String, String)>) -> Self {
        Self {
            out: String::new(),
            size,
            next_id: 0,
            images,
        }
    }

    pub fn finish(self) -> String {
        let mut out = String::new();
        let _ = write!(
            out,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">",
            w = self.size.w,
            h = self.size.h
        );
        out.push_str(&self.out);
        out.push_str("</svg>");
        out
    }

    fn take_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
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

impl Surface for CliSvg {
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

    fn image(&mut self, id: ImageId, rect: RectF, fit: Fit, _res: &mut dyn Resources) {
        if rect.is_empty() {
            return;
        }
        if let Some((_, mime, uri)) = self
            .images
            .iter()
            .find(|(image, _, _)| *image == id)
            .cloned()
        {
            let aspect = match fit {
                Fit::Contain => "xMidYMid meet",
                Fit::Cover => "xMidYMid slice",
                Fit::Fill => "none",
            };
            let _ = write!(
                self.out,
                "<image x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" preserveAspectRatio=\"{aspect}\" xlink:href=\"data:{mime};base64,{uri}\"/>",
                rect.x, rect.y, rect.w, rect.h
            );
            return;
        }
        // 没有原始字节（JPEG/GIF/WebP 或读取失败）：画占位，不白屏。
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

    fn link(&mut self, rect: RectF, href: &str) {
        if rect.is_empty() {
            return;
        }
        self.out.push_str("<a xlink:href=\"");
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

fn push_escaped(out: &mut String, text: &str) {
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

/// 标准 base64（无换行）。图片不大，直接按 3 字节一组编码。
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let bits = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(bits >> 18) as usize & 63] as char);
        out.push(ALPHABET[(bits >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(bits >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[bits as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}
