use ab_glyph_rasterizer::{point, Point, Rasterizer};
use ebook_ir::{FontId, GlyphBitmap, ImageData, ImageId, Resources};
use ebook_layout::TextMeasurer;
use ttf_parser::Face;

#[derive(Debug, Default)]
struct Metrics {
    bytes: Vec<u8>,
    units_per_em: f32,
    ascender: f32,
    descender: f32,
    advances: Vec<u16>,
    cmap: Vec<(u32, u16)>,
}

impl Metrics {
    fn parse(bytes: Vec<u8>) -> Option<Self> {
        let face = Face::parse(&bytes, 0).ok()?;
        let units_per_em = face.units_per_em() as f32;
        if units_per_em <= 0.0 {
            return None;
        }
        let ascender = face.ascender() as f32;
        let descender = face.descender() as f32;
        let advances = (0..face.number_of_glyphs())
            .map(|gid| face.glyph_hor_advance(ttf_parser::GlyphId(gid)).unwrap_or(0))
            .collect();
        let mut cmap = Vec::new();
        if let Some(table) = face.tables().cmap {
            for subtable in table.subtables {
                if !subtable.is_unicode() {
                    continue;
                }
                subtable.codepoints(|code| {
                    if let Some(gid) = subtable.glyph_index(code) {
                        cmap.push((code, gid.0));
                    }
                });
            }
        }
        cmap.sort_unstable_by_key(|entry| entry.0);
        cmap.dedup_by_key(|entry| entry.0);
        Some(Self { bytes, units_per_em, ascender, descender, advances, cmap })
    }

    fn glyph(&self, ch: char) -> u16 {
        match self.cmap.binary_search_by_key(&(ch as u32), |entry| entry.0) {
            Ok(index) => self.cmap[index].1,
            Err(_) => 0,
        }
    }

    fn advance(&self, ch: char) -> f32 {
        let gid = self.glyph(ch);
        if gid == 0 {
            return 0.0;
        }
        self.advances.get(gid as usize).copied().unwrap_or(0) as f32 / self.units_per_em
    }
}

#[derive(Debug, Default)]
pub struct FontBook {
    faces: Vec<Metrics>,
}

impl FontBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, bytes: Vec<u8>) -> Option<FontId> {
        let metrics = Metrics::parse(bytes)?;
        let id = FontId(self.faces.len() as u16);
        self.faces.push(metrics);
        Some(id)
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    fn face(&self, id: FontId) -> Option<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.faces.get(id.0 as usize)?.bytes, 0).ok()
    }

    pub fn rasterize(&self, id: FontId, gid: u16, px: f32) -> Option<GlyphBitmap> {
        if !px.is_finite() || px <= 0.0 {
            return None;
        }
        let face = self.face(id)?;
        let scale = px / face.units_per_em() as f32;
        let mut bounds = Bounds;
        let rect = face.outline_glyph(ttf_parser::GlyphId(gid), &mut bounds)?;
        let left_edge = (rect.x_min as f32 * scale).floor();
        let right_edge = (rect.x_max as f32 * scale).ceil();
        let top_edge = (rect.y_max as f32 * scale).ceil();
        let bottom_edge = (rect.y_min as f32 * scale).floor();
        let width = right_edge - left_edge;
        let height = top_edge - bottom_edge;
        if !(width >= 1.0) || !(height >= 1.0) {
            return None;
        }
        if width > u16::MAX as f32 || height > u16::MAX as f32 {
            return None;
        }
        let left = left_edge as i32;
        let top = -(top_edge as i32);
        if left < i16::MIN as i32
            || left > i16::MAX as i32
            || top < i16::MIN as i32
            || top > i16::MAX as i32
        {
            return None;
        }
        let w = width as usize;
        let h = height as usize;
        let mut rasterizer = Rasterizer::new(w, h);
        let mut outline = RasterOutline {
            rasterizer: &mut rasterizer,
            scale,
            left: left_edge,
            top: top_edge,
            start: point(0.0, 0.0),
            current: point(0.0, 0.0),
        };
        face.outline_glyph(ttf_parser::GlyphId(gid), &mut outline)?;
        drop(outline);
        let mut alpha = vec![0u8; w * h];
        rasterizer.for_each_pixel(|index, coverage| {
            alpha[index] = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
        });
        Some(GlyphBitmap {
            w: w as u16,
            h: h as u16,
            left: left as i16,
            top: top as i16,
            alpha,
        })
    }
}

struct Bounds;

impl ttf_parser::OutlineBuilder for Bounds {
    fn move_to(&mut self, _x: f32, _y: f32) {}

    fn line_to(&mut self, _x: f32, _y: f32) {}

    fn quad_to(&mut self, _x1: f32, _y1: f32, _x: f32, _y: f32) {}

    fn curve_to(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32, _x: f32, _y: f32) {}

    fn close(&mut self) {}
}

struct RasterOutline<'a> {
    rasterizer: &'a mut Rasterizer,
    scale: f32,
    left: f32,
    top: f32,
    start: Point,
    current: Point,
}

impl RasterOutline<'_> {
    fn map(&self, x: f32, y: f32) -> Point {
        point(x * self.scale - self.left, self.top - y * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for RasterOutline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.start = p;
        self.current = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.rasterizer.draw_line(self.current, p);
        self.current = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let control = self.map(x1, y1);
        let p = self.map(x, y);
        self.rasterizer.draw_quad(self.current, control, p);
        self.current = p;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let control1 = self.map(x1, y1);
        let control2 = self.map(x2, y2);
        let p = self.map(x, y);
        self.rasterizer
            .draw_cubic(self.current, control1, control2, p);
        self.current = p;
    }

    fn close(&mut self) {
        self.rasterizer.draw_line(self.current, self.start);
        self.current = self.start;
    }
}

impl TextMeasurer for FontBook {
    fn advance(&self, ch: char, font: FontId, size: f32) -> f32 {
        match self.faces.get(font.0 as usize) {
            Some(metrics) => metrics.advance(ch) * size,
            None => 0.0,
        }
    }

    fn ascent(&self, font: FontId, size: f32) -> f32 {
        match self.faces.get(font.0 as usize) {
            Some(metrics) => metrics.ascender / metrics.units_per_em * size,
            None => size * 0.8,
        }
    }

    fn descent(&self, font: FontId, size: f32) -> f32 {
        match self.faces.get(font.0 as usize) {
            Some(metrics) => metrics.descender.abs() / metrics.units_per_em * size,
            None => size * 0.2,
        }
    }

    fn covers(&self, ch: char, font: FontId) -> bool {
        self.faces
            .get(font.0 as usize)
            .map(|metrics| metrics.glyph(ch) != 0)
            .unwrap_or(false)
    }

    fn glyph_id(&self, ch: char, font: FontId) -> u16 {
        self.faces
            .get(font.0 as usize)
            .map(|metrics| metrics.glyph(ch))
            .unwrap_or(0)
    }
}

impl Resources for FontBook {
    fn glyph_alpha(&mut self, font: FontId, gid: u16, px: f32) -> Option<GlyphBitmap> {
        self.rasterize(font, gid, px)
    }

    fn image(&mut self, _id: ImageId) -> Option<ImageData> {
        None
    }
}
