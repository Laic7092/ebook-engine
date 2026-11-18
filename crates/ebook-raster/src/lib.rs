#![cfg_attr(not(test), no_std)]
extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;

use ebook_ir::{
    BorderBox, Fit, Glyph, GlyphBitmap, GlyphRun, ImageId, LayoutResult, LinkTarget, RenderError,
    Renderer, Resources, Rgba8, SizeF, Surface, Unresolved,
};

#[derive(Clone, Debug)]
pub struct RgbaSurface {
    width: u32,
    height: u32,
    pixels: Vec<Rgba8>,
}

impl RgbaSurface {
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            pixels: vec![Rgba8::WHITE; len],
        }
    }

    pub fn from_size(size: SizeF) -> Self {
        Self::new(float_to_u32(size.w), float_to_u32(size.h))
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[Rgba8] {
        &self.pixels
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgba8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = y as usize * self.width as usize + x as usize;
        self.pixels.get(index).copied()
    }

    pub fn clear(&mut self, color: Rgba8) {
        self.pixels.fill(color);
    }

    pub fn render_page(
        &mut self,
        ir: &LayoutResult,
        page: u32,
        res: &mut dyn Resources,
    ) -> Result<(), RenderError> {
        self.clear(Rgba8::WHITE);
        ebook_render::paint(ir, page, self, res)
    }

    pub fn to_ppm(&self) -> Vec<u8> {
        let mut header = String::new();
        let _ = write!(header, "P6\n{} {}\n255\n", self.width, self.height);
        let mut out = Vec::with_capacity(header.len() + self.pixels.len() * 3);
        out.extend_from_slice(header.as_bytes());
        for pixel in &self.pixels {
            out.push(pixel.r);
            out.push(pixel.g);
            out.push(pixel.b);
        }
        out
    }

    fn write(&mut self, x: i64, y: i64, color: Rgba8) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let index = y as usize * self.width as usize + x as usize;
        if let Some(dst) = self.pixels.get_mut(index) {
            *dst = blend(*dst, color);
        }
    }

    fn fill(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, color: Rgba8) {
        let x0 = x0.max(0);
        let y0 = y0.max(0);
        let x1 = x1.min(self.width as i64);
        let y1 = y1.min(self.height as i64);
        for y in y0..y1 {
            for x in x0..x1 {
                self.write(x, y, color);
            }
        }
    }

    fn fill_rect(&mut self, rect: ebook_ir::RectF, color: Rgba8) {
        if rect.is_empty() {
            return;
        }
        self.fill(
            floor_i(rect.x),
            floor_i(rect.y),
            ceil_i(rect.right()),
            ceil_i(rect.bottom()),
            color,
        );
    }

    fn draw_glyph(&mut self, color: Rgba8, bitmap: &GlyphBitmap, pen_x: f32, baseline: f32) {
        let w = bitmap.w as usize;
        let h = bitmap.h as usize;
        let len = w.saturating_mul(h);
        if w == 0 || h == 0 || bitmap.alpha.len() < len {
            return;
        }
        let x0 = floor_i(pen_x + bitmap.left as f32);
        let y0 = floor_i(baseline + bitmap.top as f32);
        for by in 0..h {
            for bx in 0..w {
                let coverage = bitmap.alpha[by * w + bx];
                let alpha = mul_alpha(color.a, coverage);
                if alpha == 0 {
                    continue;
                }
                self.write(
                    x0 + bx as i64,
                    y0 + by as i64,
                    Rgba8::new(color.r, color.g, color.b, alpha),
                );
            }
        }
    }

    fn draw_dashes(&mut self, u: &Unresolved) {
        let rect = u.rect;
        if rect.is_empty() {
            return;
        }
        let x0 = floor_i(rect.x);
        let y0 = floor_i(rect.y);
        let x1 = ceil_i(rect.right()).saturating_sub(1);
        let y1 = ceil_i(rect.bottom()).saturating_sub(1);
        if x1 < x0 || y1 < y0 {
            return;
        }
        let color = Rgba8::new(0xcc, 0x00, 0x00, 255);
        let left = x0.max(0);
        let right = x1.min(self.width as i64 - 1);
        let top = y0.max(0);
        let bottom = y1.min(self.height as i64 - 1);
        if top <= bottom {
            for x in left..=right {
                if dash_on(x - x0) {
                    self.write(x, y0, color);
                    self.write(x, y1, color);
                }
            }
        }
        if left <= right {
            for y in top..=bottom {
                if dash_on(y - y0) {
                    self.write(x0, y, color);
                    self.write(x1, y, color);
                }
            }
        }
    }

    fn draw_image(&mut self, image: &ebook_ir::ImageData, rect: ebook_ir::RectF, fit: Fit) {
        if image.w == 0 || image.h == 0 || rect.is_empty() {
            return;
        }
        let need = (image.w as usize)
            .saturating_mul(image.h as usize)
            .saturating_mul(4);
        if image.rgba.len() < need {
            return;
        }
        let iw = image.w as f32;
        let ih = image.h as f32;
        let (dw, dh, ox, oy) = match fit {
            Fit::Fill => (rect.w, rect.h, rect.x, rect.y),
            Fit::Contain => {
                let scale = (rect.w / iw).min(rect.h / ih);
                let dw = iw * scale;
                let dh = ih * scale;
                (
                    dw,
                    dh,
                    rect.x + (rect.w - dw) * 0.5,
                    rect.y + (rect.h - dh) * 0.5,
                )
            }
            Fit::Cover => {
                let scale = (rect.w / iw).max(rect.h / ih);
                let dw = iw * scale;
                let dh = ih * scale;
                (
                    dw,
                    dh,
                    rect.x + (rect.w - dw) * 0.5,
                    rect.y + (rect.h - dh) * 0.5,
                )
            }
        };
        if !(dw > 0.0) || !(dh > 0.0) {
            return;
        }
        let rx0 = floor_i(rect.x);
        let ry0 = floor_i(rect.y);
        let rx1 = ceil_i(rect.right());
        let ry1 = ceil_i(rect.bottom());
        let dx0 = floor_i(ox).max(rx0);
        let dy0 = floor_i(oy).max(ry0);
        let dx1 = ceil_i(ox + dw).min(rx1);
        let dy1 = ceil_i(oy + dh).min(ry1);
        for dy in dy0..dy1 {
            for dx in dx0..dx1 {
                let u = ((dx as f32 + 0.5 - ox) / dw).clamp(0.0, 0.999_999_94);
                let v = ((dy as f32 + 0.5 - oy) / dh).clamp(0.0, 0.999_999_94);
                let sx = (u * iw) as u32;
                let sy = (v * ih) as u32;
                let index = (sy as usize * image.w as usize + sx as usize) * 4;
                let alpha = image.rgba[index + 3];
                if alpha == 0 {
                    continue;
                }
                self.write(
                    dx,
                    dy,
                    Rgba8::new(
                        image.rgba[index],
                        image.rgba[index + 1],
                        image.rgba[index + 2],
                        alpha,
                    ),
                );
            }
        }
    }
}

impl Surface for RgbaSurface {
    fn size(&self) -> SizeF {
        SizeF::new(self.width as f32, self.height as f32)
    }

    fn glyph_run(&mut self, run: &GlyphRun, glyphs: &[Glyph], res: &mut dyn Resources) {
        let mut pen_x = run.origin.x;
        let baseline = run.origin.y;
        for glyph in glyphs {
            let advance = glyph.dx;
            if let Some(bitmap) = res.glyph_alpha(run.font, glyph.gid, run.size) {
                self.draw_glyph(run.color, &bitmap, pen_x, baseline + glyph.dy);
            }
            pen_x += advance;
        }
    }

    fn rect(&mut self, rect: ebook_ir::RectF, color: Rgba8) {
        self.fill_rect(rect, color);
    }

    fn border(&mut self, border: &BorderBox) {
        let r = border.rect;
        let w = border.width;
        let parts = [
            ebook_ir::RectF::new(r.x, r.y, r.w, w.top),
            ebook_ir::RectF::new(r.x, r.bottom() - w.bottom, r.w, w.bottom),
            ebook_ir::RectF::new(r.x, r.y, w.left, r.h),
            ebook_ir::RectF::new(r.right() - w.right, r.y, w.right, r.h),
        ];
        for part in parts {
            if !part.is_empty() {
                self.rect(part, border.color);
            }
        }
    }

    fn image(&mut self, id: ImageId, rect: ebook_ir::RectF, fit: Fit, res: &mut dyn Resources) {
        if let Some(image) = res.image(id) {
            self.draw_image(&image, rect, fit);
        }
    }

    fn unresolved(&mut self, unresolved: &Unresolved) {
        self.draw_dashes(unresolved);
    }

    fn link(&mut self, _rect: ebook_ir::RectF, _target: &LinkTarget) {}
}

impl Renderer for RgbaSurface {
    fn render(
        &mut self,
        ir: &LayoutResult,
        page: u32,
        surface: &mut dyn Surface,
        res: &mut dyn Resources,
    ) -> Result<(), RenderError> {
        ebook_render::paint(ir, page, surface, res)
    }
}

fn blend(dst: Rgba8, src: Rgba8) -> Rgba8 {
    let sa = src.a as u32;
    if sa == 0 {
        return dst;
    }
    if sa == 255 {
        return Rgba8::new(src.r, src.g, src.b, 255);
    }
    let da = dst.a as u32;
    let inv = 255 - sa;
    let da_scaled = (da * inv + 127) / 255;
    let out_a = sa + da_scaled;
    if out_a == 0 {
        return Rgba8::TRANSPARENT;
    }
    let r = (src.r as u32 * sa + dst.r as u32 * da_scaled + out_a / 2) / out_a;
    let g = (src.g as u32 * sa + dst.g as u32 * da_scaled + out_a / 2) / out_a;
    let b = (src.b as u32 * sa + dst.b as u32 * da_scaled + out_a / 2) / out_a;
    Rgba8::new(r as u8, g as u8, b as u8, out_a as u8)
}

fn mul_alpha(a: u8, b: u8) -> u8 {
    ((a as u32 * b as u32 + 127) / 255) as u8
}

fn dash_on(value: i64) -> bool {
    value.rem_euclid(8) < 4
}

fn floor_i(value: f32) -> i64 {
    let truncated = value as i64;
    if truncated == i64::MIN {
        return truncated;
    }
    if truncated as f32 > value {
        truncated - 1
    } else {
        truncated
    }
}

fn ceil_i(value: f32) -> i64 {
    let truncated = value as i64;
    if truncated == i64::MAX {
        return truncated;
    }
    if (truncated as f32) < value {
        truncated + 1
    } else {
        truncated
    }
}

fn float_to_u32(value: f32) -> u32 {
    if value > 0.0 {
        ceil_i(value).max(0) as u32
    } else {
        0
    }
}
