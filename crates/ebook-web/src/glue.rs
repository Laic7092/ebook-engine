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

/// 打开宿主已经解码好的纯文本。`name` 只用于书名/扩展名展示。
#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_open_text(text: &str, name: &str) -> String {
    crate::open_text(text, name)
}

/// 按字节自动识别 EPUB / TXT。
#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_open_file(bytes: &[u8], name: &str) -> String {
    crate::open_file(bytes, name)
}

/// 关掉当前书并回收它占用的字节。换书前可以由宿主显式调一次；
/// `eb_open*` 自己也会在开头释放上一本。
#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_close_book() {
    crate::close_book()
}

#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_font(bytes: &[u8]) -> u32 {
    crate::register_font(bytes)
}

/// 最近一次 `eb_layout` 失败的原因（成功时为空）。
#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_last_error() -> String {
    crate::last_error()
}

/// 最近一次 `eb_font` / `eb_font_face` 失败的原因（成功或重置后为空）。
#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_font_error() -> String {
    crate::font_error()
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

/// 当前章整页 RGBA（8 字节头 + `w*h*4`），由引擎自己光栅化。
/// 头部与参数含义见 `crate::page_bitmap`；页号越界/未排版返回空数组。
#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_page_bitmap(page: u32, scale: f32, ink: u32, paper: u32, bold: bool) -> Vec<u8> {
    crate::page_bitmap(page, scale, ink, paper, bold)
}

#[cfg(feature = "epub")]
#[wasm_bindgen(js_name = "eb_layout")]
pub fn eb_layout_book(spine: u32, width: f32, height: f32, margin: f32) -> Vec<u8> {
    crate::layout_cached(spine, width, height, margin)
}

#[cfg(feature = "epub")]
#[wasm_bindgen(js_name = "eb_layout_insets")]
#[allow(clippy::too_many_arguments)]
pub fn eb_layout_book_insets(
    spine: u32,
    width: f32,
    height: f32,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    font_scale: f32,
) -> Vec<u8> {
    crate::layout_cached_insets(spine, width, height, left, right, top, bottom, font_scale)
}

#[cfg(feature = "epub")]
#[wasm_bindgen(js_name = "eb_layout_style")]
#[allow(clippy::too_many_arguments)]
pub fn eb_layout_book_style(
    spine: u32,
    width: f32,
    height: f32,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    font_scale: f32,
    line_scale: f32,
    letter_px: f32,
    align: u32,
) -> Vec<u8> {
    crate::layout_cached_style(
        spine, width, height, left, right, top, bottom, font_scale, line_scale, letter_px, align,
    )
}

/// 页级元数据（`[count] + 每页 9 个 f32`）。宿主不解析 IR 就能翻页。
#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_page_meta() -> Vec<u8> {
    crate::page_meta()
}

/// 当前书第一章根节点的字号，宿主用它把“字号 18px”换算成缩放比例。
#[cfg(feature = "epub")]
#[wasm_bindgen]
pub fn eb_base_font_size() -> f32 {
    crate::base_font_size()
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

/// 目录：`[{label,spine,fragment,depth}]`。
#[wasm_bindgen]
pub fn eb_toc() -> String {
    crate::toc_json()
}

/// 当前章节的 `#fragment` → 行号（`u32::MAX` = 没有这个锚点）。
#[wasm_bindgen]
pub fn eb_fragment_line(fragment: &str) -> u32 {
    crate::fragment_line(fragment)
}

/// 全书全文搜索：`{hits:[{spine,node,offset,end,chapter,before,text,after,...}],...}`。
/// `limit == 0` 用默认上限。
#[wasm_bindgen]
pub fn eb_search(query: &str, limit: u32) -> String {
    crate::search_json(query, limit)
}

/// 搜索命中（`node + offset`）→ 当前排版里的行号（`u32::MAX` = 定位不到）。
/// 调用前必须先排版命中所在的章。
#[wasm_bindgen]
pub fn eb_search_locate(node: u32, offset: u32) -> u32 {
    crate::search_locate(node, offset)
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
