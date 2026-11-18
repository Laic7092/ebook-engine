use ebook_ir::{FontId, GlyphBitmap};
use ebook_layout::TextMeasurer;
use ttf_parser::Face;

#[derive(Debug, Default)]
struct Metrics {
    units_per_em: f32,
    ascender: f32,
    descender: f32,
    advances: Vec<u16>,
    cmap: Vec<(u32, u16)>,
}

impl Metrics {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let face = Face::parse(bytes, 0).ok()?;
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
        Some(Self { units_per_em, ascender, descender, advances, cmap })
    }

    fn glyph(&self, ch: char) -> u16 {
        match self.cmap.binary_search_by_key(&(ch as u32), |entry| entry.0) {
            Ok(index) => self.cmap[index].1,
            Err(_) => 0,
        }
    }

    fn advance(&self, ch: char) -> f32 {
        let gid = self.glyph(ch) as usize;
        self.advances.get(gid).copied().unwrap_or(0) as f32 / self.units_per_em
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
        let metrics = Metrics::parse(&bytes)?;
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

    pub fn rasterize(&self, _id: FontId, _gid: u16, _px: f32) -> Option<GlyphBitmap> {
        None
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
            Some(metrics) => -metrics.descender / metrics.units_per_em * size,
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
