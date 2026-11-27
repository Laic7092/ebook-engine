pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod glue;

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use ebook_font::FontBook;
use ebook_format::epub::EpubReader;
use ebook_format::txt::TxtReader;
use ebook_format::{Book, ChapterAnchor, ChapterLinks, FormatError, LinkTarget};
use ebook_ir::{
    codec, EdgesF, FontId, GlyphBitmap, ImageData, ImageId, LayoutResult, Location, PointF,
    Resources, Rgba8, SizeF, StyledDoc,
};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, Reader, StubMeasurer};
use ebook_raster::{RgbaSurface, Tone};
use ebook_render::ScaledSurface;
use ebook_search::{SearchCache, SearchOptions, SearchText};

static BOOK: Mutex<Option<Backend>> = Mutex::new(None);
/// `open_book` 把字节借给 `EpubReader<'static>` 之后，这里记下同一块分配的
/// `(ptr, len)`。换书/关书时先丢掉借用者（`BOOK` / `LAST`），再按原样回收，
/// 避免 `Box::leak` 变成“每开一本泄漏一本”。
static BOOK_BYTES: Mutex<Option<(usize, usize)>> = Mutex::new(None);
static FONT: Mutex<Option<FontBook>> = Mutex::new(None);
/// 最近一次排版的 IR 与原文。交互查询（命中/选区/进度）只认这一份，
/// 与 JS 侧当前显示的章节保持同步；排版层本身仍然无状态。
static LAST: Mutex<Option<LastLayout>> = Mutex::new(None);
/// 最近一次排版失败的原因。`eb_layout` 失败时只能返回空 `Vec`，JS 侧靠它
/// 把“排版失败”变成一个具体错误，而不是让用户对着空页面猜。
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
/// 最近一次字体加载失败的原因。字体加载接口保留 `u32` 返回值的兼容性，
/// 诊断单独暴露，避免坏 WOFF/WOFF2 被静默跳过。
static FONT_ERROR: Mutex<Option<String>> = Mutex::new(None);
/// 全文搜索索引。wasm 内存比 native 紧，预算比 `ebook-reader` 小。
static SEARCH: Mutex<SearchCache> = Mutex::new(SearchCache::new(8 * 1024 * 1024));

pub fn last_error() -> String {
    LAST_ERROR.lock().unwrap().clone().unwrap_or_default()
}

pub fn font_error() -> String {
    FONT_ERROR.lock().unwrap().clone().unwrap_or_default()
}

fn set_font_error(error: impl Into<String>) {
    *FONT_ERROR.lock().unwrap() = Some(error.into());
}

fn clear_font_error() {
    *FONT_ERROR.lock().unwrap() = None;
}

struct LastLayout {
    ir: LayoutResult,
    doc: StyledDoc,
    /// 正文默认字色（按文本字节数取主导色）。主题墨色只换它。
    text_color: Rgba8,
    /// 本章 `id → Anchor`。fragment 定位只对最近一次排版的那一章有效。
    anchors: Vec<ChapterAnchor>,
}

impl LastLayout {
    fn reader(&self) -> Reader<'_> {
        Reader::new(&self.ir)
    }
}

/// 已打开的书。两种格式在 wasm 侧共用一套 `LAST` / 字体 / 交互查询，
/// 只在“章怎么来、资产怎么取、链接怎么解析”这三点上分叉。
enum Backend {
    Epub(EpubReader<'static>),
    Txt(TxtReader),
}

impl Backend {
    fn book(&self) -> &Book {
        match self {
            Backend::Epub(reader) => reader.book(),
            Backend::Txt(reader) => reader.book(),
        }
    }

    fn chapter_with_anchors(
        &self,
        index: usize,
    ) -> Result<(StyledDoc, Vec<ChapterLinks>, Vec<ChapterAnchor>), FormatError> {
        match self {
            Backend::Epub(reader) => reader.chapter_with_anchors(index),
            Backend::Txt(reader) => reader.chapter_with_anchors(index),
        }
    }

    fn resolve_link_from(&self, spine: usize, href: &str) -> Option<LinkTarget> {
        match self {
            Backend::Epub(reader) => reader.resolve_link_from(spine, href),
            Backend::Txt(reader) => reader.resolve_link_from(spine, href),
        }
    }

    fn asset_owned(&self, href: &str) -> Option<(Vec<u8>, String)> {
        match self {
            Backend::Epub(reader) => reader.asset_owned(href),
            Backend::Txt(_) => None,
        }
    }

    fn font_bytes(&self, index: usize) -> Option<Vec<u8>> {
        match self {
            Backend::Epub(reader) => reader.font_bytes(index),
            Backend::Txt(_) => None,
        }
    }
}

pub fn layout_text(text: &str, width: f32, height: f32, margin: f32) -> Vec<u8> {
    let doc = ebook_format::txt::build(text.as_bytes());
    layout_doc(&doc, width, height, margin)
}

pub fn open_book(bytes: &[u8]) -> String {
    // 先把上一本书的借用者与字节都放掉，再开新的。
    close_book();
    let boxed = bytes.to_vec().into_boxed_slice();
    let len = boxed.len();
    let leaked: &'static [u8] = Box::leak(boxed);
    // 字体字节懒加载：打开书不再把全部内嵌字体解压进内存。
    match EpubReader::open_lazy(leaked) {
        Ok(reader) => {
            // 只在开书成功后才登记所有权；失败路径在下面直接回收。
            if len > 0 {
                *BOOK_BYTES.lock().unwrap() = Some((leaked.as_ptr() as usize, len));
            }
            install(Backend::Epub(reader))
        }
        Err(error) => {
            free_book_bytes(leaked.as_ptr() as *mut u8, len);
            error_json(&format!("open: {error:?}"))
        }
    }
}

/// 关掉当前书：丢掉它的排版结果与借用者，再回收 `open_book` 交出去的字节。
///
/// 宿主换书时可以直接调；`open_book` / `open_text` 也会在开头自动调一次，
/// 所以不调也不会因为翻书而泄漏，只是旧书会活到下一次打开。
pub fn close_book() {
    reset_session();
    release_book_bytes();
    *LAST_ERROR.lock().unwrap() = None;
}

/// 丢掉当前书与它的排版结果。必须在释放书字节之前调用：`BOOK` / `LAST`
/// 里的 `EpubReader<'static>` 借用的正是那块字节。
fn reset_session() {
    // 与 `page_bitmap` 的取锁顺序保持一致：先 BOOK，再 LAST（FONT 不在这里动）。
    *BOOK.lock().unwrap() = None;
    *LAST.lock().unwrap() = None;
    IMAGES.lock().unwrap().clear();
    SEARCH.lock().unwrap().clear();
}

fn release_book_bytes() {
    let Some((ptr, len)) = BOOK_BYTES.lock().unwrap().take() else {
        return;
    };
    free_book_bytes(ptr as *mut u8, len);
}

/// 回收 `Box::leak` 出去的那块 `[u8]`。调用方必须保证没有存活借用。
fn free_book_bytes(ptr: *mut u8, len: usize) {
    if len == 0 || ptr.is_null() {
        return;
    }
    // SAFETY: `ptr` / `len` 来自 `Box::leak(bytes.to_vec().into_boxed_slice())`；
    // 调用点要么已经 drop 掉 `Backend::Epub`（它持有唯一的 `&'static [u8]`），
    // 要么 `EpubReader::open` 已经失败返回、借用已结束。
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

/// 打开已经由宿主解码好的纯文本。章节在格式层按标题行切分，
/// TOC / spine / 交互查询因此和 EPUB 完全同构。
pub fn open_text(text: &str, name: &str) -> String {
    close_book();
    install(Backend::Txt(TxtReader::from_text(text, Some(name))))
}

/// 按魔数自动识别：ZIP 交给 EPUB，否则按 UTF-8 文本处理。
/// 浏览器会先用 `TextDecoder` 处理 BOM/GBK，再走 `eb_open_text`；
/// 这个入口留给直接给字节的宿主与测试。
pub fn open_file(bytes: &[u8], name: &str) -> String {
    if bytes.starts_with(b"PK") {
        open_book(bytes)
    } else {
        open_text(&String::from_utf8_lossy(bytes), name)
    }
}

fn install(backend: Backend) -> String {
    let json = book_json(backend.book());
    // 交互状态跟着上一本书：打开新书时 `LAST` 必须清掉，否则排版之前
    // 的查询/位图会读到旧书的页与行。
    *LAST.lock().unwrap() = None;
    *LAST_ERROR.lock().unwrap() = None;
    *BOOK.lock().unwrap() = Some(backend);
    // 图片缓存属于上一本书：href 空间会变，留着只会浪费内存。
    IMAGES.lock().unwrap().clear();
    SEARCH.lock().unwrap().clear();
    json
}

pub fn register_font(bytes: &[u8]) -> u32 {
    let mut slot = FONT.lock().unwrap();
    let font = slot.get_or_insert_with(FontBook::new);
    match font.try_add(bytes.to_vec()) {
        Ok(id) => {
            clear_font_error();
            id.0 as u32
        }
        Err(error) => {
            set_font_error(error.to_string());
            u32::MAX
        }
    }
}

pub fn reset_fonts() {
    *FONT.lock().unwrap() = None;
    clear_font_error();
}

/// 当前章整页 RGBA：引擎自己在 wasm 上把一页画完，宿主只负责 `putImageData`。
///
/// 返回 8 字节小端头 + 像素：`[w:u32, h:u32, rgba...]`，后面是 `w*h*4` 字节。
/// `scale` 是设备像素比（非有限/`<=0` 按 1，上限 4）；`ink` / `paper` 是
/// `0xRRGGBBAA`：`ink` 为 `0` 时不做墨色/伽马修正，`paper` 为 `0` 时是白纸。
/// `ink != 0` 只换默认字色（正文主导色），作者显式写的颜色保留。
/// 主题反转仍由宿主做（引擎只认纸色/墨色，不认识 sepia/dark）。
/// `bold` 是宿主的“全局加粗”开关：绘制阶段把 face 换成同一 family 的合成粗体。
///
/// 与 native `--ppm` 走同一个 `paint()` + `RgbaSurface`，所以两边几何一致；
/// 页号越界、还没排版、没有书、或像素数超上限时返回空数组。
pub fn page_bitmap(page: u32, scale: f32, ink: u32, paper: u32, bold: bool) -> Vec<u8> {
    // 锁序与 `layout_cached_edges` 一致：BOOK → FONT → LAST。
    let books = BOOK.lock().unwrap();
    let Some(book) = books.as_ref() else {
        return Vec::new();
    };
    let mut font = FONT.lock().unwrap();
    let last = LAST.lock().unwrap();
    let Some(last) = last.as_ref() else {
        return Vec::new();
    };
    let Some(info) = last.ir.page(page) else {
        return Vec::new();
    };
    let scale = if scale.is_finite() && scale > 0.0 {
        scale.clamp(0.25, 4.0)
    } else {
        1.0
    };
    let box_size = info.box_size();
    let w = (box_size.w * scale).round().max(1.0) as u32;
    let h = (box_size.h * scale).round().max(1.0) as u32;
    if (w as u64) * (h as u64) > MAX_PAGE_PIXELS {
        return Vec::new();
    }

    let mut surface = RgbaSurface::new(w, h);
    if ink != 0 {
        let ink = unpack_rgba(ink);
        let base = last.text_color;
        surface.set_tone(Some(Tone::ink(ink)));
        surface.set_ink_replacing(Some(ink), base);
    }
    surface.clear(if paper == 0 {
        Rgba8::WHITE
    } else {
        unpack_rgba(paper)
    });
    {
        let mut res = PageResources {
            font: font.as_mut(),
            bold,
            book,
            images: &last.doc.images,
            srcs: Vec::new(),
        };
        let mut scaled = ScaledSurface::new(&mut surface, scale);
        if ebook_render::paint(&last.ir, page, &mut scaled, &mut res).is_err() {
            return Vec::new();
        }
    }

    let mut out = Vec::with_capacity(8 + surface.pixels().len() * 4);
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&surface.into_rgba());
    out
}

/// 一页设备像素的上限：4096×4096。1170×2532@3x ≈ 2.96M，余量足够，
/// 但拦得住“把 `scale` 传成 1000”的那种调用。
const MAX_PAGE_PIXELS: u64 = 4096 * 4096;

/// 书级图片缓存的上限。CJK 字体、整本 zip 已经在内存里，图片再不设上限，
/// 移动端 wasm 很容易触顶（G-16）。
const IMAGE_CACHE_BYTES: usize = 16 * 1024 * 1024;

fn target_pixels(target: SizeF) -> (u32, u32) {
    let side = |value: f32| -> u32 {
        if value.is_finite() && value > 0.0 {
            value.ceil().min(16_384.0) as u32
        } else {
            0
        }
    };
    (side(target.w), side(target.h))
}

fn unpack_rgba(value: u32) -> Rgba8 {
    Rgba8::new(
        (value >> 24) as u8,
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    )
}

/// 解码后的图片按归档路径缓存，换书时整体清空。
///
/// 图片字节本身还在 `EpubReader` 里，这里只留 RGBA，省掉翻页时重复解码。
/// 用 `Vec` 而不是哈希表：一页涉及的图很少，线性查找比哈希更省内存，
/// 也与“IR 里禁止哈希容器”的取向一致。
#[derive(Default)]
struct ImageCache {
    entries: Vec<(String, u32, u32, Option<Arc<ImageData>>)>,
    bytes: usize,
}

impl ImageCache {
    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn get(&mut self, src: &str, w: u32, h: u32) -> Option<Arc<ImageData>> {
        self.entries
            .iter()
            .find(|(cached, cw, ch, _)| cached == src && *cw == w && *ch == h)
            .and_then(|(_, _, _, hit)| hit.clone())
    }

    fn insert(&mut self, src: String, w: u32, h: u32, data: Option<Arc<ImageData>>) {
        if let Some(image) = data.as_ref() {
            self.bytes += image.rgba.len();
        }
        self.entries.push((src, w, h, data));
        // 超预算就丢最旧的条目，直到回到预算内（至少留一条，避免反复解码同一张）。
        while self.bytes > IMAGE_CACHE_BYTES && self.entries.len() > 1 {
            let (_, _, _, dropped) = self.entries.remove(0);
            if let Some(image) = dropped {
                self.bytes = self.bytes.saturating_sub(image.rgba.len());
            }
        }
    }
}

static IMAGES: Mutex<ImageCache> = Mutex::new(ImageCache {
    entries: Vec::new(),
    bytes: 0,
});

/// 一页的资源视图：字形来自 `FontBook`，图片来自当前书的归档。
///
/// `ImageId` 只是本章图片表的下标，缓存键用归档路径（`src`），
/// 所以跨章复用同一张图不会重复解码。
struct PageResources<'a> {
    font: Option<&'a mut FontBook>,
    bold: bool,
    book: &'a Backend,
    images: &'a [ebook_ir::ImageMeta],
    /// 本页用到的 `id → src`，避免每画一次都重新取元信息。
    srcs: Vec<(ImageId, Option<String>)>,
}

impl PageResources<'_> {
    fn src_of(&mut self, id: ImageId) -> Option<String> {
        if let Some((_, hit)) = self.srcs.iter().find(|(cached, _)| *cached == id) {
            return hit.clone();
        }
        let src = self
            .images
            .get(id as usize)
            .and_then(|meta| meta.src.clone());
        self.srcs.push((id, src.clone()));
        src
    }
}

impl Resources for PageResources<'_> {
    fn glyph_alpha(&mut self, font: FontId, gid: u16, px: f32) -> Option<Arc<GlyphBitmap>> {
        self.glyph_alpha_at(font, gid, px, 0.0, 0.0)
    }

    fn glyph_alpha_at(
        &mut self,
        font: FontId,
        gid: u16,
        px: f32,
        dx: f32,
        dy: f32,
    ) -> Option<Arc<GlyphBitmap>> {
        let book = self.font.as_mut()?;
        // 全局加粗只影响取哪张 face，不改 IR：同一份 IR 可以按开关重画。
        let face = if self.bold {
            book.bold_face(font)
        } else {
            font
        };
        book.rasterize_at(face, gid, px, dx, dy)
    }

    fn image(&mut self, id: ImageId, target: SizeF) -> Option<Arc<ImageData>> {
        let src = self.src_of(id)?;
        let (tw, th) = target_pixels(target);
        let mut cache = IMAGES.lock().unwrap();
        if let Some(hit) = cache.get(&src, tw, th) {
            return Some(hit);
        }
        let decoded = self
            .book
            .asset_owned(&src)
            .and_then(|(bytes, _)| ebook_format::image::decode_image_scaled(&bytes, tw, th))
            .map(Arc::new);
        let hit = decoded.clone();
        cache.insert(src, tw, th, decoded);
        hit
    }
}

pub fn font_count() -> u32 {
    FONT.lock()
        .unwrap()
        .as_ref()
        .map(|font| font.len() as u32)
        .unwrap_or(0)
}

/// 书里声明了多少个 `@font-face`。JS 用它决定要不要逐个调 `eb_font_face`。
pub fn font_face_count() -> u32 {
    let books = BOOK.lock().unwrap();
    books
        .as_ref()
        .map(|reader| reader.book().fonts().len() as u32)
        .unwrap_or(0)
}

/// 把第 `index` 个内嵌字体按 `@font-face` 的 family/weight/style 装进 `FontBook`，
/// 返回 `FontId`；失败返回 `u32::MAX`。字体字节同时暴露给浏览器侧的 `FontFace`。
pub fn load_font_face(index: usize) -> u32 {
    let (family, weight, style, href) = {
        let books = BOOK.lock().unwrap();
        let Some(reader) = books.as_ref() else {
            return u32::MAX;
        };
        let Some(face) = reader.book().fonts().get(index) else {
            return u32::MAX;
        };
        (
            face.family.clone(),
            face.weight,
            face.style,
            face.href.clone(),
        )
    };
    let Some(bytes) = BOOK
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|reader| reader.font_bytes(index))
    else {
        set_font_error(format!("{href}: 字体字节读不出来"));
        return u32::MAX;
    };
    let mut slot = FONT.lock().unwrap();
    let font = slot.get_or_insert_with(FontBook::new);
    match font.try_add_styled(&family, weight, style, bytes) {
        Ok(id) => {
            clear_font_error();
            id.0 as u32
        }
        Err(error) => {
            set_font_error(format!("{href}: {error}"));
            u32::MAX
        }
    }
}

/// 目录：`[{"label","spine","fragment","depth"}]`。
/// `spine` 不在阅读顺序里（封面、注释页）时为 `null`。
pub fn toc_json() -> String {
    let books = BOOK.lock().unwrap();
    let Some(reader) = books.as_ref() else {
        return String::from("[]");
    };
    let mut out = String::from("[");
    for (index, entry) in reader.book().toc().iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"label\":");
        push_json_str(&mut out, &entry.label);
        out.push_str(",\"spine\":");
        if entry.in_spine() {
            let _ = write!(out, "{}", entry.spine);
        } else {
            out.push_str("null");
        }
        out.push_str(",\"fragment\":");
        push_json_opt(&mut out, entry.fragment.as_deref());
        out.push_str(",\"depth\":");
        let _ = write!(out, "{}", entry.depth);
        out.push('}');
    }
    out.push(']');
    out
}

/// 当前章节里 `#fragment` 对应的行号（`u32::MAX` = 本章没有这个锚点）。
///
/// 宿主只需要行号：翻页/滚动是行号问题，锚点结构（spine/path/offset）是
/// 引擎内部表示，不该穿过 ABI。
pub fn fragment_line(fragment: &str) -> u32 {
    let line = with_last(|last| {
        let anchor = last
            .anchors
            .iter()
            .find(|entry| entry.id == fragment)
            .map(|entry| &entry.anchor)?;
        let mut fallback = None;
        for (index, line) in last.ir.runs.lines.iter().enumerate() {
            let Some(line_anchor) = line.anchor.as_ref() else {
                continue;
            };
            if line_anchor.spine != anchor.spine || !line_anchor.path.starts_with(&anchor.path) {
                continue;
            }
            // 元素级锚点（`<p id>`）落在更深的文本节点路径上：定位到该元素的
            // 第一行。同一层路径才谈得上字节偏移。
            if line_anchor.path.len() > anchor.path.len() {
                return Some(index as u32);
            }
            if anchor.offset >= line_anchor.offset
                && anchor.offset - line_anchor.offset <= line.src.len
            {
                return Some(index as u32);
            }
            fallback = Some(index as u32);
        }
        fallback
    })
    .flatten();
    line.unwrap_or(u32::MAX)
}

/// 页级元数据：`[count: u32]` + 每页 9 个 f32
/// （`w, h, left, right, top, bottom, line_start, line_len, progress`）。
///
/// 宿主只需要页盒子、行区间和本章进度就能翻页/滚动/画进度条，不需要整份
/// IR。这比在 JS 里解 IR 少两个数量级的对象分配。
pub fn page_meta() -> Vec<u8> {
    let pages = with_last(|last| {
        let reader = last.reader();
        last.ir
            .pages
            .iter()
            .map(|page| {
                [
                    page.size.w,
                    page.size.h,
                    page.inset.left,
                    page.inset.right,
                    page.inset.top,
                    page.inset.bottom,
                    page.lines.start as f32,
                    page.lines.len as f32,
                    reader.progress(Location {
                        page: page.index,
                        line: page.lines.start,
                        byte: 0,
                    }),
                ]
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let mut out = Vec::with_capacity(4 + pages.len() * 36);
    out.extend_from_slice(&(pages.len() as u32).to_le_bytes());
    for page in &pages {
        for value in page {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

/// 在**当前章节**的上下文里解析书内链接：
/// `{"target":{"spine":1,"fragment":"n","external":false}}` 或 `{"target":null}`。
pub fn resolve_link_json(href: &str) -> String {
    // 先取 spine，释放 LAST 再锁 BOOK，避免与排版路径的加锁顺序相反。
    let spine = with_last(|last| last.ir.spine);
    let target = match spine {
        Some(spine) => {
            let books = BOOK.lock().unwrap();
            books
                .as_ref()
                .and_then(|reader| reader.resolve_link_from(spine as usize, href))
        }
        None => None,
    };
    let mut out = String::from("{\"target\":");
    match target {
        Some(target) => {
            out.push_str("{\"spine\":");
            let _ = write!(out, "{}", target.spine);
            out.push_str(",\"fragment\":");
            push_json_opt(&mut out, target.fragment.as_deref());
            out.push_str(",\"external\":");
            out.push_str(if target.external { "true" } else { "false" });
            out.push('}');
        }
        None => out.push_str("null"),
    }
    out.push('}');
    out
}

fn scale_doc_fonts(doc: &mut ebook_ir::StyledDoc, scale: f32) {
    if !scale.is_finite() || scale <= 0.0 || (scale - 1.0).abs() < 1.0e-4 {
        return;
    }
    for node in &mut doc.nodes {
        node.style.font.size = (node.style.font.size * scale).max(0.0);
        if let ebook_ir::LineHeight::Px(px) = &mut node.style.line_height {
            *px *= scale;
        }
        if let ebook_ir::Len::Px(px) = &mut node.style.text.indent {
            *px *= scale;
        }
        node.style.text.letter_spacing *= scale;
        node.style.text.word_spacing *= scale;
    }
}

/// 阅读器的独立排版选项。`font_scale` 只缩放文档字号/行高/间距；
/// 行距、字距和对齐在缩放之后覆盖，不改变页面几何。
fn apply_doc_text_options(
    doc: &mut ebook_ir::StyledDoc,
    line_scale: f32,
    letter_px: f32,
    align: u32,
) {
    let line_scale = if line_scale.is_finite() && line_scale > 0.0 {
        line_scale
    } else {
        1.0
    };
    let letter_px = if letter_px.is_finite() {
        letter_px
    } else {
        0.0
    };
    let align = match align {
        1 => Some(ebook_ir::TextAlign::Start),
        2 => Some(ebook_ir::TextAlign::Center),
        3 => Some(ebook_ir::TextAlign::End),
        4 => Some(ebook_ir::TextAlign::Justify),
        _ => None,
    };
    for node in &mut doc.nodes {
        // 阅读器的行距是绝对倍数（1.5 = 1.5em），不叠在书里的 line-height 上。
        if line_scale.is_finite() && line_scale > 0.0 {
            node.style.line_height = ebook_ir::LineHeight::Mul(line_scale);
        }
        if letter_px.abs() > 1.0e-4 {
            node.style.text.letter_spacing += letter_px;
        }
        if let Some(align) = align {
            node.style.text.align = align;
        }
    }
}

/// 全书全文搜索：`{"hits":[...],"truncated":false,"scanned":N,"skipped":M}`。
///
/// 索引按章缓存，重复搜索不会重新解析整本书。命中里的 `node + offset`
/// 是原文绝对位置；排版目标章后用 [`search_locate`] 换成行号。
pub fn search_json(query: &str, limit: u32) -> String {
    let books = BOOK.lock().unwrap();
    let Some(reader) = books.as_ref() else {
        return String::from("{\"hits\":[],\"truncated\":false,\"scanned\":0,\"skipped\":0}");
    };
    let count = reader.book().spine.len();
    let limit = if limit == 0 {
        ebook_search::DEFAULT_MAX_HITS
    } else {
        limit
    };
    if count == 0 || query.trim().is_empty() {
        return String::from("{\"hits\":[],\"truncated\":false,\"scanned\":0,\"skipped\":0}");
    }
    let titles: Vec<Option<String>> = (0..count)
        .map(|spine| {
            reader
                .book()
                .toc
                .iter()
                .find(|entry| entry.spine == spine)
                .map(|entry| entry.label.clone())
        })
        .collect();

    let mut cache = SEARCH.lock().unwrap();
    let mut out = String::from("{\"hits\":[");
    let mut pushed = 0u32;
    let mut scanned = 0u32;
    let mut skipped = 0u32;
    let mut truncated = false;
    for (spine, title) in titles.iter().enumerate() {
        scanned = scanned.saturating_add(1);
        if cache.get(spine as u32, 0).is_none() {
            match reader.chapter_with_anchors(spine) {
                Ok((doc, _, _)) => {
                    let text = SearchText::build(&doc);
                    cache.insert(spine as u32, 0, text);
                }
                Err(_) => {
                    skipped = skipped.saturating_add(1);
                    continue;
                }
            }
        }
        let remaining = limit.saturating_sub(pushed);
        if remaining == 0 {
            truncated = true;
            break;
        }
        let options = SearchOptions {
            max_hits: remaining,
            ..SearchOptions::default()
        };
        let mut found = Vec::new();
        let chapter_truncated = match cache.get(spine as u32, 0) {
            Some(index) => index.find(query, &options, &mut found),
            None => false,
        };
        for hit in found {
            if pushed > 0 {
                out.push(',');
            }
            out.push_str("{\"spine\":");
            let _ = write!(out, "{}", spine);
            out.push_str(",\"node\":");
            let _ = write!(out, "{}", hit.node);
            out.push_str(",\"offset\":");
            let _ = write!(out, "{}", hit.source_start);
            out.push_str(",\"end\":");
            let _ = write!(out, "{}", hit.source_end);
            out.push_str(",\"chapter\":");
            push_json_opt(&mut out, title.as_deref());
            out.push_str(",\"before\":");
            push_json_str(&mut out, &hit.before);
            out.push_str(",\"text\":");
            push_json_str(&mut out, &hit.text);
            out.push_str(",\"after\":");
            push_json_str(&mut out, &hit.after);
            out.push_str(",\"ellipsis_before\":");
            out.push_str(if hit.ellipsis_before { "true" } else { "false" });
            out.push_str(",\"ellipsis_after\":");
            out.push_str(if hit.ellipsis_after { "true" } else { "false" });
            out.push('}');
            pushed = pushed.saturating_add(1);
        }
        if chapter_truncated {
            truncated = true;
            break;
        }
    }
    out.push_str("],\"truncated\":");
    out.push_str(if truncated { "true" } else { "false" });
    out.push_str(",\"scanned\":");
    let _ = write!(out, "{scanned}");
    out.push_str(",\"skipped\":");
    let _ = write!(out, "{skipped}");
    out.push('}');
    out
}

/// 搜索命中（原文节点 + 绝对字节偏移）→ 当前排版里的行号。
/// 目标章还没排版、节点不属于当前章或偏移越界时返回 `u32::MAX`。
pub fn search_locate(node: u32, offset: u32) -> u32 {
    with_last(|last| {
        let anchor = last.doc.anchor_of(node)?.with_offset(offset);
        last.ir.location_of(&anchor).map(|location| location.line)
    })
    .flatten()
    .unwrap_or(u32::MAX)
}

/// 当前书第一章根节点的字号；网页用它把“18px”换算成文档内的缩放比例。
pub fn base_font_size() -> f32 {
    let books = BOOK.lock().unwrap();
    let Some(reader) = books.as_ref() else {
        return 16.0;
    };
    match reader.chapter_with_anchors(0) {
        Ok((doc, _, _)) => {
            // 正文通常不是根节点的字号：取文本节点里最常见的字号当“基准”。
            let mut sizes: Vec<(f32, usize)> = Vec::new();
            for node in &doc.nodes {
                let ebook_ir::NodeKind::Text(span) = node.kind else {
                    continue;
                };
                if doc.text_of(span).trim().is_empty() {
                    continue;
                }
                let size = node.style.font.size;
                if !size.is_finite() || size <= 0.0 {
                    continue;
                }
                match sizes
                    .iter_mut()
                    .find(|(known, _)| (*known - size).abs() < 0.05)
                {
                    Some((_, count)) => *count += 1,
                    None => sizes.push((size, 1)),
                }
            }
            sizes
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(size, _)| size)
                .or_else(|| {
                    doc.nodes
                        .get(doc.root as usize)
                        .map(|node| node.style.font.size)
                        .filter(|size| *size > 0.0)
                })
                .unwrap_or(16.0)
        }
        Err(_) => 16.0,
    }
}

pub fn layout_cached(spine: u32, width: f32, height: f32, margin: f32) -> Vec<u8> {
    layout_cached_edges(spine, width, height, EdgesF::all(margin), 1.0, 1.0, 0.0, 0)
}

/// 四边可不同的排版入口。网页阅读器要让正文避开顶部/底部工具栏，同时保持
/// 页面本身铺满视口；工具栏显隐只改变覆盖层，不再触发布局。`font_scale`
/// 只缩放文档字号与行高，页面尺寸保持不变。
#[allow(clippy::too_many_arguments)]
pub fn layout_cached_insets(
    spine: u32,
    width: f32,
    height: f32,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    font_scale: f32,
) -> Vec<u8> {
    layout_cached_style(
        spine, width, height, left, right, top, bottom, font_scale, 1.0, 0.0, 0,
    )
}

/// 在四边页边距之上，再独立设置行距、字距和对齐。
/// `align`: 0=保留原书，1=左/起始，2=居中，3=右/末尾，4=两端对齐。
#[allow(clippy::too_many_arguments)]
pub fn layout_cached_style(
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
    layout_cached_edges(
        spine,
        width,
        height,
        EdgesF {
            top,
            right,
            bottom,
            left,
        },
        font_scale,
        line_scale,
        letter_px,
        align,
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_cached_edges(
    spine: u32,
    width: f32,
    height: f32,
    margin: EdgesF,
    font_scale: f32,
    line_scale: f32,
    letter_px: f32,
    align: u32,
) -> Vec<u8> {
    let books = BOOK.lock().unwrap();
    let Some(reader) = books.as_ref() else {
        return Vec::new();
    };
    let (mut doc, _, anchors) = match reader.chapter_with_anchors(spine as usize) {
        Ok(value) => value,
        Err(error) => {
            *LAST_ERROR.lock().unwrap() = Some(format!("{error:?}"));
            return Vec::new();
        }
    };
    scale_doc_fonts(&mut doc, font_scale);
    apply_doc_text_options(&mut doc, line_scale, letter_px, align);
    *LAST_ERROR.lock().unwrap() = None;
    let mut font = FONT.lock().unwrap();
    // 语言是排版入参：文档语言要在 shape 前注入，locl 之类才会生效。
    if let Some(font) = font.as_mut() {
        font.set_language(doc.meta.lang.as_deref());
    }
    match font.as_ref() {
        Some(font) => layout_doc_with_edges(&doc, font, width, height, margin, anchors),
        None => layout_doc_full_edges(&doc, &StubMeasurer, width, height, margin, anchors),
    }
}

#[cfg(feature = "epub")]
pub fn layout_chapter(
    source: &[u8],
    name: &str,
    spine: usize,
    width: f32,
    height: f32,
    margin: f32,
) -> Vec<u8> {
    let mut src = ebook_format::MemorySource::new();
    src.insert(name, source.to_vec());
    let Ok(opened) = ebook_format::open(&src) else {
        return Vec::new();
    };
    let Ok(doc) = opened.chapter(&src, spine) else {
        return Vec::new();
    };
    layout_doc(&doc, width, height, margin)
}

fn layout_doc(doc: &ebook_ir::StyledDoc, width: f32, height: f32, margin: f32) -> Vec<u8> {
    layout_doc_full_edges(
        doc,
        &StubMeasurer,
        width,
        height,
        EdgesF::all(margin),
        Vec::new(),
    )
}

fn layout_doc_with_edges(
    doc: &ebook_ir::StyledDoc,
    measurer: &dyn ebook_layout::TextMeasurer,
    width: f32,
    height: f32,
    margin: EdgesF,
    anchors: Vec<ChapterAnchor>,
) -> Vec<u8> {
    layout_doc_full_edges(doc, measurer, width, height, margin, anchors)
}

fn layout_doc_full_edges(
    doc: &ebook_ir::StyledDoc,
    measurer: &dyn ebook_layout::TextMeasurer,
    width: f32,
    height: f32,
    margin: EdgesF,
    anchors: Vec<ChapterAnchor>,
) -> Vec<u8> {
    let env = LayoutEnv {
        measurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(width, height).with_margin(margin);
    let ir = Layout::new().layout(doc, &env, &constraint);
    *LAST.lock().unwrap() = Some(LastLayout {
        text_color: ebook_ir::default_text_color(doc),
        ir: ir.clone(),
        doc: doc.clone(),
        anchors,
    });
    codec::to_bytes(&ir).unwrap_or_default()
}

fn with_last<T>(read: impl FnOnce(&LastLayout) -> T) -> Option<T> {
    let slot = LAST.lock().unwrap();
    slot.as_ref().map(read)
}

/// 页内坐标 → 原文位置。`{"location":{...}}` 或 `{"location":null}`。
pub fn hit_json(page: u32, x: f32, y: f32) -> String {
    let hit = with_last(|last| last.reader().hit(page, PointF::new(x, y))).flatten();
    let mut out = String::from("{\"location\":");
    match hit {
        Some(loc) => push_location(&mut out, loc),
        None => out.push_str("null"),
    }
    out.push('}');
    out
}

/// 页内坐标 → 链接目标。`{"href":"..."}` 或 `{"href":null}`。
pub fn link_at_json(page: u32, x: f32, y: f32) -> String {
    let href = with_last(|last| {
        last.reader()
            .link_at(page, PointF::new(x, y))
            .map(str::to_string)
    })
    .flatten();
    let mut out = String::from("{\"href\":");
    match href {
        Some(href) => push_json_str(&mut out, &href),
        None => out.push_str("null"),
    }
    out.push('}');
    out
}

/// 两个 `Location` → 选区矩形 + 原文。矩形是页内坐标，正序、按页/行返回。
pub fn selection_json(
    a_page: u32,
    a_line: u32,
    a_byte: u32,
    b_page: u32,
    b_line: u32,
    b_byte: u32,
) -> String {
    with_last(|last| {
        let reader = last.reader();
        let sel = reader.selection(
            Location {
                page: a_page,
                line: a_line,
                byte: a_byte,
            },
            Location {
                page: b_page,
                line: b_line,
                byte: b_byte,
            },
        );
        let (start, end) = sel.normalize();
        let mut out = String::from("{\"start\":");
        push_location(&mut out, start);
        out.push_str(",\"end\":");
        push_location(&mut out, end);
        out.push_str(",\"rects\":[");
        for (index, item) in reader.selection_rects(&sel).iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "{{\"page\":{},\"x\":", item.page);
            push_f32(&mut out, item.rect.x);
            out.push_str(",\"y\":");
            push_f32(&mut out, item.rect.y);
            out.push_str(",\"w\":");
            push_f32(&mut out, item.rect.w);
            out.push_str(",\"h\":");
            push_f32(&mut out, item.rect.h);
            out.push('}');
        }
        out.push_str("],\"text\":");
        push_json_str(&mut out, &reader.selection_text(&last.doc, &sel));
        out.push('}');
        out
    })
    .unwrap_or_else(|| String::from("{\"rects\":[],\"text\":\"\"}"))
}

/// 全书进度，0..=1。`{"progress":0.25}`。
pub fn progress_json(page: u32, line: u32, byte: u32) -> String {
    let mut out = String::from("{\"progress\":");
    let progress =
        with_last(|last| last.reader().progress(Location { page, line, byte })).unwrap_or(0.0);
    push_f32(&mut out, progress);
    out.push('}');
    out
}

fn push_location(out: &mut String, loc: Location) {
    let _ = write!(
        out,
        "{{\"page\":{},\"line\":{},\"byte\":{}}}",
        loc.page, loc.line, loc.byte
    );
}

fn push_f32(out: &mut String, value: f32) {
    if value.is_finite() {
        let _ = write!(out, "{value}");
    } else {
        out.push('0');
    }
}

fn error_json(message: &str) -> String {
    let mut out = String::from("{\"error\":");
    push_json_str(&mut out, message);
    out.push('}');
    out
}

fn book_json(book: &Book) -> String {
    let mut out = String::from("{\"title\":");
    push_json_opt(&mut out, book.title.as_deref());
    out.push_str(",\"language\":");
    push_json_opt(&mut out, book.lang.as_deref());
    out.push_str(",\"lang\":");
    push_json_opt(&mut out, book.lang.as_deref());
    out.push_str(",\"creator\":");
    push_json_opt(&mut out, book.creator.as_deref());
    out.push_str(",\"creators\":[");
    for (index, creator) in book.creators.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        push_json_str(&mut out, &creator.name);
        out.push_str(",\"role\":");
        push_json_opt(&mut out, creator.role.as_deref());
        out.push_str(",\"fileAs\":");
        push_json_opt(&mut out, creator.file_as.as_deref());
        out.push('}');
    }
    out.push(']');
    out.push_str(",\"publisher\":");
    push_json_opt(&mut out, book.publisher.as_deref());
    out.push_str(",\"identifier\":");
    push_json_opt(&mut out, book.identifier.as_deref());
    out.push_str(",\"date\":");
    push_json_opt(&mut out, book.date.as_deref());
    out.push_str(",\"description\":");
    push_json_opt(&mut out, book.description.as_deref());
    out.push_str(",\"cover\":");
    push_json_opt(&mut out, book.cover_href.as_deref());
    out.push_str(",\"spine\":[");
    for (index, item) in book.spine.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"index\":");
        let _ = write!(out, "{index}");
        out.push_str(",\"id\":");
        push_json_str(&mut out, &item.id);
        out.push_str(",\"href\":");
        push_json_str(&mut out, &item.href);
        out.push_str(",\"mediaType\":");
        push_json_str(&mut out, &item.media_type);
        out.push_str(",\"media\":");
        push_json_str(&mut out, &item.media_type);
        out.push_str(",\"linear\":");
        out.push_str(if item.linear { "true" } else { "false" });
        out.push('}');
    }
    out.push_str("],\"fonts\":[");
    for (index, face) in book.fonts().iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"family\":");
        push_json_str(&mut out, &face.family);
        out.push_str(",\"weight\":");
        let _ = write!(out, "{}", face.weight);
        out.push_str(",\"style\":");
        push_json_str(&mut out, font_style_name(face.style));
        out.push_str(",\"href\":");
        push_json_str(&mut out, &face.href);
        out.push('}');
    }
    out.push_str("]}");
    out
}

fn push_json_opt(out: &mut String, value: Option<&str>) {
    match value {
        Some(value) => push_json_str(out, value),
        None => out.push_str("null"),
    }
}

fn font_style_name(style: ebook_ir::FontStyle) -> &'static str {
    match style {
        ebook_ir::FontStyle::Normal => "normal",
        ebook_ir::FontStyle::Italic => "italic",
        ebook_ir::FontStyle::Oblique => "oblique",
    }
}

fn push_json_str(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 交互 JSON 的形状与时序：排版后 `LAST` 就是查询目标。
    /// 一个测试串起来跑，避免并行测试互相覆盖 `LAST`。
    #[test]
    fn interaction_json_shapes() {
        // txt 排版：内容宽 = 520 - 2×40 = 440，行高由默认字号决定；只要有行就行。
        let bytes = layout_text("hello world hello world", 520.0, 300.0, 40.0);
        assert!(!bytes.is_empty());
        let ir: LayoutResult = ebook_ir::decode(&bytes).unwrap();
        let first = &ir.runs.lines[0];

        // 页内坐标 = 流坐标 + inset（margin 40）。行 0 左上角对应页内 (40, 40)。
        let hit = hit_json(0, 41.0, 41.0);
        assert_eq!(hit, "{\"location\":{\"page\":0,\"line\":0,\"byte\":0}}");
        assert_eq!(hit_json(99, 0.0, 0.0), "{\"location\":null}");
        assert!(first.rect.w > 0.0);

        // txt 里没有链接。
        assert_eq!(link_at_json(0, 41.0, 41.0), "{\"href\":null}");

        // 选区：前两个字节 → "he"，矩形只有一个，页内从 inset 起。
        let sel = selection_json(0, 0, 0, 0, 0, 2);
        assert!(sel.contains(",\"text\":\"he\""), "{sel}");
        assert!(sel.contains("\"rects\":[{\"page\":0,"), "{sel}");
        assert!(
            sel.contains("\"start\":{\"page\":0,\"line\":0,\"byte\":0}"),
            "{sel}"
        );

        // 反向选区先 normalize。
        let reversed = selection_json(0, 0, 2, 0, 0, 0);
        assert!(reversed.contains(",\"text\":\"he\""), "{reversed}");
        assert!(
            reversed.contains("\"start\":{\"page\":0,\"line\":0,\"byte\":0}"),
            "{reversed}"
        );

        // 进度：行首 0，最后一行末尾 1。
        assert_eq!(progress_json(0, 0, 0), "{\"progress\":0}");
        let last = ir.runs.lines.len() as u32 - 1;
        let byte = ir.runs.lines[last as usize].src.len;
        assert_eq!(progress_json(0, last, byte), "{\"progress\":1}");
        // 越界进度不 panic，夹到 1。
        assert_eq!(progress_json(9, 99, 99), "{\"progress\":1}");

        // TXT 走同一条 BOOK/LAST 链路：书名、目录、逐章排版都可用。
        let opened = open_text("第一章 一\n\n正文。\n\n第二章 二\n\n正文。\n", "demo.txt");
        assert!(opened.contains("\"title\":\"demo\""), "{opened}");
        assert!(opened.contains("\"spine\":["), "{opened}");
        assert!(toc_json().contains("第二章"), "{}", toc_json());
        let chapter = layout_cached(1, 520.0, 300.0, 40.0);
        assert!(!chapter.is_empty());
        let ir: LayoutResult = ebook_ir::decode(&chapter).unwrap();
        assert_eq!(ir.spine, 1);
        assert_eq!(ir.runs.lines[0].anchor.as_ref().unwrap().spine, 1);

        // 全文搜索：两章各有一条“正文”，命中带章名与可跳转的 node/offset。
        let found = search_json("正文", 10);
        assert!(found.contains("\"hits\":["), "{found}");
        assert!(found.contains("\"spine\":0"), "{found}");
        assert!(found.contains("\"spine\":1"), "{found}");
        assert!(found.contains("\"chapter\":\"第一章 一\""), "{found}");
        assert!(!found.contains("\"truncated\":true"), "{found}");
        assert_eq!(
            search_json("不存在的词", 10),
            "{\"hits\":[],\"truncated\":false,\"scanned\":2,\"skipped\":0}"
        );
        assert_eq!(search_locate(u32::MAX, 0), u32::MAX);

        // 四边独立页边距：覆盖式工具栏靠它让正文避开顶栏/底栏。
        let inset = layout_cached_insets(0, 520.0, 300.0, 30.0, 40.0, 80.0, 50.0, 1.0);
        let ir: LayoutResult = ebook_ir::decode(&inset).unwrap();
        let page = &ir.pages[0];
        assert_eq!(page.inset.left, 30.0);
        assert_eq!(page.inset.right, 40.0);
        assert_eq!(page.inset.top, 80.0);
        assert_eq!(page.inset.bottom, 50.0);
        assert_eq!(page.box_size().w, 520.0);
        assert_eq!(page.box_size().h, 300.0);

        // 行距/字距/对齐是独立的排版选项，不乘进字号缩放。
        let styled = layout_cached_style(0, 520.0, 300.0, 30.0, 40.0, 80.0, 50.0, 1.0, 1.8, 1.0, 2);
        let ir: LayoutResult = ebook_ir::decode(&styled).unwrap();
        let page = &ir.pages[0];
        assert_eq!(page.box_size().w, 520.0);
        assert_eq!(page.box_size().h, 300.0);

        // 换书必须回收上一本借给 `EpubReader<'static>` 的字节：
        // `open_book` 以前是 `Box::leak`，每开一本泄漏一本。
        let demo =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo.epub");
        if let Ok(bytes) = std::fs::read(&demo) {
            let opened = open_book(&bytes);
            assert!(!opened.contains("\"error\""), "{opened}");
            assert!(
                BOOK_BYTES.lock().unwrap().is_some(),
                "成功开书后必须登记字节所有权"
            );
            // 再开一次：旧块被回收，新块重新登记，不叠加。
            let _ = open_book(&bytes);
            assert!(BOOK_BYTES.lock().unwrap().is_some());
            // 换成 txt：epub 字节必须立刻释放。
            let _ = open_text("hello", "a.txt");
            assert!(
                BOOK_BYTES.lock().unwrap().is_none(),
                "打开 txt 后仍持有 epub 字节"
            );
            close_book();
            assert!(BOOK.lock().unwrap().is_none());
        }
    }
}
