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
use ebook_format::txt::{StreamBook, TxtReader};
use ebook_format::{Book, ChapterAnchor, FormatError, LinkTarget, TocEntry};
use std::sync::Arc;

use ebook_ir::{
    Anchor, AnchorRange, BlockBox, EdgesF, FontId, GlyphBitmap, ImageData, ImageId, LayoutResult,
    Location, NodeId, PointF, Resources, Rgba8, SizeF, StyledDoc,
};
use ebook_layout::{
    BlockWindow, FlowCache, Layout, LayoutEnv, LayoutOptions, Mode, NoImageSizes, PageConstraint,
    Reader, StubMeasurer,
};
use ebook_raster::{RgbaSurface, Tone};
use ebook_render::ScaledSurface;
use ebook_search::{SearchCache, SearchOptions, SearchText};

use crate::heights::{estimate_height, index_key, HeightIndex};
use crate::types::{
    Bitmap, EngineError, Limits, Locator, LocatorRange, MemoryUsage, PageBox, ScrollChapter,
    ScrollLoc, ScrollMetrics, SearchHit, SearchResults, SelectionResult, Style, Viewport,
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
    /// 正文默认字色（按文本字节数取主导色）。主题墨色只换它。
    text_color: Rgba8,
    anchors: Vec<ChapterAnchor>,
    /// 这份 IR 是哪一块窗口排出来的；`None` 是整章。
    window: Option<BlockWindow>,
}

/// The line an anchor names: its own text when it has one, else the first
/// line below it (a `<p id>` sits above its text nodes), else the last line
/// that starts there (a byte offset inside one text node).
fn line_of_anchor(ir: &LayoutResult, anchor: &Anchor) -> Option<u32> {
    let mut fallback = None;
    for (index, line) in ir.runs.lines.iter().enumerate() {
        let Some(line_anchor) = line.anchor.as_ref() else {
            continue;
        };
        if line_anchor.spine != anchor.spine || !line_anchor.path.starts_with(&anchor.path) {
            continue;
        }
        if line_anchor.path.len() > anchor.path.len() {
            return Some(index as u32);
        }
        if anchor.offset >= line_anchor.offset && anchor.offset - line_anchor.offset <= line.src.len
        {
            return Some(index as u32);
        }
        fallback = Some(index as u32);
    }
    fallback
}

/// The first text node after `anchor` in document order: nodes are pushed in
/// that order, so the node with the anchor's path, then the next text, is it.
fn next_text_after<'a>(doc: &'a StyledDoc, anchor: &Anchor) -> Option<&'a Anchor> {
    let mut seen = false;
    for node in &doc.nodes {
        let Some(src) = node.src.as_ref() else {
            continue;
        };
        if src.spine != anchor.spine {
            continue;
        }
        if !seen {
            if src.path == anchor.path {
                seen = true;
            }
            continue;
        }
        if node.kind.is_text() {
            return Some(src);
        }
    }
    None
}

/// The last line of the block an empty anchor sits in: its parent's lines.
fn last_line_in_block(ir: &LayoutResult, anchor: &Anchor) -> Option<u32> {
    if anchor.path.len() < 2 {
        return None;
    }
    let parent = &anchor.path[..anchor.path.len() - 1];
    let mut fallback = None;
    for (index, line) in ir.runs.lines.iter().enumerate() {
        let Some(line_anchor) = line.anchor.as_ref() else {
            continue;
        };
        if line_anchor.spine == anchor.spine && line_anchor.path.starts_with(parent) {
            fallback = Some(index as u32);
        }
    }
    fallback
}

fn op_bytes(ops: &[ebook_ir::Op]) -> usize {
    use core::mem::size_of;
    ops.iter()
        .map(|op| match op {
            ebook_ir::Op::GlyphRun(run) => size_of::<ebook_ir::Op>() + run.text.len(),
            ebook_ir::Op::Link { href, .. } => size_of::<ebook_ir::Op>() + href.len(),
            ebook_ir::Op::Clip { ops, .. } => size_of::<ebook_ir::Op>() + op_bytes(ops),
            _ => size_of::<ebook_ir::Op>(),
        })
        .sum()
}

impl LastLayout {
    fn reader(&self) -> Reader<'_> {
        Reader::new(&self.ir)
    }

    /// 粗略字节数：只用于缓存淘汰，不需要精确到分配器粒度。
    ///
    /// 这里必须把 `Vec`/`String` 的堆分配算进去：只数结构体本身会让 8 MB
    /// 的预算实际留下几倍的内存。`Op` 里的 run 文本、链接、嵌套 clip，
    /// 以及锚点路径，都是实打实的堆数据。
    fn approx_bytes(&self) -> usize {
        use core::mem::size_of;
        let nodes = self.doc.nodes.len() * size_of::<ebook_ir::Node>();
        let links = self
            .doc
            .nodes
            .iter()
            .map(|node| {
                node.link.as_ref().map_or(0, String::len)
                    + node
                        .src
                        .as_ref()
                        .map_or(0, |anchor| anchor.path.len() * size_of::<u16>())
            })
            .sum::<usize>();
        let images = self
            .doc
            .images
            .iter()
            .map(|image| {
                size_of::<ebook_ir::ImageMeta>()
                    + image.mime.as_ref().map_or(0, String::len)
                    + image.alt.as_ref().map_or(0, String::len)
                    + image.src.as_ref().map_or(0, String::len)
            })
            .sum::<usize>();
        let ops = op_bytes(&self.ir.display.ops);
        let lines = self
            .ir
            .runs
            .lines
            .iter()
            .map(|line| {
                size_of::<ebook_ir::LineBox>()
                    + line
                        .anchor
                        .as_ref()
                        .map_or(0, |anchor| anchor.path.len() * size_of::<u16>())
            })
            .sum::<usize>();
        let issues = self
            .ir
            .issues
            .iter()
            .map(|issue| {
                size_of::<ebook_ir::Issue>() + issue.detail.as_ref().map_or(0, String::len)
            })
            .sum::<usize>();
        self.doc.text.len()
            + self.doc.children.len() * size_of::<NodeId>()
            + nodes
            + links
            + images
            + ops
            + self.ir.display.glyphs.len() * size_of::<ebook_ir::Glyph>()
            + lines
            + self.ir.blocks.len() * size_of::<ebook_ir::BlockBox>()
            + self.anchors.len() * size_of::<ChapterAnchor>()
            + issues
    }

    /// 这份排版结果是不是这些输入排出来的。
    #[allow(clippy::too_many_arguments)]
    fn matches(
        &self,
        spine: usize,
        mode: Mode,
        viewport: Viewport,
        style: Style,
        font_rev: u32,
        limits: Limits,
        window: Option<BlockWindow>,
    ) -> bool {
        self.spine == spine
            && self.mode == mode
            && self.viewport == viewport
            && self.style == style
            && self.font_rev == font_rev
            && self.limits == limits
            && self.window == window
    }
}

/// 排版结果缓存的字节预算。滚动时当前章 + 前后各一章通常就够；按字节而不是
/// 按章数淘汰，小章节可以多留几份，大章节不会把四份整章 IR 同时留在内存里。
const CHAPTER_CACHE_BYTES: usize = 8 * 1024 * 1024;
/// 至少保留这么多份，保证跨章瓦片不用反复重排。
const CHAPTER_CACHE_MIN: usize = 2;
/// 全文搜索索引的字节预算。搜索会扫全书；小的章保留下来，
/// 重复搜索同一个词时不必重新解析。超出预算按 FIFO 淘汰。
const SEARCH_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// 连续滚动会话：全书高度索引 + 视口/样式。
///
/// 高度是**分层**的：`scroll_begin` 为每章算一个估计值（只看章节字节数），
/// 滚到某章才用排版结果替换该章的估计值。所以：
/// - `scroll_total_height()` 不再排整本书；
/// - `scroll_slice` / `scroll_hit` 只排相交的章；
/// - 实测值可以导出/导入，下次打开直接恢复精度。
struct ScrollSession {
    viewport: Viewport,
    style: Style,
    font_rev: u32,
    index: HeightIndex,
}

impl ScrollSession {
    fn new(viewport: Viewport, style: Style, font_rev: u32, index: HeightIndex) -> Self {
        Self {
            viewport,
            style,
            font_rev,
            index,
        }
    }

    fn count(&self) -> usize {
        self.index.len()
    }

    fn matches(&self, viewport: Viewport, style: Style, font_rev: u32, count: usize) -> bool {
        self.viewport == viewport
            && self.style == style
            && self.font_rev == font_rev
            && self.index.len() == count
    }

    fn chapter(&self, index: usize) -> ScrollChapter {
        ScrollChapter {
            spine: index,
            top: self.index.top(index),
            height: self.index.height(index),
            ok: self.index.error(index).is_none(),
            estimated: !self.index.is_measured(index),
        }
    }
}

/// 已打开的书。两种格式共用排版、字体、交互查询，只在"章从哪来、资产怎么取、
/// 链接怎么解析"上分叉。
enum Backend {
    Epub(EpubReader<'static>),
    Txt(TxtReader),
    /// 宿主按章推送（在线书源）。没有整本字节，章节可以 append/replace。
    Stream(StreamBook),
}

impl Backend {
    fn book(&self) -> &Book {
        match self {
            Backend::Epub(reader) => reader.book(),
            Backend::Txt(reader) => reader.book(),
            Backend::Stream(reader) => reader.book(),
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
            Backend::Stream(reader) => reader
                .chapter_with_anchors(index)
                .map(|(doc, _links, anchors)| (doc, anchors)),
        }
    }

    /// 拥有字节的资产读取：读完就把压缩字节丢掉，只留解码后的图片缓存。
    fn asset_owned(&self, href: &str) -> Option<(Vec<u8>, String)> {
        match self {
            Backend::Epub(reader) => reader.asset_owned(href),
            Backend::Txt(_) => None,
            Backend::Stream(_) => None,
        }
    }

    fn resolve_link_from(&self, spine: usize, href: &str) -> Option<LinkTarget> {
        match self {
            Backend::Epub(reader) => reader.resolve_link_from(spine, href),
            Backend::Txt(reader) => reader.resolve_link_from(spine, href),
            Backend::Stream(reader) => reader.resolve_link_from(spine, href),
        }
    }

    /// 一章的原始字节数。高度估计只需要它，不需要先解析正文。
    fn chapter_size(&self, index: usize) -> Option<u64> {
        match self {
            Backend::Epub(reader) => reader.chapter_size(index),
            Backend::Txt(reader) => reader.chapter_size(index),
            Backend::Stream(reader) => reader.chapter_size(index),
        }
    }

    /// 内嵌字体的字节。惰性打开的书在这里才真正解压字体。
    fn font_bytes(&self, index: usize) -> Option<Vec<u8>> {
        match self {
            Backend::Epub(reader) => reader.font_bytes(index),
            Backend::Txt(_) => None,
            Backend::Stream(_) => None,
        }
    }

    /// 章节内容版本；只有流式书才有。EPUB/TXT 的内容不会变，返回 `None`。
    fn chapter_rev(&self, index: usize) -> Option<u64> {
        match self {
            Backend::Stream(reader) => reader.chapter_rev(index),
            _ => None,
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
    /// 书内容指纹：高度索引跨会话复用的键。扫首尾各 8KB，不哈希整本书。
    fingerprint: u64,
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
    /// 排版前段（盒树 + 断行 + 整形）的缓存。分页依赖页高，断行只依赖宽度，
    /// 所以地址栏收起、旋屏这类「同宽不同高」的重排只需重跑分页。
    flow_cache: FlowCache,
    /// 传给 `layout_cached` 的版本号：`(spine, style, font_rev)` 变了就加一。
    flow_rev: u64,
    /// 上一次用过的版本号输入。用它判断要不要递增 `flow_rev`，比手写哈希稳。
    flow_inputs: Option<(usize, Style, u32)>,
    /// 按章缓存的纯文本索引；不参与排版，只给搜索用。
    search_cache: SearchCache,
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
            // 字体字节懒加载：打开带几 MB 内嵌字体的书不再先付一遍字体内存。
            Backend::Epub(EpubReader::open_lazy(raw)?)
        } else {
            let text = ebook_format::decode_text(&boxed);
            Backend::Txt(TxtReader::from_text(&text, Some(name)))
        };
        let fingerprint = fingerprint_of(&boxed, name);

        Ok(Self {
            backend,
            bytes: boxed,
            name: name.to_string(),
            fingerprint,
            font: None,
            last: None,
            chapters: Vec::new(),
            scroll: None,
            last_error: None,
            font_error: None,
            images: ImageCache::default(),
            limits: Limits::default(),
            font_rev: 0,
            flow_cache: FlowCache::new(),
            flow_rev: 0,
            flow_inputs: None,
            search_cache: SearchCache::new(SEARCH_CACHE_BYTES),
        })
    }

    /// 新建一本空的书。章节由宿主用 [`Engine::append_chapter`] 推入；
    /// 没有整本字节，打开不扫描任何正文。
    pub fn new_book(title: &str) -> Self {
        let stream = StreamBook::new(title);
        let fingerprint = stream.fingerprint();
        Self {
            backend: Backend::Stream(stream),
            bytes: Box::default(),
            name: title.to_string(),
            fingerprint,
            font: None,
            last: None,
            chapters: Vec::new(),
            scroll: None,
            last_error: None,
            font_error: None,
            images: ImageCache::default(),
            limits: Limits::default(),
            font_rev: 0,
            flow_cache: FlowCache::new(),
            flow_rev: 0,
            flow_inputs: None,
            search_cache: SearchCache::new(SEARCH_CACHE_BYTES),
        }
    }

    /// 尾部追加一章纯文本，返回新的章数。`title` 由宿主给（显示在目录/章节头）。
    ///
    /// `text` 每行一段，不跑“第 N 章”分章规则；单章受 `max_text_bytes` 限制。
    /// 只在流式书（[`Engine::new_book`]）上可用。
    pub fn append_chapter(&mut self, title: &str, text: &str) -> Result<usize, EngineError> {
        const MAX_CHAPTER_BYTES: usize = 20_000_000;
        if text.is_empty() {
            return Err(EngineError::Chapter(String::from("empty chapter")));
        }
        if text.len() > MAX_CHAPTER_BYTES {
            return Err(EngineError::Chapter(format!(
                "chapter is {} bytes, limit is {MAX_CHAPTER_BYTES}",
                text.len()
            )));
        }
        let (count, fingerprint) = match &mut self.backend {
            Backend::Stream(reader) => {
                let count = reader.append(title, text);
                (count, reader.fingerprint())
            }
            _ => {
                return Err(EngineError::Chapter(String::from(
                    "append_chapter is only available on a stream book",
                )))
            }
        };
        self.fingerprint = fingerprint;
        self.invalidate_stream_caches(None);
        if let Some(session) = self.scroll.as_ref() {
            let viewport = session.viewport;
            let style = session.style;
            let estimate = estimate_height(
                self.backend.chapter_size(count - 1),
                viewport,
                style,
                viewport.height,
            );
            let key = self.height_key(viewport, style);
            if let Some(session) = self.scroll.as_mut() {
                session.index.push_estimate(estimate);
                session.index.set_key(key);
            }
        }
        Ok(count)
    }

    /// 替换一章。章数与其它章不变；该章 `rev` 变、它的排版/高度缓存失效。
    pub fn replace_chapter(
        &mut self,
        index: usize,
        title: &str,
        text: &str,
    ) -> Result<(), EngineError> {
        const MAX_CHAPTER_BYTES: usize = 20_000_000;
        if text.is_empty() {
            return Err(EngineError::Chapter(String::from("empty chapter")));
        }
        if text.len() > MAX_CHAPTER_BYTES {
            return Err(EngineError::Chapter(format!(
                "chapter is {} bytes, limit is {MAX_CHAPTER_BYTES}",
                text.len()
            )));
        }
        let fingerprint = match &mut self.backend {
            Backend::Stream(reader) => {
                if index >= reader.count() {
                    return Err(EngineError::ChapterOutOfRange(index as u32));
                }
                if !reader.replace(index, title, text) {
                    return Err(EngineError::ChapterOutOfRange(index as u32));
                }
                reader.fingerprint()
            }
            _ => {
                return Err(EngineError::Chapter(String::from(
                    "replace_chapter is only available on a stream book",
                )))
            }
        };
        self.fingerprint = fingerprint;
        self.invalidate_stream_caches(Some(index));
        if let Some(session) = self.scroll.as_ref() {
            let viewport = session.viewport;
            let style = session.style;
            let estimate = estimate_height(
                self.backend.chapter_size(index),
                viewport,
                style,
                viewport.height,
            );
            let key = self.height_key(viewport, style);
            if let Some(session) = self.scroll.as_mut() {
                session.index.invalidate(index, estimate);
                session.index.set_key(key);
            }
        }
        Ok(())
    }

    /// 清空流式书的全部章节。EPUB/TXT 不支持。
    pub fn clear_chapters(&mut self) -> Result<(), EngineError> {
        let (fingerprint, is_empty) = match &mut self.backend {
            Backend::Stream(reader) => {
                reader.clear();
                (reader.fingerprint(), reader.count() == 0)
            }
            _ => {
                return Err(EngineError::Chapter(String::from(
                    "clear_chapters is only available on a stream book",
                )))
            }
        };
        self.fingerprint = fingerprint;
        self.scroll = None;
        self.invalidate_stream_caches(None);
        let _ = is_empty;
        Ok(())
    }

    /// 该章内容版本。流式书每次 append/replace 后变化，宿主用它做页位图的键；
    /// EPUB/TXT 内容不会变，返回 `None`。
    pub fn chapter_rev(&self, index: usize) -> Option<u64> {
        self.backend.chapter_rev(index)
    }

    fn invalidate_stream_caches(&mut self, spine: Option<usize>) {
        self.flow_cache.clear();
        self.flow_inputs = None;
        match spine {
            Some(index) => self.search_cache.invalidate(index as u32),
            None => self.search_cache.clear(),
        }
        match spine {
            Some(index) => {
                if self.last.as_ref().is_some_and(|last| last.spine == index) {
                    self.last = None;
                }
                self.chapters.retain(|entry| entry.spine != index);
            }
            None => {
                self.last = None;
                self.chapters.clear();
            }
        }
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

    /// 当前后端：`"epub"` / `"txt"` / `"stream"`。只给适配层做能力查询。
    pub fn format(&self) -> &'static str {
        match self.backend {
            Backend::Epub(_) => "epub",
            Backend::Txt(_) => "txt",
            Backend::Stream(_) => "stream",
        }
    }

    pub fn toc(&self) -> &[TocEntry] {
        self.backend.book().toc()
    }

    /// 封面图片的字节与媒体类型（EPUB 的 `cover-image`，或 EPUB2 的
    /// `meta name="cover"`）；没有封面时 `None`。TXT 永远没有。
    ///
    /// 返回拥有值：封面只读一次，没有必要为了一个 `&[u8]` 把压缩字节常驻。
    pub fn cover(&self) -> Option<(Vec<u8>, String)> {
        let href = self.backend.book().cover_href.as_deref()?;
        self.backend.asset_owned(href)
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
            .is_some_and(|last| last.matches(spine, mode, viewport, style, font_rev, limits, None))
        {
            return Ok(());
        }
        if let Some(index) = self
            .chapters
            .iter()
            .position(|entry| entry.matches(spine, mode, viewport, style, font_rev, limits, None))
        {
            let layout = self.chapters.remove(index);
            if let Some(previous) = self.last.replace(layout) {
                self.remember(previous);
            }
            return Ok(());
        }
        let layout = self.build_layout(spine, mode, viewport, style, None)?;
        if let Some(previous) = self.last.replace(layout) {
            self.remember(previous);
        }
        Ok(())
    }

    fn remember(&mut self, layout: LastLayout) {
        self.chapters.push(layout);
        // 按字节预算淘汰最旧的整章 IR；小章节能多留几份，大章节不会把四份
        // 几十 MB 的结果同时留在内存里。
        let mut total: usize = self.chapters.iter().map(LastLayout::approx_bytes).sum();
        while self.chapters.len() > CHAPTER_CACHE_MIN && total > CHAPTER_CACHE_BYTES {
            let dropped = self.chapters.remove(0);
            total = total.saturating_sub(dropped.approx_bytes());
        }
    }

    fn build_layout(
        &mut self,
        spine: usize,
        mode: Mode,
        viewport: Viewport,
        style: Style,
        window: Option<BlockWindow>,
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
        // 断行的输入只有「文档 + 字体 + 可用宽度 + 行数预算」。`spine`、`style`、
        // `font_rev` 一变，文档字节就变，所以它们是这里唯一的版本号来源；
        // **页高与 `mode` 故意不在里面**：它们只影响分页。
        let inputs = (spine, style, self.font_rev);
        if self.flow_inputs != Some(inputs) {
            self.flow_inputs = Some(inputs);
            self.flow_rev = self.flow_rev.wrapping_add(1);
        }
        // 拆借用：`env.measurer` 借的是 `self.font`，缓存借的是另一个字段。
        let flow_rev = self.flow_rev;
        let Engine {
            font, flow_cache, ..
        } = self;
        let env = match font.as_ref() {
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
            window,
            ..LayoutOptions::default()
        };
        let ir =
            Layout::new().layout_cached(&doc, &env, &constraint, &options, flow_cache, flow_rev);
        let text_color = ebook_ir::default_text_color(&doc);

        Ok(LastLayout {
            spine,
            mode,
            viewport,
            style,
            font_rev: self.font_rev,
            limits: self.limits,
            ir,
            doc,
            text_color,
            anchors,
            window,
        })
    }

    /// 只排一章里的一个块窗口（`from..=to` 的文档节点子树）。
    ///
    /// 这是章内流式排版入口：窗口外的块不进入 `flow`，所以产出的 ops/行数
    /// 只和窗口有关，不受前面有多少段影响。窗口坐标已经平移到 0 附近；
    /// `blocks()` 给出每个块在窗口里的矩形，宿主按 `[spine, node]` 持久化。
    ///
    /// 结果会替换当前章的 `last`：`bitmap` / `pages` / `hit` 都只看到这一窗。
    /// 需要整章时再 `layout()` 一次即可。
    pub fn layout_block_window(
        &mut self,
        spine: usize,
        from: NodeId,
        to: NodeId,
        viewport: Viewport,
        style: Style,
    ) -> Result<(), EngineError> {
        let window = BlockWindow { from, to };
        let layout = self.build_layout(spine, Mode::Scroll, viewport, style, Some(window))?;
        if let Some(previous) = self.last.replace(layout) {
            self.remember(previous);
        }
        Ok(())
    }

    /// 当前窗/整章里的块表，按流坐标排序。窗口排版后就是这一窗的块。
    pub fn blocks(&self) -> &[BlockBox] {
        self.last
            .as_ref()
            .map(|last| last.ir.blocks.as_slice())
            .unwrap_or(&[])
    }

    /// 页内点 → 不依赖像素的原文锚点（章 + DOM 路径 + 字节偏移）。
    pub fn anchor_at(&self, page: u32, x: f32, y: f32) -> Option<Anchor> {
        self.last
            .as_ref()?
            .reader()
            .anchor_at(page, PointF::new(x, y))
    }

    /// 两个页内位置 → 稳定区间。
    pub fn anchor_range(&self, a: Location, b: Location) -> Option<AnchorRange> {
        self.last.as_ref()?.ir.anchor_range(a, b)
    }

    /// 稳定锚点 → 当前排版里的页/行/字节位置。
    pub fn locate(&self, anchor: &Anchor) -> Option<Location> {
        let last = self.last.as_ref()?;
        if anchor.spine as usize != last.spine {
            return None;
        }
        last.ir.location_of(anchor)
    }

    /// 锚点 → 它所属的块节点。窗口排版的 `from`/`to` 用它。
    pub fn block_of_anchor(&self, anchor: &Anchor) -> Option<NodeId> {
        let last = self.last.as_ref()?;
        if anchor.spine as usize != last.spine {
            return None;
        }
        last.doc.block_of_anchor(anchor)
    }

    /// 当前章里某一行的块单元。
    pub fn block_at_line(&self, line: u32) -> Option<&BlockBox> {
        self.last.as_ref()?.ir.block_at_line(line)
    }

    /// 页内点 → 宿主可持久化的 [`Locator`]（章 + 节点 + 字节偏移）。
    pub fn locator_at(&self, page: u32, x: f32, y: f32) -> Option<Locator> {
        let last = self.last.as_ref()?;
        // 和 `hit` 用同一条路径：宿主拿到的 Locator 必须能换算回同一个
        // 页/行/字节；`anchor_at` 是严格命中，落在行间空白时返回 None。
        let location = last.reader().hit(page, PointF::new(x, y))?;
        let line = last.ir.line(location.line)?;
        let anchor = line.anchor.as_ref()?;
        Some(Locator {
            spine: anchor.spine,
            node: line.node,
            offset: anchor.offset.saturating_add(location.byte),
        })
    }

    /// [`Locator`] → 当前排版里的页/行/字节位置。
    pub fn locate_locator(&self, locator: &Locator) -> Option<Location> {
        let last = self.last.as_ref()?;
        if locator.spine as usize != last.spine {
            return None;
        }
        let anchor = last.doc.anchor_of(locator.node)?;
        last.ir.location_of(&anchor.with_offset(locator.offset))
    }

    /// 两个页内位置 → 稳定区间。端点落在行尾时夹到该行长度。
    pub fn locator_range(&self, a: Location, b: Location) -> Option<LocatorRange> {
        let last = self.last.as_ref()?;
        let point = |location: Location| {
            let line = last.ir.line(location.line)?;
            let anchor = line.anchor.as_ref()?;
            Some(Locator {
                spine: anchor.spine,
                node: line.node,
                offset: anchor.offset.saturating_add(location.byte),
            })
        };
        Some(LocatorRange {
            start: point(a)?,
            end: point(b)?,
        })
    }

    /// [`Locator`] 所属的块节点；窗口排版的 `from`/`to` 可以直接用它。
    pub fn block_of_locator(&self, locator: &Locator) -> Option<NodeId> {
        let last = self.last.as_ref()?;
        if locator.spine as usize != last.spine {
            return None;
        }
        last.doc.block_of(locator.node)
    }

    /// 前段缓存的命中/未命中次数。宿主可以据此观察「换页高」有没有真的省下断行。
    pub fn flow_cache_stats(&self) -> (u64, u64) {
        (self.flow_cache.hits(), self.flow_cache.misses())
    }

    /// 当前缓存的近似内存占用。用于宿主内存警告时的决策，不要求逐字节精确。
    pub fn memory_usage(&self) -> MemoryUsage {
        let layout_bytes = self.last.as_ref().map_or(0, LastLayout::approx_bytes);
        let chapters_bytes = self.chapters.iter().map(LastLayout::approx_bytes).sum();
        let flow_bytes = self.flow_cache.bytes();
        let image_bytes = self.images.bytes();
        let search_bytes = self.search_cache.bytes();
        let font_bytes = self.font.as_ref().map_or(0, FontBook::cache_bytes);
        MemoryUsage {
            layout_bytes,
            chapters_bytes,
            flow_bytes,
            image_bytes,
            search_bytes,
            font_bytes,
            total_bytes: layout_bytes
                .saturating_add(chapters_bytes)
                .saturating_add(flow_bytes)
                .saturating_add(image_bytes)
                .saturating_add(search_bytes)
                .saturating_add(font_bytes),
        }
    }

    /// 丢掉可再生的缓存，保留当前页 IR。
    ///
    /// 适合 iOS/Android 的内存警告：跨章 IR、断行缓存、图片 RGBA、字体位图
    /// 全部释放；当前页仍能继续画。宿主若还需要更多，可以重新
    /// [`layout`](Self::layout) 或直接 drop `Engine`。
    pub fn trim_memory(&mut self) {
        self.chapters.clear();
        self.flow_cache.clear();
        self.search_cache.clear();
        self.images.clear();
        if let Some(font) = self.font.as_ref() {
            font.clear_caches();
        }
    }

    /// 当前章图片表。`image_id` 就是这里的下标；没排版时为空。
    pub fn images(&self) -> &[ebook_ir::ImageMeta] {
        self.last
            .as_ref()
            .map_or(&[], |layout| layout.doc.images.as_slice())
    }

    /// 全分辨率预览一张当前章图片。页面渲染走降采样缓存，这条路径给宿主
    /// 的“点击查看原图”用；两者缓存键不同，互不污染。
    pub fn image_full(&mut self, id: ImageId) -> Option<Arc<ImageData>> {
        let src = self
            .last
            .as_ref()?
            .doc
            .images
            .get(id as usize)?
            .src
            .clone()?;
        if let Some(hit) = self.images.get(&src, 0, 0) {
            return Some(hit);
        }
        let decoded = self
            .backend
            .asset_owned(&src)
            .and_then(|(bytes, _)| ebook_format::image::decode_image(&bytes))
            .map(Arc::new);
        let hit = decoded.clone();
        self.images
            .insert(src, 0, 0, decoded, self.limits.image_cache_bytes);
        hit
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
    /// 把位置换成该滚到哪。
    ///
    /// 这里**不排任何章**：只读 zip 中央目录/切章区间，为每章算一个估计高度。
    /// 滚动条可以立刻显示；`scroll_chapter` / `scroll_slice` 滚到某章时才排版
    /// 那一章并用实测值替换估计值。
    ///
    /// 上下留白会被置 0（滚动容器自己留，瓦片之间才没有空白带），左右保留。
    /// 同样的视口/样式/字体再调一次不会丢失已实测的高度；字体换了会把实测值
    /// 退回估计值（旧高度属于旧字体），但**保留会话与滚动位置**。
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
            .is_some_and(|session| session.matches(viewport, style, self.font_rev, count));
        if !keep {
            let estimates = self.estimates(viewport, style);
            let reuse = self.scroll.as_ref().is_some_and(|session| {
                session.viewport == viewport && session.style == style && session.count() == count
            });
            if reuse {
                // 只有字体版本变了：保留章数/视口，实测高度作废、退回估计。
                let key = self.height_key(viewport, style);
                let session = self.scroll.as_mut().expect("checked above");
                session.font_rev = self.font_rev;
                session.index = HeightIndex::with_estimates(estimates, key);
            } else {
                let key = self.height_key(viewport, style);
                self.scroll = Some(ScrollSession::new(
                    viewport,
                    style,
                    self.font_rev,
                    HeightIndex::with_estimates(estimates, key),
                ));
            }
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
    /// 只实测 `index` 这一章，不要求前面的章已经量过。`estimated` 为 `true`
    /// 表示这一章仍是估计值（排版失败也会被当成“已量过”，避免反复重试）。
    pub fn scroll_chapter(&mut self, index: usize) -> Result<ScrollChapter, EngineError> {
        self.scroll_check()?;
        let count = self.chapter_count();
        if index >= count {
            return Err(EngineError::ChapterOutOfRange(index as u32));
        }
        self.scroll_measure(index)?;
        let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
        Ok(session.chapter(index))
    }

    /// 全书章节表。会实测每一章（宿主要分摊开销请逐章 `scroll_chapter`，
    /// 或者用 [`Engine::scroll_chapters_estimated`] 先拿估计表）。
    pub fn scroll_chapters(&mut self) -> Result<Vec<ScrollChapter>, EngineError> {
        self.scroll_check()?;
        let count = self.chapter_count();
        for index in 0..count {
            self.scroll_measure(index)?;
        }
        let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
        Ok((0..count).map(|index| session.chapter(index)).collect())
    }

    /// 全书章节表，但不触发排版：没实测的章用估计高度。
    pub fn scroll_chapters_estimated(&mut self) -> Result<Vec<ScrollChapter>, EngineError> {
        self.scroll_check()?;
        let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
        Ok((0..session.count())
            .map(|index| session.chapter(index))
            .collect())
    }

    /// 高度索引的当前状态：总高、已实测章数、总章数。
    pub fn scroll_metrics(&mut self) -> Result<ScrollMetrics, EngineError> {
        self.scroll_check()?;
        let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
        Ok(ScrollMetrics {
            total: session.index.total(),
            measured: session.index.measured_count() as u32,
            count: session.count() as u32,
        })
    }

    /// 导出已实测的高度表（键含书指纹/视口/样式/字体版本）。宿主持久化它，
    /// 下次打开 `scroll_begin` 后 `scroll_index_import`，滚动条直接是上次的精度。
    pub fn scroll_index_export(&mut self) -> Result<Vec<u8>, EngineError> {
        self.scroll_check()?;
        let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
        Ok(session.index.export())
    }

    /// 导入高度表。键或章数不匹配时返回 `false`，不部分应用。
    pub fn scroll_index_import(&mut self, bytes: &[u8]) -> Result<bool, EngineError> {
        self.scroll_check()?;
        let session = self.scroll.as_mut().ok_or(EngineError::NoScroll)?;
        Ok(session.index.import(bytes))
    }

    /// 全书流坐标总长（滚动容器的 contentSize）。
    ///
    /// 这是当前索引的总和：已实测的章用实测值，其余用估计值。**不会排整本书**。
    /// 需要精确值就逐章 `scroll_chapter` 或用 [`Engine::scroll_chapters`]。
    pub fn scroll_total_height(&mut self) -> Result<f32, EngineError> {
        self.scroll_check()?;
        let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
        Ok(session.index.total())
    }

    /// 全书流坐标 `y` 落在哪一章：返回章下标与该章起点。不触发排版。
    pub fn scroll_spine_at(&mut self, y: f32) -> Result<Option<(usize, f32)>, EngineError> {
        self.scroll_check()?;
        let Some(index) = self.scroll_index_at(y)? else {
            return Ok(None);
        };
        let top = self
            .scroll
            .as_ref()
            .map_or(0.0, |session| session.index.top(index));
        Ok(Some((index, top)))
    }

    /// 光栅化整个滚动容器里的一段：`y`/`height` 是**全书流坐标**，跨章也画。
    ///
    /// 一种调用搞定一屏（或一枚瓦片）：引擎自己找出涉及的章、分别光栅化再拼
    /// 到同一张位图，章缝不会错行。内容之外的区域是纸色。
    ///
    /// `y` 先按当前高度索引换算成「章 + 章内偏移」再排版该章：实测让总高变化
    /// 时，同一块内容不会因为前面某章高了一行而跳走。宿主如果要绝对稳定，
    /// 应当用 `scroll_line_y` / `scroll_anchor_y` 维护锚点。
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
        let style = self
            .scroll
            .as_ref()
            .map(|session| session.style)
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
        let count = self.chapter_count();
        // 绝对 y →（章，章内偏移）。只实测这一章；它前面的章仍可用估计值。
        let Some(first) = self.scroll_index_at(y)? else {
            return Ok(out);
        };
        let local = {
            let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
            (y - session.index.top(first)).max(0.0)
        };
        self.scroll_measure(first)?;
        let (y, mut index) = {
            let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
            let height = session.index.height(first).max(0.0);
            let local = local.min(height);
            let y = session.index.top(first) + local;
            (y, session.index.locate(y).unwrap_or(count))
        };

        let stride = width as usize * 4;
        while index < count {
            self.scroll_measure(index)?;
            let (top, chapter_height) = {
                let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
                (session.index.top(index), session.index.height(index))
            };
            if top >= y + height {
                break;
            }
            let segment_start = top.max(y);
            let segment_end = (top + chapter_height).min(y + height);
            if segment_end > segment_start {
                let row_from = (((segment_start - y) * scale).round() as i64).clamp(0, rows as i64);
                let row_to =
                    (((segment_end - y) * scale).round() as i64).clamp(row_from, rows as i64);
                if row_to > row_from {
                    let local_top = segment_start - top;
                    let local_height = (row_to - row_from) as f32 / scale;
                    self.focus(index, Mode::Scroll, viewport, style)?;
                    let bitmap =
                        self.bitmap_window(0, local_top, local_height, scale, ink, paper, bold)?;
                    if bitmap.width == width {
                        let copy = (bitmap.height as i64).min(row_to - row_from) as usize;
                        let from = row_from as usize * stride;
                        let bytes = copy * stride;
                        if from + bytes <= out.rgba.len() && bitmap.rgba.len() >= bytes {
                            out.rgba[from..from + bytes].copy_from_slice(&bitmap.rgba[..bytes]);
                        }
                    }
                }
            }
            if top + chapter_height >= y + height {
                break;
            }
            index += 1;
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
        self.scroll_measure(index)?;
        let (top, height, viewport, style) = {
            let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
            (
                session.index.top(index),
                session.index.height(index),
                session.viewport,
                session.style,
            )
        };
        self.focus(index, Mode::Scroll, viewport, style)?;
        let local = (y - top).clamp(0.0, height.max(0.0));
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
    ///
    /// 只实测 `spine` 这一章；它前面的章即使还是估计值也不影响返回值之外的章。
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

    /// 全书流坐标 → 滚动进度 0..=1。用当前高度索引，不触发全书排版。
    pub fn scroll_progress(&mut self, y: f32) -> Result<f32, EngineError> {
        let total = self.scroll_total_height()?;
        if total <= 0.0 || !y.is_finite() {
            return Ok(0.0);
        }
        Ok((y / total).clamp(0.0, 1.0))
    }

    /// 滚动会话是否还在（字体换过之后会自动退回估计值，不会丢弃会话）。
    pub fn has_scroll_session(&self) -> bool {
        self.scroll.is_some()
    }

    fn scroll_check(&mut self) -> Result<(), EngineError> {
        let Some(session) = self.scroll.as_ref() else {
            return Err(EngineError::NoScroll);
        };
        if session.font_rev == self.font_rev {
            return Ok(());
        }
        // 字体变了：实测高度不可信，但会话/视口/滚动位置还在。退回估计值，
        // 实测过的章会在下次滚到时重新量。
        let viewport = session.viewport;
        let style = session.style;
        let estimates = self.estimates(viewport, style);
        let key = self.height_key(viewport, style);
        if let Some(session) = self.scroll.as_mut() {
            session.font_rev = self.font_rev;
            session.index = HeightIndex::with_estimates(estimates, key);
        }
        Ok(())
    }

    /// 实测一章。只排这一章；失败按 0 高记录，不向调用方传播（`ok` 会说明）。
    fn scroll_measure(&mut self, index: usize) -> Result<(), EngineError> {
        self.scroll_check()?;
        let count = self.chapter_count();
        if index >= count {
            return Err(EngineError::ChapterOutOfRange(index as u32));
        }
        let (viewport, style, measured) = {
            let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
            (
                session.viewport,
                session.style,
                session.index.is_measured(index),
            )
        };
        if measured {
            return Ok(());
        }
        let height = match self.focus(index, Mode::Scroll, viewport, style) {
            Ok(()) => self
                .last
                .as_ref()
                .and_then(|last| last.ir.pages.first())
                .map_or(0.0, |page| ceil_to_viewport(page.size.h, viewport.height)),
            Err(error) => {
                if let Some(session) = self.scroll.as_mut() {
                    session.index.set_error(index, error.to_string());
                }
                return Ok(());
            }
        };
        if let Some(session) = self.scroll.as_mut() {
            session.index.set_measured(index, height);
        }
        Ok(())
    }

    /// 流坐标 `y` 落在哪一章。纯索引查询，不排版。
    fn scroll_index_at(&mut self, y: f32) -> Result<Option<usize>, EngineError> {
        self.scroll_check()?;
        let y = if y.is_finite() { y.max(0.0) } else { 0.0 };
        let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
        Ok(session.index.locate(y))
    }

    /// 排版/聚焦一章并返回它在全书流坐标里的起点。
    fn scroll_focus(&mut self, spine: usize) -> Result<f32, EngineError> {
        self.scroll_check()?;
        let count = self.chapter_count();
        if spine >= count {
            return Err(EngineError::ChapterOutOfRange(spine as u32));
        }
        self.scroll_measure(spine)?;
        let (top, viewport, style) = {
            let session = self.scroll.as_ref().ok_or(EngineError::NoScroll)?;
            (session.index.top(spine), session.viewport, session.style)
        };
        self.focus(spine, Mode::Scroll, viewport, style)?;
        Ok(top)
    }

    /// 为全书每一章算一个初始高度。只看章节字节数 + 视口 + 样式，不解压正文。
    fn estimates(&self, viewport: Viewport, style: Style) -> Vec<f32> {
        let count = self.chapter_count();
        let sizes: Vec<Option<u64>> = (0..count)
            .map(|index| self.backend.chapter_size(index))
            .collect();
        let sample = sizes.iter().flatten().copied().next().unwrap_or(16 * 1024);
        let fallback = estimate_height(Some(sample), viewport, style, viewport.height);
        sizes
            .into_iter()
            .map(|size| estimate_height(size.or(Some(sample)), viewport, style, fallback))
            .collect()
    }

    fn height_key(&self, viewport: Viewport, style: Style) -> u64 {
        index_key(self.fingerprint, viewport, style, self.font_rev)
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
        if let Some(line) = line_of_anchor(&last.ir, anchor) {
            return Some(line);
        }
        // 空的内联锚点（`<a id="m1"></a>`，Calibre 的脚注就是这样）自己不成
        // 行：它的位置是它后面第一个文本节点。锚点在块尾、后面没有同一块里
        // 的文本时，落在该块的最后一行。
        if let Some(next) = next_text_after(&last.doc, anchor) {
            let parent = &anchor.path[..anchor.path.len().saturating_sub(1)];
            if next.path.starts_with(parent) {
                if let Some(line) = line_of_anchor(&last.ir, next) {
                    return Some(line);
                }
            }
        }
        last_line_in_block(&last.ir, anchor)
    }

    /// 全书全文搜索。默认大小写不敏感、按 Unicode 小写比较；中文按子串，
    /// 英文/数字查询要求词边界（`the` 不会命中 `theatre`）。
    ///
    /// 索引按章缓存，`(spine, rev)` 是键：流式书 `replace_chapter` 后只失效
    /// 那一章。解析失败的章计入 `skipped`，不中断其余章。`max_hits == 0`
    /// 时用 `ebook_search::DEFAULT_MAX_HITS`。
    ///
    /// 命中是 `LocatorRange`，和字号、页高、滚动位置无关；宿主对目标章调用
    /// [`Engine::layout`] 后再用 [`Engine::locate_locator`] 换算页/行。
    pub fn search(&mut self, query: &str, max_hits: u32) -> SearchResults {
        let count = self.chapter_count();
        let max_hits = if max_hits == 0 {
            ebook_search::DEFAULT_MAX_HITS
        } else {
            max_hits
        };
        let mut out = SearchResults::default();
        if count == 0 || query.trim().is_empty() {
            return out;
        }
        let titles: Vec<Option<String>> = (0..count)
            .map(|spine| {
                self.toc()
                    .iter()
                    .find(|entry| entry.spine == spine)
                    .map(|entry| entry.label.clone())
            })
            .collect();

        for (spine, title) in titles.iter().enumerate() {
            out.scanned = out.scanned.saturating_add(1);
            let rev = self.backend.chapter_rev(spine).unwrap_or(0);
            if self.search_cache.get(spine as u32, rev).is_none() {
                match self.backend.chapter(spine) {
                    Ok((doc, _)) => {
                        let text = SearchText::build(&doc);
                        self.search_cache.insert(spine as u32, rev, text);
                    }
                    Err(_) => {
                        out.skipped = out.skipped.saturating_add(1);
                        continue;
                    }
                }
            }
            let remaining = max_hits.saturating_sub(out.hits.len() as u32);
            if remaining == 0 {
                out.truncated = true;
                break;
            }
            let options = SearchOptions {
                max_hits: remaining,
                ..SearchOptions::default()
            };
            let mut found = Vec::new();
            let truncated = match self.search_cache.get(spine as u32, rev) {
                Some(index) => index.find(query, &options, &mut found),
                None => false,
            };
            for hit in found {
                out.hits.push(SearchHit {
                    range: LocatorRange {
                        start: Locator {
                            spine: spine as u32,
                            node: hit.node,
                            offset: hit.source_start,
                        },
                        end: Locator {
                            spine: spine as u32,
                            node: hit.node,
                            offset: hit.source_end,
                        },
                    },
                    spine,
                    chapter: title.clone(),
                    before: hit.before,
                    text: hit.text,
                    after: hit.after,
                    ellipsis_before: hit.ellipsis_before,
                    ellipsis_after: hit.ellipsis_after,
                });
            }
            if truncated {
                out.truncated = true;
                break;
            }
        }
        out
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
    ///
    /// 惰性打开的书在这里才解压字体字节：宿主没用到这个字体就不付内存。
    pub fn register_font_face(&mut self, index: usize) -> Result<FontId, String> {
        let Some(face) = self.backend.book().fonts().get(index) else {
            let message = format!("没有第 {index} 个内嵌字体");
            self.font_error = Some(message.clone());
            return Err(message);
        };
        let family = face.family.clone();
        let weight = face.weight;
        let style = face.style;
        let href = face.href.clone();
        let Some(bytes) = self.backend.font_bytes(index) else {
            let message = format!("{href}: 字体字节读不出来");
            self.font_error = Some(message.clone());
            return Err(message);
        };
        let font = self.font.get_or_insert_with(FontBook::new);
        match font.try_add_styled(&family, weight, style, bytes) {
            Ok(id) => {
                self.font_error = None;
                self.font_rev = self.font_rev.wrapping_add(1);
                Ok(id)
            }
            Err(error) => {
                let message = format!("{href}: {error}");
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

/// 书内容指纹：长度 + 名字 + 首尾各 8KB。不哈希整本书，几十 MB 的书也
/// 只在打开时付一次很小的常数成本。
fn fingerprint_of(bytes: &[u8], name: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |chunk: &[u8]| {
        for &byte in chunk {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(b"ebook-book-fingerprint-v1");
    eat(&(bytes.len() as u64).to_le_bytes());
    eat(name.as_bytes());
    let head = &bytes[..bytes.len().min(8192)];
    let tail = &bytes[bytes.len().saturating_sub(8192)..];
    eat(head);
    if tail.len() > head.len() || bytes.len() > 8192 {
        eat(tail);
    }
    hash
}

fn unpack_rgba(value: u32) -> Rgba8 {
    Rgba8::new(
        (value >> 24) as u8,
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    )
}

/// 滚动列里的章高按视口高向上取整：每章至少占一屏，题记、单段小节不会和下一章
/// 贴在一起。刚好整页的章别因浮点误差多出一屏；排版失败（0）仍是 0。
fn ceil_to_viewport(height: f32, viewport: f32) -> f32 {
    if !height.is_finite() || height <= 0.0 || !viewport.is_finite() || viewport <= 0.0 {
        return height.max(0.0);
    }
    let pages = (height / viewport - 1.0e-4).ceil().max(1.0);
    pages * viewport
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
        // 墨色既是对比度的依据，也是默认字形的颜色：夜里是浅色字，不是黑字。
        // 只换「默认字色」（正文主导色），作者显式写的链接色/强调色保留，
        // 否则夜间主题会把整页染成一种颜色。
        let ink = unpack_rgba(ink);
        let base = layout.text_color;
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
        rgba: surface.into_rgba(),
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
        if let Some(hit) = self.cache.get(&src, tw, th) {
            return Some(hit);
        }
        let decoded = self
            .book
            .asset_owned(&src)
            .and_then(|(bytes, _)| ebook_format::image::decode_image_scaled(&bytes, tw, th))
            .map(Arc::new);
        let hit = decoded.clone();
        self.cache.insert(src, tw, th, decoded, self.budget);
        hit
    }
}

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

/// 解码后的图片按 `(归档路径, 目标像素)` 缓存。同一张图在页面和
/// “点击看原图”下会各有一份，互不覆盖。
///
/// 用 `Vec` 而不是哈希表：一页涉及的图很少，线性查找更省内存，
/// 也与“IR 里禁止哈希容器”的取向一致。
#[derive(Default)]
struct ImageCache {
    entries: Vec<ImageEntry>,
    bytes: usize,
}

struct ImageEntry {
    src: String,
    w: u32,
    h: u32,
    data: Option<Arc<ImageData>>,
}

impl ImageCache {
    fn get(&mut self, src: &str, w: u32, h: u32) -> Option<Arc<ImageData>> {
        self.entries
            .iter()
            .find(|entry| entry.src == src && entry.w == w && entry.h == h)
            .and_then(|entry| entry.data.clone())
    }

    fn insert(&mut self, src: String, w: u32, h: u32, data: Option<Arc<ImageData>>, budget: usize) {
        let entry_bytes = |entry: &ImageEntry| -> usize {
            core::mem::size_of::<ImageEntry>()
                + entry.src.len()
                + entry.data.as_ref().map_or(0, |image| image.rgba.len())
        };
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.src == src && entry.w == w && entry.h == h)
        {
            let old = self.entries.remove(index);
            self.bytes = self.bytes.saturating_sub(entry_bytes(&old));
        }
        let entry = ImageEntry { src, w, h, data };
        self.bytes = self.bytes.saturating_add(entry_bytes(&entry));
        self.entries.push(entry);
        // 超预算就丢最旧的条目，直到回到预算内（至少留一条，避免反复解码同一张）。
        while self.bytes > budget && self.entries.len() > 1 {
            let dropped = self.entries.remove(0);
            self.bytes = self.bytes.saturating_sub(entry_bytes(&dropped));
        }
    }

    fn bytes(&self) -> usize {
        self.bytes
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
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
