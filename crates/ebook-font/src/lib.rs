use ebook_ir::{FontId, GlyphBitmap};
use ebook_layout::TextMeasurer;

#[derive(Debug, Default)]
pub struct FontBook {
    faces: Vec<Vec<u8>>,
}

impl FontBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, bytes: Vec<u8>) -> Option<FontId> {
        let id = FontId(self.faces.len() as u16);
        self.faces.push(bytes);
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
    fn advance(&self, _ch: char, _font: FontId, _size: f32) -> f32 {
        0.0
    }

    fn ascent(&self, _font: FontId, size: f32) -> f32 {
        size * 0.8
    }

    fn descent(&self, _font: FontId, size: f32) -> f32 {
        size * 0.2
    }

    fn covers(&self, _ch: char, _font: FontId) -> bool {
        false
    }

    fn glyph_id(&self, _ch: char, _font: FontId) -> u16 {
        0
    }
}
