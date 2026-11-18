use alloc::vec::Vec;

use ebook_ir::{FontId, FontSpec};

pub use crate::shape::ShapeGlyph;
use crate::Hyphenator;

pub trait TextMeasurer {
    fn advance(&self, ch: char, font: FontId, size: f32) -> f32;
    fn ascent(&self, font: FontId, size: f32) -> f32;
    fn descent(&self, font: FontId, size: f32) -> f32;

    /// 把一段同字体文本整形成字形序列，返回总前进宽度。
    ///
    /// 默认实现逐字 advance（即升级前的行为），所以没有字体的 `StubMeasurer`
    /// 仍然能排版。字形按逻辑顺序追加到 `out`，`ch` 是簇首字符的字节偏移。
    fn shape(&self, text: &str, font: FontId, size: f32, out: &mut Vec<ShapeGlyph>) -> f32 {
        crate::shape::shape_per_char(self, text, font, size, out)
    }

    fn x_height(&self, _font: FontId, size: f32) -> f32 {
        size * 0.5
    }

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

    /// 断字词典。默认没有：`hyphens: auto` 只有在宿主注入词典后才生效。
    ///
    /// 放在 `TextMeasurer` 上而不是新增 `LayoutEnv` 字段，是为了不改变
    /// `LayoutEnv` 的形状（宿主侧只实现一次度量接口就够）。
    fn hyphenator(&self) -> Option<&dyn Hyphenator> {
        None
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
