use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

pub const FONT_STACK_MAX: usize = 8;
pub const FONT_FAMILY_MAX: usize = 32;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct FontId(pub u16);

impl FontId {
    pub const NONE: FontId = FontId(u16::MAX);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontFamily {
    len: u8,
    bytes: [u8; FONT_FAMILY_MAX],
}

impl Default for FontFamily {
    fn default() -> Self {
        Self {
            len: 0,
            bytes: [0; FONT_FAMILY_MAX],
        }
    }
}

impl FontFamily {
    pub fn new(name: &str) -> Self {
        let mut family = Self::default();
        let mut len = 0usize;
        for ch in name.trim().chars() {
            let width = ch.len_utf8();
            if len + width > FONT_FAMILY_MAX {
                break;
            }
            ch.encode_utf8(&mut family.bytes[len..]);
            len += width;
        }
        family.len = len as u8;
        family
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontFamilies {
    len: u8,
    items: [FontFamily; FONT_STACK_MAX],
}

impl Default for FontFamilies {
    fn default() -> Self {
        Self {
            len: 0,
            items: [FontFamily::default(); FONT_STACK_MAX],
        }
    }
}

impl FontFamilies {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_slice(names: &[&str]) -> Self {
        let mut families = Self::new();
        for name in names {
            families.push(name);
        }
        families
    }

    pub fn push(&mut self, name: &str) -> bool {
        if self.len as usize >= FONT_STACK_MAX {
            return false;
        }
        let family = FontFamily::new(name);
        if family.is_empty() {
            return false;
        }
        self.items[self.len as usize] = family;
        self.len += 1;
        true
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<&FontFamily> {
        if index < self.len as usize {
            Some(&self.items[index])
        } else {
            None
        }
    }

    pub fn as_slice(&self) -> &[FontFamily] {
        &self.items[..self.len as usize]
    }

    pub fn iter(&self) -> core::slice::Iter<'_, FontFamily> {
        self.as_slice().iter()
    }
}

impl From<&[&str]> for FontFamilies {
    fn from(names: &[&str]) -> Self {
        Self::from_slice(names)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontDesc {
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
}

impl Default for FontDesc {
    fn default() -> Self {
        Self {
            family: String::new(),
            weight: 400,
            style: FontStyle::Normal,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontData {
    pub desc: FontDesc,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FontSet {
    pub fonts: Vec<FontData>,
}

impl FontSet {
    pub fn get(&self, id: FontId) -> Option<&FontData> {
        self.fonts.get(id.0 as usize)
    }

    pub fn push(&mut self, desc: FontDesc, bytes: Vec<u8>) -> FontId {
        let id = FontId(self.fonts.len() as u16);
        self.fonts.push(FontData { desc, bytes });
        id
    }
}
