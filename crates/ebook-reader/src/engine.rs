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
use ebook_layout::{
    Layout, LayoutEnv, LayoutOptions, Mode, NoImageSizes, PageConstraint, Reader, StubMeasurer,
};
use ebook_raster::{RgbaSurface, Tone};
use ebook_render::ScaledSurface;

use crate::types::{
    Bitmap, EngineError, Limits, PageBox, ScrollChapter, ScrollLoc, SelectionResult, Style,
    Viewport,
};

/// 最近一次排版的产物。交互查询（命中/选区/进度/锚点）只认这一份。
struct LastLayout {
    spine: usize,
    mode: Mode,
    viewport: Viewport,
    style: Style,
    font_rev: u32,
    limits: Limits,
    ir: LayoutResult,
    doc: StyledDoc,
    anchors: Vec<ChapterAnchor>,
}

impl LastLayout {
    fn reader(&self) -> Reader<'_> {
        Reader::new(&self.ir)
    }

    /// 这份排版结果是不是这些输入排出来的。
    fn matches(
        &self,
        spine: usize,
        mode: Mode,
        viewport: Viewport,
        style: Style,
        font_rev: u32,
        limits: Limits,
    ) -> bool {
        self.spine == spine
            && self.mode == mode
            && self.viewport == viewport
            && self.style == style
            && self.font_rev == font_rev
            && self.limits == limits
    }
}

/// 滚动会话里缓存几章的排版结果：当前章加左右各一章，跨章瓦片够用。
const CHAPTER_CACHE: usize = 4;

/// 连续滚动会话：跨章的高度表 + 已排过的章。
///
/// 宿主只给视口和样式，章节起点（`tops`）由引擎算：需要某章高度时现场排版
/// 并记住，所以第一次滚到某处可能触发一次排版，之后只读表。高度是**懒**
/// 的：`scroll_chapter(i)` 只保证 `0..=i` 量过，`scroll_total_height()` 才量全书。
struct ScrollSession {
    viewport: Viewport,
    style: Style,
    font_rev: u32,
    /// 每章内容高度；`None` 还没排。
    heights: Vec<Option<f32>>,
    /// 每章排版失败的原因（失败章按 0 高计，不阻塞后面的章）。
    errors: Vec<Option<String>>,
    /// `tops[i]` 是第 i 章在全书流坐标里的起点；对 `i <= measured` 有效。
    tops: Vec<f32>,
    /// 已经连续量过的高度的章数：`tops[measured]` 是已量部分的末尾。
    measured: usize,
}

impl ScrollSession {
    fn new(viewport: Viewport, style: Style, font_rev: u32, count: usize) -> Self {
        Self {
            viewport,
            style,
            font_rev,
            heights: vec![None; count],
            errors: vec![None; count],
            tops: vec![0.0; count + 1],
            measured: 0,
        }
    }

    fn count(&self) -> usize {
        self.heights.len()
    }

    fn matches(&self, viewport: Viewport, style: Style, font_rev: u32) -> bool {
        self.viewport == viewport && self.style == style && self.font_rev == font_rev
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
    /// 已经排过、暂时不在 `last` 里的章（滚动会话跨章用，LRU）。
    chapters: Vec<LastLayout>,
    /// 连续滚动会话；`None` 表示还没开始。
    scroll: Option<ScrollSession>,
    last_error: Option<String>,
    font_error: Option<String>,
    images: ImageCache,
    limits: Limits,
    /// 每次成功注册字体就加一；排版缓存靠它失效。
    font_rev: u32,
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
            chapters: Vec::new(),
            scroll: None,
            last_error: None,
            font_error: None,
            images: ImageCache::default(),
            limits: Limits::default(),
            font_rev: 0,
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

    /// 封面图片的字节与媒体类型（EPUB 的 `cover-image`，或 EPUB2 的
    /// `meta name="cover"`）；没有封面时 `None`。TXT 永远没有。
    pub fn cover(&self) -> Option<(&[u8], &str)> {
        let href = self.backend.book().cover_href.as_deref()?;
        self.backend.asset(href)
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
        self.layout_mode(spine, viewport, style, Mode::Paged)
    }

    /// 把一章排成**一整页滚动流**：`Page.origin` 为 0，`Page.size.h` 是内容
    /// 实际高度。`viewport.height` 不参与切页（它只决定视口宽高比，宿主取
    /// 窗口用 `bitmap_window`）；上下留白应由调用方传 0，左右留白保留。
    pub fn layout_scroll(
        &mut self,
        spine: usize,
        viewport: Viewport,
        style: Style,
    ) -> Result<(), EngineError> {
        self.layout_mode(spine, viewport, style, Mode::Scroll)
    }

    fn layout_mode(
        &mut self,
        spine: usize,
        viewport: Viewport,
        style: Style,
        mode: Mode,
    ) -> Result<(), EngineError> {
        self.focus(spine, mode, viewport, style)
    }

    /// 把 `spine` 章按这些输入排版的结果放进 `last`。
    ///
    /// 排版是纯函数：`last` 就是它、或者缓存里有同一份输入的结果时直接复用，
    /// 所以滚动一屏取多个瓦片、或来回滚到邻章，都只排一次。
    fn focus(
        &mut self,
        spine: usize,
        mode: Mode,
        viewport: Viewport,
        style: Style,
    ) -> Result<(), EngineError> {
        let font_rev = self.font_rev;
        let limits = self.limits;
        if self
            .last
            .as_ref()
            .is_some_and(|last| last.matches(spine, mode, viewport, style, font_rev, limits))
        {
            return Ok(());
        }
        if let Some(index) = self
            .chapters
            .iter()
            .position(|entry| entry.matches(spine, mode, viewport, style, font_rev, limits))
        {
            let layout = self.chapters.remove(index);
            if let Some(previous) = self.last.replace(layout) {
                self.remember(previous);
            }
            return Ok(());
        }
        let layout = self.build_layout(spine, mode, viewport, style)?;
        if let Some(previous) = self.last.replace(layout) {
            self.remember(previous);
        }
        Ok(())
    }

    fn remember(&mut self, layout: LastLayout) {
        self.chapters.push(layout);
        if self.chapters.len() > CHAPTER_CACHE {
            self.chapters.remove(0);
        }
    }

    fn build_layout(
        &mut self,
        spine: usize,
        mode: Mode,
        viewport: Viewport,
        style: Style,
    ) -> Result<LastLayout, EngineError> {
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
            let options = LayoutOptions {
                mode,
                ..LayoutOptions::default()
            };
            Layout::new().layout_with(&doc, &env, &constraint, &options)
        };

        Ok(LastLayout {
            spine,
            mode,
            viewport,
            style,
            font_rev: self.font_rev,
            limits: self.limits,
            ir,
            doc,
            anchors,
        })
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

    /// 单独一页的元数据。等于 `pages()` 按下标取，但没有整表分配：
    /// 翻页、画瓦片这类逐页调用不必扫全章。
    pub fn page(&self, index: u32) -> Option<PageBox> {
        let last = self.last.as_ref()?;
        // `Page.index` 是唯一权威；下标对不上就是没有这页。
        let page = last.ir.pages.get(index as usize)?;
        if page.index != index {
            return None;
        }
        Some(PageBox {
            index: page.index,
            width: page.size.w,
            height: page.size.h,
            inset: page.inset,
            line_from: page.lines.start,
            line_len: page.lines.len,
            progress: last.reader().progress(Location {
                page: page.index,
                line: page.lines.start,
                byte: 0,
            }),
        })
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

    /// 滚动模式的内容高度（本章流坐标总长）；分页模式/没排版时 0。
    /// 分页模式请用 `pages()`——每页高度是视口内容盒，不是全书高度。
    pub fn content_height(&self) -> f32 {
        let Some(last) = self.last.as_ref() else {
            return 0.0;
        };
        if last.mode != Mode::Scroll {
            return 0.0;
        }
        last.ir.pages.first().map_or(0.0, |page| page.size.h)
    }

    /// 最近一次排版是不是滚动模式。
    pub fn is_scroll(&self) -> bool {
        self.last
            .as_ref()
            .is_some_and(|last| last.mode == Mode::Scroll)
    }

    // ------------------------------------------------------- 连续滚动

    /// 开始（或重设）一次**跨章连续滚动**会话。
    ///
    /// 此后用 [`Engine::scroll_slice`] 按全书流坐标取瓦片、[`Engine::scroll_hit`]
    /// 按流坐标命中、[`Engine::scroll_line_y`] / [`Engine::scroll_anchor_y`]
    /// 把位置换成该滚到哪。章节起点不用宿主自己算：引擎在需要时排版那一章并
    /// 记住高度（懒），宿主只需要把 `scroll_chapter(i)` 得到的表拿去做滚动容器。
    ///
    /// 上下留白会被置 0（滚动容器自己留，瓦片之间才没有空白带），左右保留。
    /// 同样的视口/样式/字体再调一次不会丢失已量到的高度。字体换了（
    /// `register_font`）之后必须重新开始：旧高度属于旧字体，引擎不会拿它凑合。
    pub fn scroll_begin(
        &mut self,
        mut viewport: Viewport,
        style: Style,
    ) -> Result<usize, EngineError> {
        let count = self.chapter_count();
        if count == 0 {
            return Err(EngineError::NoBook);
        }
        viewport.insets.top = 0.0;
        viewport.insets.bottom = 0.0;
        let keep = self
            .scroll
            .as_ref()
            .is_some_and(|session| session.matches(viewport, style, self.font_rev));
        if !keep {
            self.scroll = Some(ScrollSession::new(viewport, style, self.font_rev, count));
        }
        // 章节缓存可以跨会话复用；参数变了的条目留着只占内存。
        self.chapters.retain(|entry| {
            entry.mode == Mode::Scroll
                && entry.viewport == viewport
                && entry.style == style
                && entry.font_rev == self.font_rev
        });
        Ok(count)
    }

    /// 滚动会话里的一章：起点（全书流坐标）、高度、这一章排没排成功。
    ///
    /// 返回 `top` 之前会把 `0..=index` 的章都量过（只差没量的那几章），
    /// 所以宿主可以逐章取表、分摊到多帧里。
    pub fn scroll_chapter(&mut self, index: usize) -> Result<ScrollChapter, EngineError> {
        self.scroll_check()?;
        let count = self.chapter_count();
        if index >= count {
            return Err(EngineError::ChapterOutOfRange(index as u32));
        }
        self.scroll_measure_to(index)?;
        let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
        Ok(ScrollChapter {
            spine: index,
            top: session.tops[index],
            height: session.heights[index].unwrap_or(0.0),
            ok: session.errors[index].is_none(),
        })
    }

    /// 全书章节表。等同于逐章 `scroll_chapter`，代价是量完全书（首次）。
    pub fn scroll_chapters(&mut self) -> Result<Vec<ScrollChapter>, EngineError> {
        self.scroll_check()?;
        let count = self.chapter_count();
        if count > 0 {
            self.scroll_measure_to(count - 1)?;
        }
        (0..count).map(|index| self.scroll_chapter(index)).collect()
    }

    /// 全书流坐标总长（滚动容器的 contentSize）。首次调用会排完全书。
    pub fn scroll_total_height(&mut self) -> Result<f32, EngineError> {
        self.scroll_check()?;
        let count = self.chapter_count();
        if count > 0 {
            self.scroll_measure_to(count - 1)?;
        }
        Ok(self
            .scroll
            .as_ref()
            .map_or(0.0, |session| session.tops[count]))
    }

    /// 全书流坐标 `y` 落在哪一章：返回章下标与该章起点。
    pub fn scroll_spine_at(&mut self, y: f32) -> Result<Option<(usize, f32)>, EngineError> {
        self.scroll_check()?;
        let Some(index) = self.scroll_index_at(y)? else {
            return Ok(None);
        };
        let top = self
            .scroll
            .as_ref()
            .map_or(0.0, |session| session.tops[index]);
        Ok(Some((index, top)))
    }

    /// 光栅化整个滚动容器里的一段：`y`/`height` 是**全书流坐标**，跨章也画。
    ///
    /// 一种调用搞定一屏（或一枚瓦片）：引擎自己找出涉及的章、分别光栅化再拼
    /// 到同一张位图，章缝不会错行。内容之外的区域是纸色。
    #[allow(clippy::too_many_arguments)]
    pub fn scroll_slice(
        &mut self,
        y: f32,
        height: f32,
        scale: f32,
        ink: u32,
        paper: u32,
        bold: bool,
    ) -> Result<Bitmap, EngineError> {
        if !y.is_finite() || !height.is_finite() || height <= 0.0 {
            return Err(EngineError::Render(format!(
                "window {y}+{height} is not a box"
            )));
        }
        self.scroll_check()?;
        let scale = if scale.is_finite() && scale > 0.0 {
            scale.clamp(0.25, 4.0)
        } else {
            1.0
        };
        let viewport = self
            .scroll
            .as_ref()
            .map(|session| session.viewport)
            .ok_or(EngineError::NoScroll)?;
        let width = (viewport.width * scale).round().max(1.0) as u32;
        let rows = (height * scale).round().max(1.0) as u32;
        if (width as u64) * (rows as u64) > self.limits.max_page_pixels {
            return Err(EngineError::TooLarge {
                width,
                height: rows,
            });
        }
        // 先把整块铺成纸色：章节没覆盖到的行（章缝、末尾空白）就是纸。
        let empty = if paper == 0 {
            Rgba8::WHITE
        } else {
            unpack_rgba(paper)
        };
        let mut out = Bitmap {
            width,
            height: rows,
            rgba: Vec::with_capacity(width as usize * rows as usize * 4),
        };
        let pixel = [empty.r, empty.g, empty.b, empty.a];
        for _ in 0..(width as usize * rows as usize) {
            out.rgba.extend_from_slice(&pixel);
        }

        // 起点对齐设备像素：相邻两章的边界行由同一公式算出，不会重一行/漏一行。
        let y = ((y.max(0.0) * scale).round() / scale).max(0.0);
        let stride = width as usize * 4;
        let count = self.chapter_count();
        for index in 0..count {
            self.scroll_measure_to(index)?;
            let (top, chapter_height) = {
                let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
                (session.tops[index], session.heights[index].unwrap_or(0.0))
            };
            if top >= y + height {
                break;
            }
            let segment_start = top.max(y);
            let segment_end = (top + chapter_height).min(y + height);
            if segment_end <= segment_start {
                continue;
            }
            let row_from = (((segment_start - y) * scale).round() as i64).clamp(0, rows as i64);
            let row_to = (((segment_end - y) * scale).round() as i64).clamp(row_from, rows as i64);
            if row_to <= row_from {
                continue;
            }
            let local_top = segment_start - top;
            let local_height = (row_to - row_from) as f32 / scale;
            let style = self
                .scroll
                .as_ref()
                .map(|session| session.style)
                .ok_or(EngineError::NoScroll)?;
            self.focus(index, Mode::Scroll, viewport, style)?;
            let bitmap = self.bitmap_window(0, local_top, local_height, scale, ink, paper, bold)?;
            if bitmap.width != width {
                // 视口宽是同一份视口算出来的，宽度不该变；真变了宁可留纸色，
                // 也不要按错的跨距搬行。
                continue;
            }
            let copy = (bitmap.height as i64).min(row_to - row_from) as usize;
            let from = row_from as usize * stride;
            let bytes = copy * stride;
            if from + bytes <= out.rgba.len() && bitmap.rgba.len() >= bytes {
                out.rgba[from..from + bytes].copy_from_slice(&bitmap.rgba[..bytes]);
            }
            if top + chapter_height >= y + height {
                break;
            }
        }
        Ok(out)
    }

    /// 全书流坐标 → 原文位置（跨章）。`y` 落在章与章之间的空白时按该章末尾算。
    pub fn scroll_hit(&mut self, x: f32, y: f32) -> Result<Option<ScrollLoc>, EngineError> {
        self.scroll_check()?;
        let y = if y.is_finite() { y.max(0.0) } else { 0.0 };
        let Some(index) = self.scroll_index_at(y)? else {
            return Ok(None);
        };
        let (top, viewport, style) = {
            let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
            (session.tops[index], session.viewport, session.style)
        };
        self.focus(index, Mode::Scroll, viewport, style)?;
        let local = y - top;
        let Some(hit) = self.hit(0, x, local) else {
            return Ok(None);
        };
        let line_top = self.flow_line_y(0, hit.line).unwrap_or(local);
        Ok(Some(ScrollLoc {
            spine: index,
            line: hit.line,
            byte: hit.byte,
            y: top + line_top,
        }))
    }

    /// 行号 → 全书流坐标（行盒顶边）。滚动到某行就用它。
    pub fn scroll_line_y(&mut self, spine: usize, line: u32) -> Result<Option<f32>, EngineError> {
        let top = self.scroll_focus(spine)?;
        Ok(self.flow_line_y(0, line).map(|y| top + y))
    }

    /// `#fragment` 锚点 → 全书流坐标（行盒顶边）。目录/书内链接跳转用它；
    /// 这一章没有这个锚点时返回 `None`。
    pub fn scroll_anchor_y(
        &mut self,
        spine: usize,
        fragment: &str,
    ) -> Result<Option<f32>, EngineError> {
        let top = self.scroll_focus(spine)?;
        let Some(line) = self.fragment_line(fragment) else {
            return Ok(None);
        };
        Ok(self.flow_line_y(0, line).map(|y| top + y))
    }

    /// 全书流坐标 → 滚动进度 0..=1。首次调用会量完全书（滚动条本来就需要总高）。
    pub fn scroll_progress(&mut self, y: f32) -> Result<f32, EngineError> {
        let total = self.scroll_total_height()?;
        if total <= 0.0 || !y.is_finite() {
            return Ok(0.0);
        }
        Ok((y / total).clamp(0.0, 1.0))
    }

    /// 滚动会话是否还在（字体换过之后会失效）。
    pub fn has_scroll_session(&self) -> bool {
        self.scroll.is_some()
    }

    fn scroll_check(&mut self) -> Result<(), EngineError> {
        let Some(session) = self.scroll.as_ref() else {
            return Err(EngineError::NoScroll);
        };
        // 字体变了：高度表属于旧字体，不能凑合，丢掉让宿主重新 begin。
        if session.font_rev != self.font_rev {
            self.scroll = None;
            return Err(EngineError::NoScroll);
        }
        Ok(())
    }

    /// 量到第 `index` 章（含）：只排还没量过的那几章。
    fn scroll_measure_to(&mut self, index: usize) -> Result<(), EngineError> {
        self.scroll_check()?;
        loop {
            let (next, viewport, style) = {
                let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
                if session.measured > index || session.measured >= session.count() {
                    return Ok(());
                }
                (session.measured, session.viewport, session.style)
            };
            // 一章排不出来不该拖垮整本书：高度按 0 记，错误留给 `scroll_chapter`。
            let height = match self.focus(next, Mode::Scroll, viewport, style) {
                Ok(()) => self
                    .last
                    .as_ref()
                    .and_then(|last| last.ir.pages.first())
                    .map_or(0.0, |page| page.size.h),
                Err(error) => {
                    if let Some(session) = self.scroll.as_mut() {
                        session.errors[next] = Some(error.to_string());
                    }
                    0.0
                }
            };
            let Some(session) = self.scroll.as_mut() else {
                return Err(EngineError::NoScroll);
            };
            session.heights[next] = Some(height);
            session.tops[next + 1] = session.tops[next] + height;
            session.measured = next + 1;
        }
    }

    /// 流坐标 `y` 落在哪一章（必要时继续量后面的章）。
    fn scroll_index_at(&mut self, y: f32) -> Result<Option<usize>, EngineError> {
        self.scroll_check()?;
        let y = if y.is_finite() { y.max(0.0) } else { 0.0 };
        let count = self.chapter_count();
        for index in 0..count {
            self.scroll_measure_to(index)?;
            let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
            if y < session.tops[index + 1] {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    /// 排版/聚焦一章并返回它在全书流坐标里的起点。
    fn scroll_focus(&mut self, spine: usize) -> Result<f32, EngineError> {
        self.scroll_check()?;
        let count = self.chapter_count();
        if spine >= count {
            return Err(EngineError::ChapterOutOfRange(spine as u32));
        }
        self.scroll_measure_to(spine)?;
        let (top, viewport, style) = {
            let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
            (session.tops[spine], session.viewport, session.style)
        };
        self.focus(spine, Mode::Scroll, viewport, style)?;
        Ok(top)
    }

    /// 行盒在**页内流坐标**里的顶边（不含章起点）。
    fn flow_line_y(&self, page: u32, line: u32) -> Option<f32> {
        let last = self.last.as_ref()?;
        let page_ref = last.ir.page(page)?;
        let start = page_ref.lines.start;
        let end = start.checked_add(page_ref.lines.len)?;
        if line < start || line >= end {
            return None;
        }
        last.ir.line(line).map(|line| line.rect.y)
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
        let layout = self.last.take().ok_or(EngineError::NoLayout)?;
        let result = render_bitmap(
            &layout,
            &mut self.font,
            &self.backend,
            &mut self.images,
            self.limits,
            page,
            None,
            scale,
            ink,
            paper,
            bold,
        );
        self.last = Some(layout);
        result
    }

    /// 光栅化**流坐标窗口**：从 `top` 起、`height` 点高的一片。
    ///
    /// 滚动模式的一枚瓦片就是它：`top` 是瓦片在本章流坐标里的起点，
    /// surface 只有瓦片高，不会分配整章位图。宽度仍是整页宽。
    /// `height` 不要求是视口高；同一个函数也可画分页页面的一段（分页页的
    /// 内容 y 从 `page.origin` 起，宿主可先取 `pages()` 再换算）。
    ///
    /// 起点会对齐到设备像素栅格（偏差不超过半个设备像素）：相邻瓦片的字形
    /// 亚像素相位因此相同，滚动时字形光栅缓存才能命中。
    #[allow(clippy::too_many_arguments)]
    pub fn bitmap_window(
        &mut self,
        page: u32,
        top: f32,
        height: f32,
        scale: f32,
        ink: u32,
        paper: u32,
        bold: bool,
    ) -> Result<Bitmap, EngineError> {
        if !top.is_finite() || !height.is_finite() || height <= 0.0 {
            return Err(EngineError::Render(format!(
                "window {top}+{height} is not a box"
            )));
        }
        let layout = self.last.take().ok_or(EngineError::NoLayout)?;
        let result = render_bitmap(
            &layout,
            &mut self.font,
            &self.backend,
            &mut self.images,
            self.limits,
            page,
            Some((top.max(0.0), height)),
            scale,
            ink,
            paper,
            bold,
        );
        self.last = Some(layout);
        result
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
                self.font_rev = self.font_rev.wrapping_add(1);
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
                self.font_rev = self.font_rev.wrapping_add(1);
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

/// 光栅化一份已经排好的章。
///
/// 不碰 `Engine` 的其它状态，所以滚动会话能把章节从缓存里取出来画，
/// 而不要求它就是 `last`。
#[allow(clippy::too_many_arguments)]
fn render_bitmap(
    layout: &LastLayout,
    font: &mut Option<FontBook>,
    backend: &Backend,
    images: &mut ImageCache,
    limits: Limits,
    page: u32,
    window: Option<(f32, f32)>,
    scale: f32,
    ink: u32,
    paper: u32,
    bold: bool,
) -> Result<Bitmap, EngineError> {
    let info = layout
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
    let height = match window {
        Some((_, h)) => (h * scale).round().max(1.0) as u32,
        None => (box_size.h * scale).round().max(1.0) as u32,
    };
    if (width as u64) * (height as u64) > limits.max_page_pixels {
        return Err(EngineError::TooLarge { width, height });
    }
    let top = match window {
        // 对齐到设备像素栅格，字形相位就不随滚动位置变（见 `bitmap_window`）。
        Some((top, _)) => {
            let snapped = (top * scale).round() / scale;
            if snapped.is_finite() {
                snapped.max(0.0)
            } else {
                top
            }
        }
        None => 0.0,
    };

    let mut surface = RgbaSurface::new(width, height);
    if ink != 0 {
        // 墨色既是对比度的依据，也是字形的颜色：夜里是浅色字，不是黑字。
        surface.set_tone(Some(Tone::ink(unpack_rgba(ink))));
        surface.set_ink(Some(unpack_rgba(ink)));
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
            images: &layout.doc.images,
            srcs: Vec::new(),
            cache: images,
            budget: limits.image_cache_bytes,
        };
        let mut scaled = ScaledSurface::new(&mut surface, scale);
        let painted = match window {
            Some(_) => ebook_render::paint_window(&layout.ir, page, top, &mut scaled, &mut res),
            None => ebook_render::paint(&layout.ir, page, &mut scaled, &mut res),
        };
        if painted.is_err() {
            return Err(EngineError::Render(format!("page {page}")));
        }
    }

    Ok(Bitmap {
        width,
        height,
        rgba: surface.to_rgba(),
    })
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
