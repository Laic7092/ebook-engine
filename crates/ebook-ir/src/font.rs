use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FontDesc {
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
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
