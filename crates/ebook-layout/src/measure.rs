use ebook_ir::{FontId, FontSpec};

pub trait TextMeasurer {
    fn advance(&self, ch: char, font: FontId, size: f32) -> f32;
    fn ascent(&self, font: FontId, size: f32) -> f32;
    fn descent(&self, font: FontId, size: f32) -> f32;

    fn covers(&self, _ch: char, _font: FontId) -> bool {
        true
    }

    fn glyph_id(&self, ch: char, _font: FontId) -> u16 {
        let v = ch as u32;
        if v <= 0xFFFF {
            v as u16
        } else {
            0xFFFD
        }
    }

    fn select(&self, spec: &FontSpec) -> FontId {
        spec.font
    }

    fn select_char(&self, spec: &FontSpec, _ch: char) -> FontId {
        self.select(spec)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StubMeasurer;

impl TextMeasurer for StubMeasurer {
    fn advance(&self, ch: char, _font: FontId, size: f32) -> f32 {
        if ch == ' ' {
            size * 0.28
        } else if is_wide(ch) {
            size
        } else {
            size * 0.52
        }
    }

    fn ascent(&self, _font: FontId, size: f32) -> f32 {
        size * 0.80
    }

    fn descent(&self, _font: FontId, size: f32) -> f32 {
        size * 0.20
    }
}

pub fn is_wide(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x3FFFD
    )
}
