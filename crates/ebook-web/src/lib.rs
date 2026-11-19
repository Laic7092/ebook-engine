pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod glue;

use std::fmt::Write as _;
use std::sync::Mutex;

use ebook_font::FontBook;
use ebook_format::epub::EpubReader;
use ebook_format::txt::TxtReader;
use ebook_format::{Book, ChapterAnchor, ChapterLinks, FormatError, LinkTarget};
use ebook_ir::{codec, EdgesF, FontId, LayoutResult, Location, PointF, StyledDoc};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, Reader, StubMeasurer};

static BOOK: Mutex<Option<Backend>> = Mutex::new(None);
static FONT: Mutex<Option<FontBook>> = Mutex::new(None);
/// 最近一次排版的 IR 与原文。交互查询（命中/选区/进度）只认这一份，
/// 与 JS 侧当前显示的章节保持同步；排版层本身仍然无状态。
static LAST: Mutex<Option<LastLayout>> = Mutex::new(None);
/// 最近一次排版失败的原因。`eb_layout` 失败时只能返回空 `Vec`，JS 侧靠它
/// 把“排版失败”变成一个具体错误，而不是让用户对着空页面猜。
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

pub fn last_error() -> String {
    LAST_ERROR.lock().unwrap().clone().unwrap_or_default()
}

struct LastLayout {
    ir: LayoutResult,
    doc: StyledDoc,
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

    fn asset<'a>(&'a self, href: &str) -> Option<(&'a [u8], &'a str)> {
        match self {
            Backend::Epub(reader) => reader.asset(href),
            Backend::Txt(reader) => reader.asset(href),
        }
    }

    fn resolve_link_from(&self, spine: usize, href: &str) -> Option<LinkTarget> {
        match self {
            Backend::Epub(reader) => reader.resolve_link_from(spine, href),
            Backend::Txt(reader) => reader.resolve_link_from(spine, href),
        }
    }
}

pub fn layout_text(text: &str, width: f32, height: f32, margin: f32) -> Vec<u8> {
    let doc = ebook_format::txt::build(text.as_bytes());
    layout_doc(&doc, width, height, margin)
}

pub fn open_book(bytes: &[u8]) -> String {
    let leaked: &'static [u8] = Box::leak(bytes.to_vec().into_boxed_slice());
    match EpubReader::open(leaked) {
        Ok(reader) => install(Backend::Epub(reader)),
        Err(error) => error_json(&format!("open: {error:?}")),
    }
}

/// 打开已经由宿主解码好的纯文本。章节在格式层按标题行切分，
/// TOC / spine / 交互查询因此和 EPUB 完全同构。
pub fn open_text(text: &str, name: &str) -> String {
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
    *BOOK.lock().unwrap() = Some(backend);
    json
}

pub fn register_font(bytes: &[u8]) -> u32 {
    let mut slot = FONT.lock().unwrap();
    let font = slot.get_or_insert_with(FontBook::new);
    match font.add(bytes.to_vec()) {
        Some(id) => id.0 as u32,
        None => u32::MAX,
    }
}

pub fn reset_fonts() {
    *FONT.lock().unwrap() = None;
}

/// 宿主按 `GlyphRun` 的 glyph id 取回光栅位图，自己合成页面。
///
/// 返回 8 字节小端头 + `w*h` 的 alpha：
/// `[w:u16, h:u16, left:i16, top:i16, alpha...]`。
/// `left/top` 是相对整像素落点的偏移，`top` 为负；没有位图时返回空数组。
/// 合成粗体/斜体在 `FontBook` 内部完成，宿主不需要再猜浏览器会不会合成。
pub fn glyph_bitmap(font: u32, gid: u32, px: f32, dx: f32, dy: f32) -> Vec<u8> {
    let slot = FONT.lock().unwrap();
    let Some(book) = slot.as_ref() else {
        return Vec::new();
    };
    let Some(bitmap) = book.rasterize_at(FontId(font as u16), gid as u16, px, dx, dy) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(8 + bitmap.alpha.len());
    out.extend_from_slice(&bitmap.w.to_le_bytes());
    out.extend_from_slice(&bitmap.h.to_le_bytes());
    out.extend_from_slice(&bitmap.left.to_le_bytes());
    out.extend_from_slice(&bitmap.top.to_le_bytes());
    out.extend_from_slice(&bitmap.alpha);
    out
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
    let face = {
        let books = BOOK.lock().unwrap();
        books
            .as_ref()
            .and_then(|reader| reader.book().fonts().get(index).cloned())
    };
    let Some(face) = face else {
        return u32::MAX;
    };
    let mut slot = FONT.lock().unwrap();
    let font = slot.get_or_insert_with(FontBook::new);
    match font.add_styled(&face.family, face.weight, face.style, face.bytes) {
        Some(id) => id.0 as u32,
        None => u32::MAX,
    }
}

/// 内嵌字体的原始字节，用于浏览器 `new FontFace(family, bytes)`。
pub fn load_font_face_bytes(index: usize) -> Vec<u8> {
    let books = BOOK.lock().unwrap();
    books
        .as_ref()
        .and_then(|reader| reader.book().fonts().get(index))
        .map(|face| face.bytes.clone())
        .unwrap_or_default()
}

/// 按归档路径取资产字节（图片、字体……）。找不到返回空数组，JS 侧按长度判空。
pub fn asset_bytes(href: &str) -> Vec<u8> {
    let books = BOOK.lock().unwrap();
    books
        .as_ref()
        .and_then(|reader| reader.asset(href))
        .map(|(bytes, _)| bytes.to_vec())
        .unwrap_or_default()
}

/// 本章图片表：`[{"id":0,"href":"OEBPS/pics/a.png","mime":"image/png"}]`。
/// `id` 是最近一次排版的 `StyledDoc::images` 下标，也就是 `Op::Image.id`。
pub fn chapter_images_json() -> String {
    with_last(|last| {
        let mut out = String::from("[");
        for (id, image) in last.doc.images.iter().enumerate() {
            if id > 0 {
                out.push(',');
            }
            let _ = write!(out, "{{\"id\":{id},\"href\":");
            push_json_opt(&mut out, image.src.as_deref());
            out.push_str(",\"mime\":");
            push_json_opt(&mut out, image.mime.as_deref());
            out.push('}');
        }
        out.push(']');
        out
    })
    .unwrap_or_else(|| String::from("[]"))
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

/// 本章的 `id → Anchor`：`{"anchor":{"spine":0,"path":[3,9],"offset":0}}`
/// 或 `{"anchor":null}`。切章之后要重新查。
pub fn anchor_json(fragment: &str) -> String {
    let anchor = with_last(|last| {
        last.anchors
            .iter()
            .find(|entry| entry.id == fragment)
            .map(|entry| entry.anchor.clone())
    })
    .flatten();
    let mut out = String::from("{\"anchor\":");
    match anchor {
        Some(anchor) => {
            let _ = write!(
                out,
                "{{\"spine\":{},\"path\":[{}],\"offset\":{}}}",
                anchor.spine,
                anchor
                    .path
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                anchor.offset
            );
        }
        None => out.push_str("null"),
    }
    out.push('}');
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

pub fn font_desc(id: u32) -> String {
    let fonts = FONT.lock().unwrap();
    let Some(book) = fonts.as_ref() else {
        return String::from("{\"family\":null}");
    };
    let font = ebook_ir::FontId(id as u16);
    match book.desc(font) {
        Some(desc) => {
            let mut out = String::from("{\"family\":");
            push_json_str(&mut out, &desc.family);
            out.push_str(",\"weight\":");
            let _ = write!(out, "{}", desc.weight);
            out.push_str(",\"style\":");
            push_json_str(&mut out, font_style_name(desc.style));
            out.push_str(",\"bold\":");
            out.push_str(if desc.weight >= 600 || book.is_synthetic_bold(font) {
                "true"
            } else {
                "false"
            });
            out.push_str(",\"italic\":");
            out.push_str(
                if !matches!(desc.style, ebook_ir::FontStyle::Normal)
                    || book.is_synthetic_italic(font)
                {
                    "true"
                } else {
                    "false"
                },
            );
            out.push('}');
            out
        }
        None => String::from("{\"family\":null}"),
    }
}

fn font_style_name(style: ebook_ir::FontStyle) -> &'static str {
    match style {
        ebook_ir::FontStyle::Normal => "normal",
        ebook_ir::FontStyle::Italic => "italic",
        ebook_ir::FontStyle::Oblique => "oblique",
    }
}

/// 网页“字号”不是缩放整页，而是只改文档里的字号/行高；页面约束、分页
/// 几何都不变，所以容器不会跟着字号一起变。
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

/// 把 worker 排版好的 IR 装回主线程。命中、选区、链接、进度都读 `LAST`，
/// 主线程不需要再跑一遍 `Layout`；只重建 `StyledDoc` 取回选区原文与图片表。
/// 返回空串表示成功，否则是给宿主显示的错误。
pub fn adopt_layout(spine: usize, bytes: &[u8]) -> String {
    let ir: LayoutResult = match codec::decode(bytes) {
        Ok(ir) => ir,
        Err(error) => return format!("decode: {error:?}"),
    };
    let books = BOOK.lock().unwrap();
    let Some(reader) = books.as_ref() else {
        return String::from("no book");
    };
    match reader.chapter_with_anchors(spine) {
        Ok((doc, _, anchors)) => {
            *LAST.lock().unwrap() = Some(LastLayout { ir, doc, anchors });
            String::new()
        }
        Err(error) => format!("chapter: {error:?}"),
    }
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
    }
}
