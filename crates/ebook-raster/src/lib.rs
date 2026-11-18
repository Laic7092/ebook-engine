#![cfg_attr(not(test), no_std)]
extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;

use ebook_ir::{
    BorderBox, Fit, Glyph, GlyphBitmap, GlyphRun, ImageData, ImageId, LayoutResult, RectF,
    RenderError, Renderer, Resources, Rgba8, SizeF, Surface, Unresolved,
};

/// sRGB 编码值到线性光值的查表，单位 1/65535。
///
/// 由 `round(65535 * linear(c / 255))` 生成，linear 是 sRGB 的逆 EOTF。
/// 生成脚本在 `crates/ebook-raster/tests/coverage.rs` 的注释里给出。整数常量，
/// 不依赖 libm，跨平台逐字节一致。
const SRGB_DECODE: [u16; 256] = [
    0, 20, 40, 60, 80, 99, 119, 139, 159, 179, 199, 219, 241, 264, 288, 313, 340, 367, 396, 427,
    458, 491, 526, 562, 599, 637, 677, 718, 761, 805, 851, 898, 947, 997, 1048, 1101, 1156, 1212,
    1270, 1330, 1391, 1453, 1517, 1583, 1651, 1720, 1790, 1863, 1937, 2013, 2090, 2170, 2250, 2333,
    2418, 2504, 2592, 2681, 2773, 2866, 2961, 3058, 3157, 3258, 3360, 3464, 3570, 3678, 3788, 3900,
    4014, 4129, 4247, 4366, 4488, 4611, 4736, 4864, 4993, 5124, 5257, 5392, 5530, 5669, 5810, 5953,
    6099, 6246, 6395, 6547, 6700, 6856, 7014, 7174, 7335, 7500, 7666, 7834, 8004, 8177, 8352, 8528,
    8708, 8889, 9072, 9258, 9445, 9635, 9828, 10022, 10219, 10417, 10619, 10822, 11028, 11235,
    11446, 11658, 11873, 12090, 12309, 12530, 12754, 12980, 13209, 13440, 13673, 13909, 14146,
    14387, 14629, 14874, 15122, 15371, 15623, 15878, 16135, 16394, 16656, 16920, 17187, 17456,
    17727, 18001, 18277, 18556, 18837, 19121, 19407, 19696, 19987, 20281, 20577, 20876, 21177,
    21481, 21787, 22096, 22407, 22721, 23038, 23357, 23678, 24002, 24329, 24658, 24990, 25325,
    25662, 26001, 26344, 26688, 27036, 27386, 27739, 28094, 28452, 28813, 29176, 29542, 29911,
    30282, 30656, 31033, 31412, 31794, 32179, 32567, 32957, 33350, 33745, 34143, 34544, 34948,
    35355, 35764, 36176, 36591, 37008, 37429, 37852, 38278, 38706, 39138, 39572, 40009, 40449,
    40891, 41337, 41785, 42236, 42690, 43147, 43606, 44069, 44534, 45002, 45473, 45947, 46423,
    46903, 47385, 47871, 48359, 48850, 49344, 49841, 50341, 50844, 51349, 51858, 52369, 52884,
    53401, 53921, 54445, 54971, 55500, 56032, 56567, 57105, 57646, 58190, 58737, 59287, 59840,
    60396, 60955, 61517, 62082, 62650, 63221, 63795, 64372, 64952, 65535,
];

/// 线性光值（12 位索引）回到 sRGB 编码的查表。
///
/// 由 [`SRGB_DECODE`] 单调反推，4096 项，误差不超过一个编码。`const fn` 在
/// 编译期生成，不占运行时，也没有全局可变状态。
const SRGB_ENCODE: [u8; 4096] = build_encode_table();

const fn build_encode_table() -> [u8; 4096] {
    let mut out = [0u8; 4096];
    let mut code = 0usize;
    let mut index = 0usize;
    while code < 256 {
        // 下一个编码在线性空间里的起点，也是当前编码覆盖到的桶。
        let next = if code + 1 < 256 {
            (SRGB_DECODE[code + 1] as usize) / 16
        } else {
            4096
        };
        while index < next && index < 4096 {
            out[index] = code as u8;
            index += 1;
        }
        code += 1;
    }
    while index < 4096 {
        out[index] = 255;
        index += 1;
    }
    out
}

/// `65536 / a` 的定点倒数，用来把除法换成乘加。
const RECIP: [u32; 256] = build_recip_table();

const fn build_recip_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 1usize;
    while i < 256 {
        table[i] = (65536 + (i as u32) / 2) / (i as u32);
        i += 1;
    }
    table
}

/// 墨色与伽马修正。
///
/// 默认 `None`，混合、覆盖率都保持 sRGB 空间的既有行为，像素基线不变。
/// 打开后有两件事，都走查表、整数运算、跨平台逐字节一致：
///
/// - **墨色**：覆盖率对比度随墨色变深而增强，并在边缘做轻微生长。曲线取自
///   `layx/raster` 的 `Tone`（DirectWrite 的 grayscale 对比度 + CoreText 的
///   font smoothing）：几何光栅器只画轮廓，系统打印机还会补一层字重。
/// - **伽马**：混合在线性光空间做，而不是在 sRGB 编码值上直接插值。半透明
///   边缘的亮度因此线性，而不是偏暗。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tone {
    lut: [u8; 256],
    linear: bool,
}

impl Tone {
    /// 按墨色建对比度表，不做字干生长。
    pub fn ink(ink: Rgba8) -> Self {
        Self::ink_with_grow(ink, 0.0)
    }

    /// 按墨色建对比度表，`grow` 是边缘覆盖率增长（0..=1，0.5 处最强）。
    pub fn ink_with_grow(ink: Rgba8, grow: f32) -> Self {
        let luma = (0.30 * ink.r as f32 + 0.59 * ink.g as f32 + 0.11 * ink.b as f32) / 255.0;
        // 深墨吃满对比度，浅墨（比如夜纸上的白字）几乎不加。
        let contrast = 0.5 * (3.0 - 4.0 * luma).clamp(0.0, 1.0);
        let grow = grow.clamp(0.0, 1.0);
        let mut lut = [0u8; 256];
        let mut i = 0usize;
        while i < 256 {
            let a = i as f32 / 255.0;
            let mut v = a * (contrast + 1.0) / (a * contrast + 1.0);
            // 生长在覆盖率为一半处最强，在纸面与墨心处归零：像膨胀了的边缘，
            // 但不挪动字形。
            v += grow * (1.0 - (2.0 * v - 1.0).abs());
            let scaled = (v.min(1.0) * 255.0 + 0.5) as i32;
            lut[i] = scaled.clamp(0, 255) as u8;
            i += 1;
        }
        Self { lut, linear: false }
    }

    /// 打开线性光混合。只有不透明纸面（`a == 255`）走线性路径，
    /// 其余仍走直通道合成，避免把预乘误差带进半透明表面。
    pub fn linear(mut self) -> Self {
        self.linear = true;
        self
    }

    /// 修正一档覆盖率。纸面 0 与墨心 255 是固定点。
    pub fn coverage(&self, a: u8) -> u8 {
        self.lut[a as usize]
    }

    pub fn is_linear(&self) -> bool {
        self.linear
    }
}

#[derive(Clone, Debug)]
pub struct RgbaSurface {
    width: u32,
    height: u32,
    pixels: Vec<Rgba8>,
    /// 裁剪栈，每一项已经是整条栈的交集，只存像素中心的整数范围。
    clips: Vec<Clip>,
    tone: Option<Tone>,
}

/// 一个裁剪矩形的像素范围，`x1`/`y1` 是开区间。
///
/// 语义与逐像素的“中心点落在矩形内”一致：像素 `x` 的中心是 `x + 0.5`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Clip {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl Clip {
    fn from_rect(rect: RectF) -> Self {
        if rect.is_empty() {
            return Self {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0,
            };
        }
        Self {
            x0: ceil_i(rect.x - 0.5),
            y0: ceil_i(rect.y - 0.5),
            x1: ceil_i(rect.right() - 0.5),
            y1: ceil_i(rect.bottom() - 0.5),
        }
    }

    fn intersect(self, other: Self) -> Self {
        Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }
}

impl RgbaSurface {
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            pixels: vec![Rgba8::WHITE; len],
            clips: Vec::new(),
            tone: None,
        }
    }

    pub fn from_page(page: &ebook_ir::Page) -> Self {
        Self::from_size(page.box_size())
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

    /// 设置墨色/伽马修正。`None`（默认）保持既有 sRGB 混合行为。
    pub fn set_tone(&mut self, tone: Option<Tone>) {
        self.tone = tone;
    }

    pub fn tone(&self) -> Option<Tone> {
        self.tone
    }

    pub fn with_tone(mut self, tone: Option<Tone>) -> Self {
        self.tone = tone;
        self
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

    /// 当前生效的裁剪范围，已经与画布求过交。
    fn clip(&self) -> Clip {
        self.clips.last().copied().unwrap_or(Clip {
            x0: 0,
            y0: 0,
            x1: self.width as i64,
            y1: self.height as i64,
        })
    }

    fn put(&mut self, x: i64, y: i64, color: Rgba8) {
        let index = y as usize * self.width as usize + x as usize;
        let dst = self.pixels[index];
        let out = self.blend(dst, color);
        self.pixels[index] = out;
    }

    fn write(&mut self, x: i64, y: i64, color: Rgba8) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let clip = self.clip();
        if x < clip.x0 || x >= clip.x1 || y < clip.y0 || y >= clip.y1 {
            return;
        }
        self.put(x, y, color);
    }

    /// 填充整数矩形。先与画布、裁剪求交，再整行写：不透明色走 `fill`，
    /// 逐像素混合里没有除法；半透明才逐像素走混合。
    fn fill(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, color: Rgba8) {
        if color.a == 0 {
            return;
        }
        let clip = self.clip();
        let x0 = x0.max(0).max(clip.x0);
        let y0 = y0.max(0).max(clip.y0);
        let x1 = x1.min(self.width as i64).min(clip.x1);
        let y1 = y1.min(self.height as i64).min(clip.y1);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let width = self.width as usize;
        let start = x0 as usize;
        let end = x1 as usize;
        for y in y0..y1 {
            let row = y as usize * width;
            if color.a == 255 {
                self.pixels[row + start..row + end].fill(color);
            } else {
                for x in start..end {
                    let dst = self.pixels[row + x];
                    let out = self.blend(dst, color);
                    self.pixels[row + x] = out;
                }
            }
        }
    }

    fn fill_rect(&mut self, rect: RectF, color: Rgba8) {
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

    fn draw_glyph(&mut self, color: Rgba8, bitmap: &GlyphBitmap, x0: i64, y0: i64) {
        let w = bitmap.w as usize;
        let h = bitmap.h as usize;
        if w == 0 || h == 0 || bitmap.alpha.len() < w.saturating_mul(h) {
            return;
        }
        let gx = x0 + bitmap.left as i64;
        let gy = y0 + bitmap.top as i64;
        let clip = self.clip();
        let mut bx0 = 0i64;
        let mut bx1 = w as i64;
        let mut by0 = 0i64;
        let mut by1 = h as i64;
        bx0 = bx0.max(-gx).max(clip.x0 - gx);
        bx1 = bx1.min(self.width as i64 - gx).min(clip.x1 - gx);
        by0 = by0.max(-gy).max(clip.y0 - gy);
        by1 = by1.min(self.height as i64 - gy).min(clip.y1 - gy);
        if bx1 <= bx0 || by1 <= by0 {
            return;
        }
        let tone = self.tone;
        let width = self.width as usize;
        for by in by0..by1 {
            let row = by as usize * w;
            let y = gy + by;
            for bx in bx0..bx1 {
                let mut coverage = bitmap.alpha[row + bx as usize];
                if let Some(tone) = tone {
                    coverage = tone.coverage(coverage);
                }
                if coverage == 0 {
                    continue;
                }
                let alpha = mul_alpha(color.a, coverage);
                if alpha == 0 {
                    continue;
                }
                let x = gx + bx;
                let index = y as usize * width + x as usize;
                if alpha == 255 {
                    self.pixels[index] = Rgba8::new(color.r, color.g, color.b, 255);
                } else {
                    let dst = self.pixels[index];
                    let out = self.blend(dst, Rgba8::new(color.r, color.g, color.b, alpha));
                    self.pixels[index] = out;
                }
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

    fn draw_placeholder(&mut self, rect: RectF) {
        if rect.is_empty() {
            return;
        }
        let x0 = ceil_i(rect.x);
        let y0 = ceil_i(rect.y);
        let x1 = floor_i(rect.right()).saturating_sub(1);
        let y1 = floor_i(rect.bottom()).saturating_sub(1);
        if x1 < x0 || y1 < y0 {
            return;
        }
        let left = x0.max(0);
        let top = y0.max(0);
        let right = x1.min(self.width as i64 - 1);
        let bottom = y1.min(self.height as i64 - 1);
        let hatch = Rgba8::new(0xd0, 0xd0, 0xd0, 255);
        let border = Rgba8::new(0x99, 0x99, 0x99, 255);
        if left <= right && top <= bottom {
            for y in top..=bottom {
                for x in left..=right {
                    if (x + y).rem_euclid(8) == 0 {
                        self.write(x, y, hatch);
                    }
                }
            }
        }
        if top <= bottom {
            for x in left..=right {
                if dash_on(x - x0) {
                    self.write(x, y0, border);
                    self.write(x, y1, border);
                }
            }
        }
        if left <= right {
            for y in top..=bottom {
                if dash_on(y - y0) {
                    self.write(x0, y, border);
                    self.write(x1, y, border);
                }
            }
        }
    }

    fn draw_image(&mut self, image: &ImageData, rect: RectF, fit: Fit) {
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
        // 三种 Fit 都是同一个采样映射，只是目标矩形的缩放与偏移不同。
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
        if !(dw > 0.0 && dh > 0.0) {
            return;
        }
        let clip = self.clip();
        // 目标像素必须同时落在采样矩形、op 矩形、画布与裁剪里。
        let x0 = floor_i(ox).max(floor_i(rect.x)).max(0).max(clip.x0);
        let y0 = floor_i(oy).max(floor_i(rect.y)).max(0).max(clip.y0);
        let x1 = ceil_i(ox + dw)
            .min(ceil_i(rect.right()))
            .min(self.width as i64)
            .min(clip.x1);
        let y1 = ceil_i(oy + dh)
            .min(ceil_i(rect.bottom()))
            .min(self.height as i64)
            .min(clip.y1);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let width = self.width as usize;
        let scale_x = 256.0 * iw / dw;
        let scale_y = 256.0 * ih / dh;
        for dy in y0..y1 {
            let fy = (dy as f32 + 0.5 - oy) * scale_y - 128.0;
            let fy = fixed_round(fy);
            for dx in x0..x1 {
                let fx = (dx as f32 + 0.5 - ox) * scale_x - 128.0;
                let fx = fixed_round(fx);
                let Some(color) = sample_bilinear(image, fx, fy) else {
                    continue;
                };
                let index = dy as usize * width + dx as usize;
                let dst = self.pixels[index];
                let out = self.blend(dst, color);
                self.pixels[index] = out;
            }
        }
    }

    /// 源覆盖到目标的混合。`src.a` 已经乘过覆盖率，两条绘制路径
    /// （字形与图片）都只调这一处。
    fn blend(&self, dst: Rgba8, src: Rgba8) -> Rgba8 {
        let sa = src.a as u32;
        if sa == 0 {
            return dst;
        }
        if sa == 255 {
            return Rgba8::new(src.r, src.g, src.b, 255);
        }
        let da = dst.a as u32;
        if da == 255 {
            if let Some(tone) = self.tone {
                if tone.linear {
                    return blend_linear(dst, src, sa);
                }
            }
            let inv = 255 - sa;
            let r = div255(src.r as u32 * sa + dst.r as u32 * inv);
            let g = div255(src.g as u32 * sa + dst.g as u32 * inv);
            let b = div255(src.b as u32 * sa + dst.b as u32 * inv);
            return Rgba8::new(r as u8, g as u8, b as u8, 255);
        }
        // 半透明目标：预乘后合成，再解预乘。纸面不透明时上面的快速路径
        // 已经返回，这里只在离屏半透明表面上走到。
        let inv = 255 - sa;
        let da_scaled = div255(da * inv);
        let out_a = sa + da_scaled;
        if out_a == 0 {
            return Rgba8::TRANSPARENT;
        }
        let unpremultiply =
            |num: u32| -> u8 { ((num as u64 * RECIP[out_a as usize] as u64 + 32768) >> 16) as u8 };
        let r = unpremultiply(src.r as u32 * sa + dst.r as u32 * da_scaled);
        let g = unpremultiply(src.g as u32 * sa + dst.g as u32 * da_scaled);
        let b = unpremultiply(src.b as u32 * sa + dst.b as u32 * da_scaled);
        Rgba8::new(r, g, b, out_a as u8)
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
            let pen_y = baseline + glyph.dy;
            // 整数部分是落点，小数部分是相位；两者一起交给资源层，
            // 位图边界与覆盖率都由相位决定，抖动时不会跳一个像素。
            let origin_x = floor_i(pen_x);
            let origin_y = floor_i(pen_y);
            let dx = pen_x - origin_x as f32;
            let dy = pen_y - origin_y as f32;
            if let Some(bitmap) = res.glyph_alpha_at(run.font, glyph.gid, run.size, dx, dy) {
                self.draw_glyph(run.color, &bitmap, origin_x, origin_y);
            }
            pen_x += advance;
        }
    }

    fn rect(&mut self, rect: RectF, color: Rgba8) {
        self.fill_rect(rect, color);
    }

    fn border(&mut self, border: &BorderBox) {
        let r = border.rect;
        let w = border.width;
        // 四条边互不重叠，半透明边框在拐角不会被混合两次。
        let top = RectF::new(r.x, r.y, r.w, w.top);
        let bottom = RectF::new(r.x, r.bottom() - w.bottom, r.w, w.bottom);
        let left = RectF::new(r.x, r.y + w.top, w.left, r.h - w.top - w.bottom);
        let right = RectF::new(
            r.right() - w.right,
            r.y + w.top,
            w.right,
            r.h - w.top - w.bottom,
        );
        for part in [top, bottom, left, right] {
            if !part.is_empty() {
                self.rect(part, border.color);
            }
        }
    }

    fn image(&mut self, id: ImageId, rect: RectF, fit: Fit, res: &mut dyn Resources) {
        match res.image(id) {
            Some(image) => self.draw_image(&image, rect, fit),
            None => self.draw_placeholder(rect),
        }
    }

    fn unresolved(&mut self, unresolved: &Unresolved) {
        self.draw_dashes(unresolved);
    }

    fn push_clip(&mut self, rect: RectF) {
        let next = Clip::from_rect(rect);
        let merged = match self.clips.last() {
            Some(current) => current.intersect(next),
            None => next,
        };
        self.clips.push(merged);
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }
}

impl Renderer for RgbaSurface {
    fn render(
        &mut self,
        ir: &LayoutResult,
        page: u32,
        surface: &mut dyn Surface,
        res: &mut dyn Resources,
    ) -> Result<(), RenderError> {
        let size = surface.size();
        surface.rect(RectF::new(0.0, 0.0, size.w, size.h), Rgba8::WHITE);
        ebook_render::paint(ir, page, surface, res)
    }
}

/// 双线性采样。`fx`/`fy` 是 1/256 个 texel 的定点坐标，texel 中心在整数上。
///
/// 先在预乘空间插值，再解预乘：完全透明的texel 的 RGB 不参与平均，
/// 因此透明边缘不会把背景拉黑。混合权重是整数，解预乘用倒数表，没有除法。
fn sample_bilinear(image: &ImageData, fx: i64, fy: i64) -> Option<Rgba8> {
    let iw = image.w as i64;
    let ih = image.h as i64;
    let x0 = fx.div_euclid(256);
    let y0 = fy.div_euclid(256);
    let (x0, wx) = if x0 < 0 {
        (0i64, 0i64)
    } else if x0 >= iw - 1 {
        (iw - 1, 0)
    } else {
        (x0, fx.rem_euclid(256))
    };
    let (y0, wy) = if y0 < 0 {
        (0i64, 0i64)
    } else if y0 >= ih - 1 {
        (ih - 1, 0)
    } else {
        (y0, fy.rem_euclid(256))
    };
    let x1 = (x0 + 1).min(iw - 1);
    let y1 = (y0 + 1).min(ih - 1);
    let w00 = ((256 - wx) * (256 - wy)) as u64;
    let w10 = (wx * (256 - wy)) as u64;
    let w01 = ((256 - wx) * wy) as u64;
    let w11 = (wx * wy) as u64;
    let stride = image.w as usize;
    let at = |x: i64, y: i64| -> usize { (y as usize * stride + x as usize) * 4 };
    let index = [at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1)];
    let weights = [w00, w10, w01, w11];
    let mut acc_a = 0u64;
    let mut acc = [0u64; 3];
    for (index, weight) in index.into_iter().zip(weights) {
        if weight == 0 {
            continue;
        }
        let alpha = image.rgba[index + 3] as u64;
        if alpha == 0 {
            continue;
        }
        acc_a += alpha * weight;
        for (channel, slot) in acc.iter_mut().enumerate() {
            *slot += image.rgba[index + channel] as u64 * alpha * weight;
        }
    }
    if acc_a == 0 {
        return None;
    }
    // 四舍五入到 8 位；alpha 与颜色同时除以 65536。
    let alpha = ((acc_a + 32768) >> 16) as u32;
    if alpha == 0 {
        return None;
    }
    let mut channel = [0u8; 3];
    for (slot, out) in acc.iter().zip(channel.iter_mut()) {
        let premultiplied = (*slot + 32768) >> 16;
        let straight = (premultiplied * RECIP[alpha as usize] as u64 + 32768) >> 16;
        *out = straight.min(255) as u8;
    }
    Some(Rgba8::new(channel[0], channel[1], channel[2], alpha as u8))
}

fn blend_linear(dst: Rgba8, src: Rgba8, sa: u32) -> Rgba8 {
    let inv = 255 - sa;
    let mix = |s: u8, d: u8| -> u8 {
        let linear =
            (SRGB_DECODE[s as usize] as u32 * sa + SRGB_DECODE[d as usize] as u32 * inv + 127)
                / 255;
        SRGB_ENCODE[(linear >> 4).min(4095) as usize]
    };
    Rgba8::new(mix(src.r, dst.r), mix(src.g, dst.g), mix(src.b, dst.b), 255)
}

/// 除以 255 的整数实现：`x` 最大 65025（255*255）时精确四舍五入。
fn div255(x: u32) -> u32 {
    let y = x + 128;
    (y + (y >> 8)) >> 8
}

fn mul_alpha(a: u8, b: u8) -> u8 {
    div255(a as u32 * b as u32) as u8
}

fn dash_on(value: i64) -> bool {
    value.rem_euclid(8) < 4
}

/// 浮点坐标四舍五入到 i64。
fn fixed_round(value: f32) -> i64 {
    floor_i(value + 0.5)
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
