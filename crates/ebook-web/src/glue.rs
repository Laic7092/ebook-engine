use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn eb_layout(text: &str, width: f32, height: f32, margin: f32) -> Vec<u8> {
    crate::layout_text(text, width, height, margin)
}

#[wasm_bindgen]
pub fn eb_handle(bytes: &[u8]) -> Vec<u8> {
    crate::protocol::handle(bytes)
}

#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_layout_chapter(
    bytes: &[u8],
    name: &str,
    spine: usize,
    width: f32,
    height: f32,
    margin: f32,
) -> Vec<u8> {
    crate::layout_chapter(bytes, name, spine, width, height, margin)
}

#[wasm_bindgen]
pub fn eb_version() -> u32 {
    (ebook_ir::IR_VERSION.major as u32) << 16 | ebook_ir::IR_VERSION.minor as u32
}
