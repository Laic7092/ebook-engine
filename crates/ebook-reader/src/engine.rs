//! 一个进程里可以有多个 `Engine`，每个引擎一本会话。
//!
//! 与 `ebook-web` 的区别只有"状态归谁"：
//! - 这里的状态在结构体里，`&mut self` 就是全部同步；
//! - 书字节由引擎自己持有（`Box<[u8]>`），不再 `Box::leak`；
//! - 返回类型是结构体，不是 JSON 字符串。
//!
//! 排版本身仍然是纯函数（`Layout` 不碰 I/O、时钟、随机数），所以同一份书
//! + 同样的设置在任何平台上都必须给出逐字节相同的 IR。

use ebook_font::FontBook;
use ebook_format::epub::EpubReader;
use ebook_format::txt::TxtReader;
use ebook_format::{Book, ChapterAnchor, FormatError, LinkTarget, TocEntry};
use ebook_ir::{
    EdgesF, FontId, GlyphBitmap, ImageData, ImageId, LayoutResult, Location, PointF, Resources,
    Rgba8, StyledDoc,
};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, Reader, StubMeasurer};
use ebook_raster::{RgbaSurface, Tone};
use ebook_render::ScaledSurface;

use crate::types::{Bitmap, EngineError, Limits, PageBox, SelectionResult, Style, Viewport};

/// 最近一次排版的产物。交互查询（命中/选区/进度/锚点）只认这一份。
struct LastLayout {
    spine: usize,
    ir: LayoutResult,
    doc: StyledDoc,
    anchors: Vec<ChapterAnchor>,
}

impl LastLayout {
    fn reader(&self) -> Reader<'_> {
        Reader::new(&self.ir)
    }
}

/// 已打开的书。两种格式共用排版、字体、交互查询，只在"章从哪来、资产怎么取、
/// 链接怎么解析"上分叉。
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

    fn chapter(&self, index: usize) -> Result<(StyledDoc, Vec<ChapterAnchor>), FormatError> {
        match self {
            Backend::Epub(reader) => reader
                .chapter_with_anchors(index)
                .map(|(doc, _links, anchors)| (doc, anchors)),
            Backend::Txt(reader) => reader
                .chapter_with_anchors(index)
                .map(|(doc, _links, anchors)| (doc, anchors)),
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

/// 一本书的会话：书目、字体、最近一次排版、图片缓存。
pub struct Engine {
    // 字段顺序即析构顺序：`backend` 借用了 `bytes`，必须先释放。
    backend: Backend,
    /// `EpubReader<'static>` 的借用来源。
    ///
    /// 堆上的字节不会因为 `Box` 本身搬家而移动，所以这里拿到的 `'static`
    /// 切片在 `Engine` 存活期间始终有效；析构顺序见上面的注释。
    bytes: Box<[u8]>,
    name: String,
    font: Option<FontBook>,
    last: Option<LastLayout>,
    last_error: Option<String>,
    font_error: Option<String>,
    images: ImageCache,
    limits: Limits,
}

impl Engine {
    /// 按魔数识别格式：ZIP 走 EPUB，其余按文本解码（UTF-8/UTF-16/GBK/Big5…）。
    ///
    /// 书字节由引擎持有，宿主可以立刻丢掉自己那份。换书直接 `drop` 这个
    /// `Engine`，不会像 web 端那样把整本书泄漏到进程结束。
    pub fn open(bytes: Vec<u8>, name: &str) -> Result<Self, FormatError> {
        let boxed = bytes.into_boxed_slice();
        // SAFETY: `boxed` 是堆分配，移动 `Box` 不会移动堆上的字节；切片只在
        // `Engine` 存活期间使用，`bytes` 的析构在 `backend` 之后（字段顺序）。
        let raw: &'static [u8] = unsafe { std::slice::from_raw_parts(boxed.as_ptr(), boxed.len()) };

        let backend = if boxed.starts_with(b"PK") {
            Backend::Epub(EpubReader::open(raw)?)
        } else {
            let text = ebook_format::decode_text(&boxed);
            Backend::Txt(TxtReader::from_text(&text, Some(name)))
        };

        Ok(Self {
            backend,
            bytes: boxed,
            name: name.to_string(),
            font: None,
            last: None,
            last_error: None,
            font_error: None,
            images: ImageCache::default(),
            limits: Limits::default(),
        })
    }

    /// 资源上限：native 与 wasm 的预算不同，由宿主决定。
    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 书的原始字节（宿主可能要算 hash 或取封面）。
    pub fn source(&self) -> &[u8] {
        &self.bytes
    }

    pub fn book(&self) -> &Book {
        self.backend.book()
    }

    pub fn toc(&self) -> &[TocEntry] {
        self.backend.book().toc()
    }

    pub fn chapter_count(&self) -> usize {
        self.backend.book().spine.len()
    }

    /// 最近一次排版失败的原因，给宿主显示用；成功排版后清空。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn font_error(&self) -> Option<&str> {
        self.font_error.as_deref()
    }

    /// 排版一章。幂等：同样的 `(spine, viewport, style)` 给出同样的页。
    pub fn layout(
        &mut self,
        spine: usize,
        viewport: Viewport,
        style: Style,
    ) -> Result<(), EngineError> {
        let (mut doc, anchors) = self.backend.chapter(spine).map_err(|error| {
            let message = format!("{error:?}");
            self.last_error = Some(message.clone());
            EngineError::Chapter(message)
        })?;
        style.apply(&mut doc);
        self.last_error = None;

        // 语言是排版入参：要在 shape 之前注入，`locl` 之类才会生效。
        if let Some(font) = self.font.as_mut() {
            font.set_language(doc.meta.lang.as_deref());
        }
        let ir = {
            let env = match self.font.as_ref() {
                Some(font) => LayoutEnv {
                    measurer: font,
                    resources: &NoImageSizes,
                },
                None => LayoutEnv {
                    measurer: &StubMeasurer,
                    resources: &NoImageSizes,
                },
            };
            let constraint =
                PageConstraint::new(viewport.width, viewport.height).with_margin(viewport.insets);
            Layout::new().layout(&doc, &env, &constraint)
        };

        self.last = Some(LastLayout {
            spine,
            ir,
            doc,
            anchors,
        });
        Ok(())
    }

    /// 最近一次排版的 IR。适配层拿它编码成 postcard / 别的形状。
    pub fn ir(&self) -> Option<&LayoutResult> {
        self.last.as_ref().map(|last| &last.ir)
    }

    pub fn spine(&self) -> Option<usize> {
        self.last.as_ref().map(|last| last.spine)
    }

    pub fn page_count(&self) -> u32 {
        self.last
            .as_ref()
            .map(|last| last.ir.pages.len() as u32)
            .unwrap_or(0)
    }

    /// 页级元数据：几何、行区间、本章进度。宿主翻页/画进度条只需要这些。
    pub fn pages(&self) -> Vec<PageBox> {
        let Some(last) = self.last.as_ref() else {
            return Vec::new();
        };
        let reader = last.reader();
        last.ir
            .pages
            .iter()
            .map(|page| PageBox {
                index: page.index,
                width: page.size.w,
                height: page.size.h,
                inset: page.inset,
                line_from: page.lines.start,
                line_len: page.lines.len,
                progress: reader.progress(Location {
                    page: page.index,
                    line: page.lines.start,
                    byte: 0,
                }),
            })
            .collect()
    }

    /// 光栅化一页。`scale` 是设备像素比（1..4），`ink`/`paper` 是
    /// `0xRRGGBBAA`：`ink` 为 0 表示不做墨色修正，`paper` 为 0 表示白纸。
    pub fn bitmap(
        &mut self,
        page: u32,
        scale: f32,
        ink: u32,
        paper: u32,
        bold: bool,
    ) -> Result<Bitmap, EngineError> {
        let limits = self.limits;
        // 分开借：`font`/`images` 可变，`last`/`backend` 不可变，字段互不重叠。
        let Self {
            backend,
            font,
            last,
            images,
            ..
        } = self;
        let last = last.as_ref().ok_or(EngineError::NoLayout)?;
        let info = last
            .ir
            .page(page)
            .ok_or(EngineError::PageOutOfRange(page))?;

        let scale = if scale.is_finite() && scale > 0.0 {
            scale.clamp(0.25, 4.0)
        } else {
            1.0
        };
        let box_size = info.box_size();
        let width = (box_size.w * scale).round().max(1.0) as u32;
        let height = (box_size.h * scale).round().max(1.0) as u32;
        if (width as u64) * (height as u64) > limits.max_page_pixels {
            return Err(EngineError::TooLarge { width, height });
        }

        let mut surface = RgbaSurface::new(width, height);
        if ink != 0 {
            surface.set_tone(Some(Tone::ink(unpack_rgba(ink))));
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
                book: backend,
                images: &last.doc.images,
                srcs: Vec::new(),
                cache: images,
                budget: limits.image_cache_bytes,
            };
            let mut scaled = ScaledSurface::new(&mut surface, scale);
            if ebook_render::paint(&last.ir, page, &mut scaled, &mut res).is_err() {
                return Err(EngineError::Render(format!("page {page}")));
            }
        }

        Ok(Bitmap {
            width,
            height,
            rgba: surface.to_rgba(),
        })
    }

    /// 页内点坐标 → 原文位置。
    pub fn hit(&self, page: u32, x: f32, y: f32) -> Option<Location> {
        let last = self.last.as_ref()?;
        last.reader().hit(page, PointF::new(x, y))
    }

    /// 页内点坐标 → 链接地址（书内或外部）。
    pub fn link_at(&self, page: u32, x: f32, y: f32) -> Option<&str> {
        let last = self.last.as_ref()?;
        // 借的是 `last.ir`，生命周期跟着 `&self`，不是 `Reader` 这个临时值。
        let reader = Reader::new(&last.ir);
        reader.link_at(page, PointF::new(x, y))
    }

    /// 两个位置 → 选区（规范化端点 + 页内矩形 + 原文）。
    pub fn selection(&self, a: Location, b: Location) -> Option<SelectionResult> {
        let last = self.last.as_ref()?;
        let reader = last.reader();
        let selection = reader.selection(a, b);
        let (start, end) = selection.normalize();
        Some(SelectionResult {
            start,
            end,
            rects: reader.selection_rects(&selection),
            text: reader.selection_text(&last.doc, &selection),
        })
    }

    /// 本章进度 0..=1。
    pub fn progress(&self, location: Location) -> Option<f32> {
        Some(self.last.as_ref()?.reader().progress(location))
    }

    /// 本章里 `#fragment` 对应的行号；没有这个锚点时返回 `None`。
    pub fn fragment_line(&self, fragment: &str) -> Option<u32> {
        let last = self.last.as_ref()?;
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
    }

    /// 在**当前章**上下文里解析书内链接。
    pub fn resolve_link(&self, href: &str) -> Option<LinkTarget> {
        let spine = self.spine()?;
        self.backend.resolve_link_from(spine, href)
    }

    /// 注册一个字体（sfnt/woff/woff2）。
    pub fn register_font(&mut self, bytes: Vec<u8>) -> Result<FontId, String> {
        let font = self.font.get_or_insert_with(FontBook::new);
        match font.try_add(bytes) {
            Ok(id) => {
                self.font_error = None;
                Ok(id)
            }
            Err(error) => {
                self.font_error = Some(error.to_string());
                Err(error.to_string())
            }
        }
    }

    /// 把书里第 `index` 个内嵌字体按 `@font-face` 的 family/weight/style 装进来。
    pub fn register_font_face(&mut self, index: usize) -> Result<FontId, String> {
        let Some(face) = self.backend.book().fonts().get(index).cloned() else {
            let message = format!("没有第 {index} 个内嵌字体");
            self.font_error = Some(message.clone());
            return Err(message);
        };
        let font = self.font.get_or_insert_with(FontBook::new);
        match font.try_add_styled(&face.family, face.weight, face.style, face.bytes) {
            Ok(id) => {
                self.font_error = None;
                Ok(id)
            }
            Err(error) => {
                let message = format!("{}: {error}", face.href);
                self.font_error = Some(message.clone());
                Err(message)
            }
        }
    }

    pub fn font_count(&self) -> u32 {
        self.font
            .as_ref()
            .map(|font| font.len() as u32)
            .unwrap_or(0)
    }

    /// 书里声明了多少个 `@font-face`。
    pub fn font_face_count(&self) -> u32 {
        self.backend.book().fonts().len() as u32
    }

    /// 当前书第一章正文的基准字号。宿主拿它把"用户选的 px"换算成缩放比例。
    pub fn base_font_size(&self) -> f32 {
        let Ok((doc, _)) = self.backend.chapter(0) else {
            return 16.0;
        };
        // 正文通常不是根节点的字号：取文本节点里最常见的字号当基准。
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
}

fn unpack_rgba(value: u32) -> Rgba8 {
    Rgba8::new(
        (value >> 24) as u8,
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    )
}

/// 一页的资源视图：字形来自 `FontBook`，图片来自当前书的归档。
struct PageResources<'a> {
    font: Option<&'a mut FontBook>,
    bold: bool,
    book: &'a Backend,
    images: &'a [ebook_ir::ImageMeta],
    /// 本页用到的 `id → src`，避免每画一次都重新取元信息。
    srcs: Vec<(ImageId, Option<String>)>,
    cache: &'a mut ImageCache,
    budget: usize,
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
    fn glyph_alpha(&mut self, font: FontId, gid: u16, px: f32) -> Option<GlyphBitmap> {
        self.glyph_alpha_at(font, gid, px, 0.0, 0.0)
    }

    fn glyph_alpha_at(
        &mut self,
        font: FontId,
        gid: u16,
        px: f32,
        dx: f32,
        dy: f32,
    ) -> Option<GlyphBitmap> {
        let book = self.font.as_mut()?;
        // 全局加粗只影响取哪张 face，不改 IR：同一份 IR 可以按开关重画。
        let face = if self.bold {
            book.bold_face(font)
        } else {
            font
        };
        book.rasterize_at(face, gid, px, dx, dy)
    }

    fn image(&mut self, id: ImageId) -> Option<ImageData> {
        let src = self.src_of(id)?;
        if let Some(hit) = self.cache.get(&src) {
            return Some(hit);
        }
        let decoded = self
            .book
            .asset(&src)
            .and_then(|(bytes, _)| ebook_format::image::decode_image(bytes));
        let hit = decoded.clone();
        self.cache.insert(src, decoded, self.budget);
        hit
    }
}

/// 解码后的图片按归档路径缓存。图片字节本身在书里，这里只留 RGBA。
///
/// 用 `Vec` 而不是哈希表：一页涉及的图很少，线性查找更省内存，
/// 也与"IR 里禁止哈希容器"的取向一致。
#[derive(Default)]
struct ImageCache {
    entries: Vec<(String, Option<ImageData>)>,
    bytes: usize,
}

impl ImageCache {
    fn get(&mut self, src: &str) -> Option<ImageData> {
        self.entries
            .iter()
            .find(|(cached, _)| cached == src)
            .and_then(|(_, hit)| hit.clone())
    }

    fn insert(&mut self, src: String, data: Option<ImageData>, budget: usize) {
        if let Some(image) = data.as_ref() {
            self.bytes += image.rgba.len();
        }
        self.entries.push((src, data));
        // 超预算就丢最旧的条目，直到回到预算内（至少留一条，避免反复解码同一张）。
        while self.bytes > budget && self.entries.len() > 1 {
            let (_, dropped) = self.entries.remove(0);
            if let Some(image) = dropped {
                self.bytes = self.bytes.saturating_sub(image.rgba.len());
            }
        }
    }
}

/// 四边内边距的便捷构造。
pub fn insets(top: f32, right: f32, bottom: f32, left: f32) -> EdgesF {
    EdgesF {
        top,
        right,
        bottom,
        left,
    }
}
