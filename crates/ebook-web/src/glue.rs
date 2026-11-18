use wasm_bindgen::prelude::*;

#[cfg(not(feature = "epub"))]
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
pub fn eb_open(bytes: &[u8]) -> String {
    crate::open_book(bytes)
}

#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_font(bytes: &[u8]) -> u32 {
    crate::register_font(bytes)
}

#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_font_reset() {
    crate::reset_fonts()
}

#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_font_count() -> u32 {
    crate::font_count()
}

#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_font_desc(id: u32) -> String {
    crate::font_desc(id)
}

#[cfg(feature = "epub")]
#[wasm_bindgen(js_name = "eb_layout")]
pub fn eb_layout_book(spine: u32, width: f32, height: f32, margin: f32) -> Vec<u8> {
    crate::layout_cached(spine, width, height, margin)
}

#[cfg(feature = "epub")]
#[wasm_bindgen(js_name = "eb_layout_text")]
pub fn eb_layout_text_named(text: &str, width: f32, height: f32, margin: f32) -> Vec<u8> {
    crate::layout_text(text, width, height, margin)
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

// 交互查询：都作用在最近一次 `eb_layout*` 的 IR 上，返回 JSON（与 `protocol.rs`
// 同一套"明确字段"约定），JS 侧由 `decode.js` 解析。
#[wasm_bindgen]
pub fn eb_hit(page: u32, x: f32, y: f32) -> String {
    crate::hit_json(page, x, y)
}

#[wasm_bindgen]
pub fn eb_link_at(page: u32, x: f32, y: f32) -> String {
    crate::link_at_json(page, x, y)
}

#[wasm_bindgen]
pub fn eb_selection(
    a_page: u32,
    a_line: u32,
    a_byte: u32,
    b_page: u32,
    b_line: u32,
    b_byte: u32,
) -> String {
    crate::selection_json(a_page, a_line, a_byte, b_page, b_line, b_byte)
}

#[wasm_bindgen]
pub fn eb_progress(page: u32, line: u32, byte: u32) -> String {
    crate::progress_json(page, line, byte)
}

// ---- 资源、目录与跳转 ------------------------------------------------------
// 与交互查询一样走 JSON（列表/结构体），只有图片字节走 Uint8Array。

/// 归档内资产字节；找不到返回空数组。
#[wasm_bindgen]
pub fn eb_asset(href: &str) -> Vec<u8> {
    crate::asset_bytes(href)
}

/// 最近一次排版那一章的图片表：`[{id,href,mime}]`。
#[wasm_bindgen]
pub fn eb_chapter_images() -> String {
    crate::chapter_images_json()
}

/// 目录：`[{label,spine,fragment,depth}]`。
#[wasm_bindgen]
pub fn eb_toc() -> String {
    crate::toc_json()
}

/// 当前章节的 `#fragment` → Anchor。
#[wasm_bindgen]
pub fn eb_anchor(fragment: &str) -> String {
    crate::anchor_json(fragment)
}

/// 按当前章节解析书内链接：`{target:{spine,fragment,external}}`。
#[wasm_bindgen]
pub fn eb_resolve_link(href: &str) -> String {
    crate::resolve_link_json(href)
}

#[wasm_bindgen]
pub fn eb_font_face(index: u32) -> u32 {
    crate::load_font_face(index as usize)
}

#[wasm_bindgen]
pub fn eb_font_face_count() -> u32 {
    crate::font_face_count()
}

#[wasm_bindgen]
pub fn eb_font_face_bytes(index: u32) -> Vec<u8> {
    crate::load_font_face_bytes(index as usize)
}
