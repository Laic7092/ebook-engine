//! 覆盖率与亚像素定位的参考对比。
//!
//! 参考值是 16x16 点采样的真实面积近似：把轮廓在设备空间展平，逐像素取
//! 256 个样本，数非零环绕。`ab_glyph_rasterizer` 做的是解析面积积分，二者
//! 的差就是“积分 vs 采样”的差，`ss=16` 时采样本身的量化误差上限是 1/255。
//!
//! `src/lib.rs` 的 `SRGB_DECODE` 由此脚本生成（sRGB 逆 EOTF，放大到 16 位）：
//!
//! ```text
//! def lin(c):
//!     v = c / 255
//!     return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4
//! [round(65535 * lin(i)) for i in range(256)]
//! ```

use std::fs;
use std::path::PathBuf;

use ebook_font::FontBook;
use ebook_ir::{FontId, Resources};
use ebook_layout::TextMeasurer;
use read_fonts::{
    model::{
        glyph::{
            outline::{DrawMemory, DrawOptions, PathContourStart, PathSink},
            GlyphSource, GlyphSourceOrder,
        },
        Font,
    },
    types::GlyphId,
};

const SS: usize = 16;

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/font/test.ttf");
    fs::read(path).expect("test font")
}

fn book() -> FontBook {
    let mut book = FontBook::new();
    book.add(fixture()).expect("font");
    book
}

struct Contours {
    scale: f32,
    contours: Vec<Vec<[f32; 2]>>,
    current: Vec<[f32; 2]>,
}

impl Contours {
    fn new(scale: f32) -> Self {
        Self {
            scale,
            contours: Vec::new(),
            current: Vec::new(),
        }
    }

    fn push(&mut self, x: f32, y: f32) {
        self.current.push([x * self.scale, -y * self.scale]);
    }

    fn finish(mut self) -> Vec<Vec<[f32; 2]>> {
        if self.current.len() >= 3 {
            self.contours.push(self.current);
        }
        self.contours
    }
}

impl PathSink for Contours {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.current.len() >= 3 {
            self.contours.push(std::mem::take(&mut self.current));
        }
        self.current.clear();
        self.push(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let [x0, y0] = *self.current.last().expect("move_to first");
        let x0 = x0 / self.scale;
        let y0 = -y0 / self.scale;
        for i in 1..=16 {
            let t = i as f32 / 16.0;
            let u = 1.0 - t;
            let px = u * u * x0 + 2.0 * u * t * x1 + t * t * x;
            let py = u * u * y0 + 2.0 * u * t * y1 + t * t * y;
            self.push(px, py);
        }
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let [x0, y0] = *self.current.last().expect("move_to first");
        let x0 = x0 / self.scale;
        let y0 = -y0 / self.scale;
        for i in 1..=16 {
            let t = i as f32 / 16.0;
            let u = 1.0 - t;
            let px = u * u * u * x0 + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t * x;
            let py = u * u * u * y0 + 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t * y;
            self.push(px, py);
        }
    }

    fn close(&mut self) {}
}

struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// 逐像素 16x16 点采样的覆盖率。位图左上角是 `(left, top)`，与 GlyphBitmap
/// 的语义一致：`left` 是相对笔位的像素偏移，`top` 是相对基线的像素偏移。
fn supersample(contours: &[Vec<[f32; 2]>], w: usize, h: usize, left: i32, top: i32) -> Vec<u8> {
    let mut edges = Vec::new();
    for contour in contours {
        for i in 0..contour.len() {
            let a = contour[i];
            let b = contour[(i + 1) % contour.len()];
            if a[1] == b[1] {
                continue;
            }
            edges.push(Edge {
                x0: a[0],
                y0: a[1],
                x1: b[0],
                y1: b[1],
            });
        }
    }
    let step = 1.0 / SS as f32;
    let total = (SS * SS) as u32;
    let mut out = vec![0u8; w * h];
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    for py in 0..h {
        let mut hits = vec![0u32; w];
        for sy in 0..SS {
            let y = top as f32 + py as f32 + (sy as f32 + 0.5) * step;
            crossings.clear();
            for edge in &edges {
                let dir = if edge.y0 <= y && y < edge.y1 {
                    1
                } else if edge.y1 <= y && y < edge.y0 {
                    -1
                } else {
                    0
                };
                if dir == 0 {
                    continue;
                }
                let t = (y - edge.y0) / (edge.y1 - edge.y0);
                crossings.push((edge.x0 + t * (edge.x1 - edge.x0), dir));
            }
            if crossings.is_empty() {
                continue;
            }
            crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut winding = 0i32;
            let mut next = 0usize;
            for (px, hit) in hits.iter_mut().enumerate() {
                for sx in 0..SS {
                    let x = left as f32 + px as f32 + (sx as f32 + 0.5) * step;
                    while next < crossings.len() && crossings[next].0 <= x {
                        winding += crossings[next].1;
                        next += 1;
                    }
                    if winding != 0 {
                        *hit += 1;
                    }
                }
            }
        }
        for (px, hit) in hits.iter().enumerate() {
            out[py * w + px] = ((hit * 255 + total / 2) / total) as u8;
        }
    }
    out
}

fn face(bytes: &[u8]) -> Font {
    Font::new(bytes.to_vec(), 0).expect("face")
}

const OUTLINE_SOURCES: GlyphSourceOrder =
    GlyphSourceOrder::new(&[GlyphSource::Glyf, GlyphSource::Cff]);

/// 均值/最大值上界来自实测（`fixtures/font/test.ttf`，8 个字形 × 12/24/48px，
/// 11909 个位图像素）：平均差 0.33/255，最大差 8/255。剩下的差主要是
/// 16x16 点采样本身的量化误差与极窄斜边；解析面积积分对真实面积应更小。
/// 上界留约 30% 余量，防止展平细节变化误报，但仍能抓住覆盖率整体偏灰/偏黑。
const MEAN_BOUND: f64 = 0.45;
const MAX_BOUND: u8 = 10;

#[test]
fn coverage_matches_supersampling_reference() {
    let bytes = fixture();
    let parsed = face(&bytes);
    let upem = parsed.units_per_em() as f32;
    let book = book();
    let id = FontId(0);

    let mut sum_err = 0f64;
    let mut count = 0usize;
    let mut max_err = 0u8;
    let mut cases = 0usize;
    for ch in ['A', 'a', 'O', 'o', 'Q', 'g', '&', '%'] {
        if !book.covers(ch, id) {
            continue;
        }
        let gid = book.glyph_id(ch, id);
        for px in [12.0f32, 24.0, 48.0] {
            let Some(bitmap) = book.rasterize(id, gid, px) else {
                continue;
            };
            let scale = px / upem;
            let mut builder = Contours::new(scale);
            let outlines = parsed.outline_glyphs_with(OUTLINE_SOURCES);
            let Some(glyph) = outlines.get(GlyphId::new(gid as u32)) else {
                continue;
            };
            let options = DrawOptions::new().with_contour_start(PathContourStart::ScanForward);
            if glyph
                .draw(&options, DrawMemory::Scratch, &mut builder)
                .is_err()
            {
                continue;
            }
            let contours = builder.finish();
            let w = bitmap.w as usize;
            let h = bitmap.h as usize;
            let want = supersample(&contours, w, h, bitmap.left as i32, bitmap.top as i32);
            for (got, want) in bitmap.alpha.iter().zip(want.iter()).take(w * h) {
                let err = got.abs_diff(*want);
                sum_err += err as f64;
                max_err = max_err.max(err);
            }
            count += w * h;
            cases += 1;
        }
    }
    assert!(cases >= 12, "not enough glyph/size cases: {cases}");
    let mean = sum_err / count as f64;
    println!("coverage cases={cases} px={count} mean={mean:.3}/255 max={max_err}/255");
    assert!(mean < MEAN_BOUND, "mean coverage error {mean:.3}/255");
    assert!(max_err <= MAX_BOUND, "max coverage error {max_err}/255");
}

fn centroid(bitmap: &ebook_ir::GlyphBitmap, vertical: bool) -> f64 {
    let w = bitmap.w as usize;
    let h = bitmap.h as usize;
    let mut sum = 0f64;
    let mut weighted = 0f64;
    for y in 0..h {
        for x in 0..w {
            let a = bitmap.alpha[y * w + x] as f64;
            sum += a;
            if vertical {
                weighted += a * (y as f64 + 0.5 + bitmap.top as f64);
            } else {
                weighted += a * (x as f64 + 0.5 + bitmap.left as f64);
            }
        }
    }
    weighted / sum
}

#[test]
fn subpixel_offset_shifts_centroid_by_the_same_fraction() {
    let mut book = book();
    let id = FontId(0);
    let gid = book.glyph_id('A', id);
    let base = book
        .glyph_alpha_at(id, gid, 48.0, 0.0, 0.0)
        .expect("bitmap");
    let half = book
        .glyph_alpha_at(id, gid, 48.0, 0.5, 0.0)
        .expect("bitmap");
    let dx = centroid(&half, false) - centroid(&base, false);
    assert!((dx - 0.5).abs() < 0.02, "dx centroid moved {dx}");

    // 位图边界会随相位翻转，但覆盖率的重心必须连续。
    for phase in [0.1f32, 0.25, 0.5, 0.75, 0.9] {
        let shifted = book
            .glyph_alpha_at(id, gid, 48.0, phase, 0.0)
            .expect("bitmap");
        let delta = centroid(&shifted, false) - centroid(&base, false);
        assert!(
            (delta - phase as f64).abs() < 0.02,
            "phase {phase} moved {delta}"
        );
    }

    let down = book
        .glyph_alpha_at(id, gid, 48.0, 0.0, 0.5)
        .expect("bitmap");
    let dy = centroid(&down, true) - centroid(&base, true);
    assert!((dy - 0.5).abs() < 0.02, "dy centroid moved {dy}");
}

#[test]
fn repeated_rasterisation_is_byte_identical() {
    let book = book();
    let id = FontId(0);
    let gid = book.glyph_id('Q', id);
    let first = book.rasterize_at(id, gid, 37.5, 0.3, 0.7).expect("bitmap");
    let second = book.rasterize_at(id, gid, 37.5, 0.3, 0.7).expect("bitmap");
    assert_eq!(first, second);
}
