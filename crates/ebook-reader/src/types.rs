//! 宿主侧的类型。全部是值类型或纯数据，没有 JSON、没有全局状态。
//!
//! 适配层（wasm 的 `ebook-web`、C ABI 的 `ebook-abi`）负责把这些翻译成
//! 各自宿主方便的形状；核心与这里的类型不知道宿主是谁。

use ebook_ir::{EdgesF, Location};
use ebook_layout::SelectionRect;

/// 排版视口：页面区域（点）+ 四边内边距。
///
/// 内边距是"正文避开工具栏"用的，与页边距叠加；它不改变页面几何，
/// 所以工具栏显隐不需要重新排版。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    pub insets: EdgesF,
}

impl Viewport {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            insets: EdgesF::all(0.0),
        }
    }

    pub fn with_insets(mut self, insets: EdgesF) -> Self {
        self.insets = insets;
        self
    }

    /// 四边相同的页边距。
    pub fn with_margin(self, margin: f32) -> Self {
        self.with_insets(EdgesF::all(margin))
    }
}

/// 阅读设置。`font_scale` 只缩放文档字号/行高/间距，页面几何不变。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub font_scale: f32,
    /// 行距倍数（1.5 = 1.5em），覆盖书里写的 `line-height`。
    pub line_scale: f32,
    /// 额外字距（px），叠加在书的 `letter-spacing` 上。
    pub letter_px: f32,
    /// 额外词距（px），叠加在书的 `word-spacing` 上：中文没有词距，西文有。
    pub word_px: f32,
    /// 0=保留原书，1=起始，2=居中，3=末尾，4=两端。
    pub align: u32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            font_scale: 1.0,
            line_scale: 1.0,
            letter_px: 0.0,
            word_px: 0.0,
            align: 0,
        }
    }
}

impl Style {
    /// 把设置写进 `StyledDoc`。排版层仍然拿到一份普通的文档，不知道有"阅读器设置"。
    pub fn apply(&self, doc: &mut ebook_ir::StyledDoc) {
        scale_fonts(doc, self.font_scale);
        let line_scale = if self.line_scale.is_finite() && self.line_scale > 0.0 {
            self.line_scale
        } else {
            1.0
        };
        let letter_px = if self.letter_px.is_finite() {
            self.letter_px
        } else {
            0.0
        };
        let word_px = if self.word_px.is_finite() {
            self.word_px
        } else {
            0.0
        };
        let align = match self.align {
            1 => Some(ebook_ir::TextAlign::Start),
            2 => Some(ebook_ir::TextAlign::Center),
            3 => Some(ebook_ir::TextAlign::End),
            4 => Some(ebook_ir::TextAlign::Justify),
            _ => None,
        };
        for node in &mut doc.nodes {
            node.style.line_height = ebook_ir::LineHeight::Mul(line_scale);
            if letter_px.abs() > 1.0e-4 {
                node.style.text.letter_spacing += letter_px;
            }
            if word_px.abs() > 1.0e-4 {
                node.style.text.word_spacing += word_px;
            }
            if let Some(align) = align {
                node.style.text.align = align;
            }
        }
    }
}

fn scale_fonts(doc: &mut ebook_ir::StyledDoc, scale: f32) {
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

/// 一页的几何、行区间和本章进度。宿主靠它翻页、画进度条，不需要 IR。
#[derive(Clone, Copy, Debug)]
pub struct PageBox {
    pub index: u32,
    pub width: f32,
    pub height: f32,
    pub inset: EdgesF,
    pub line_from: u32,
    pub line_len: u32,
    /// 本章内进度 0..1。
    pub progress: f32,
}

/// 滚动会话里的一章：起点、内容高度、这章排没排成功。
#[derive(Clone, Copy, Debug)]
pub struct ScrollChapter {
    /// 章节在 spine 里的下标。
    pub spine: usize,
    /// 全章在全书流坐标里的起点（当前高度索引里前面所有章的高度之和）。
    pub top: f32,
    /// 本章高度：实测值按视口高向上取整（至少一屏）；还没实测时是估计值。
    pub height: f32,
    /// 排版成功为 `true`；失败的章高度按 0 计，不会拖垮后面的章。
    pub ok: bool,
    /// `true` 表示 `height` 还是估计值，宿主可以再调 `scroll_chapter` 实测。
    pub estimated: bool,
}

/// 全书高度索引的当前状态。滚动条/进度条用它决定“要不要先接受一个估计值”。
#[derive(Clone, Copy, Debug)]
pub struct ScrollMetrics {
    /// 当前高度索引里的全书总高（实测 + 估计）。
    pub total: f32,
    /// 已经实测过的章数。
    pub measured: u32,
    /// 总章数。
    pub count: u32,
}

/// 宿主可持久化的稳定位置：章 + 文档节点 + 节点内字节偏移。
///
/// 不依赖页号、行号、像素或滚动值；重排/换字号后仍然指向同一段原文。
/// 章节内容变化时用 `chapter_rev` 作废。`node` 是 `Anchor.path` 解析出的
/// 文档节点 id，C ABI 也能直接携带。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Locator {
    pub spine: u32,
    pub node: u32,
    pub offset: u32,
}

/// 稳定区间：两端都是 [`Locator`]。`start == end` 时可以当点用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocatorRange {
    pub start: Locator,
    pub end: Locator,
}

/// 滚动模式下的命中结果：全书的章 + 章内位置。
#[derive(Clone, Copy, Debug)]
pub struct ScrollLoc {
    /// 落在哪个 spine 下标。
    pub spine: usize,
    /// 全章行号（不是页内行号）。
    pub line: u32,
    /// 该行内的字节偏移。
    pub byte: u32,
    /// 该行行盒顶边在全书流坐标里的位置。
    pub y: f32,
}

/// 一张设备像素位图：`rgba` 是 `width * height * 4` 字节，行优先，非预乘。
#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// 选区：规范化后的端点 + 页内矩形 + 原文。
#[derive(Clone, Debug)]
pub struct SelectionResult {
    pub start: Location,
    pub end: Location,
    pub rects: Vec<SelectionRect>,
    pub text: String,
}

/// 全文搜索的一条命中。位置是稳定的 [`LocatorRange`]，与字号、页高无关；
/// 宿主先排版目标章，再用 [`Engine::locate_locator`](crate::Engine::locate_locator)
/// 换算成页/行。`before` / `text` / `after` 是已经做空白折叠的上下文，
/// 可以直接拼给 UI 高亮。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    /// 命中在原文里的区间；`start.node` 就是命中所在的文本节点。
    pub range: LocatorRange,
    /// 命中所在章在 spine 里的下标。
    pub spine: usize,
    /// 目录里的章名（该章没有独立目录项时为 `None`）。
    pub chapter: Option<String>,
    pub before: String,
    pub text: String,
    pub after: String,
    pub ellipsis_before: bool,
    pub ellipsis_after: bool,
}

/// 一次全文搜索的结果。单章解析失败不会中断整本书：计入 `skipped` 后继续。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// 命中数超过 `max_hits`，尾部没有返回。
    pub truncated: bool,
    /// 实际扫过的章数（含解析失败的章）。
    pub scanned: u32,
    /// 解析失败、无法搜索的章数。
    pub skipped: u32,
}

/// 当前引擎持有的可释放内存估算。宿主内存警告时先看这里，再决定是否
/// 调 [`Engine::trim_memory`](crate::Engine::trim_memory)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryUsage {
    /// 当前章 IR。
    pub layout_bytes: usize,
    /// 滚动会话跨章保留的 IR。
    pub chapters_bytes: usize,
    /// 断行/整形前段缓存。
    pub flow_bytes: usize,
    /// 解码后的图片 RGBA。
    pub image_bytes: usize,
    /// 全文搜索索引。
    pub search_bytes: usize,
    /// 字体光栅/整形缓存。
    pub font_bytes: usize,
    /// 以上之和。
    pub total_bytes: usize,
}

/// 宿主的资源上限。native 与 wasm 的预算不同，所以是每个 `Engine` 的配置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// 单页像素上限，拦得住"把 scale 传成 1000"的调用。
    pub max_page_pixels: u64,
    /// 书级图片缓存上限（解码后的 RGBA 字节数）。
    pub image_cache_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // 4096×4096：1170×2532@3x ≈ 2.96M，余量足够。
            max_page_pixels: 4096 * 4096,
            image_cache_bytes: 24 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum EngineError {
    /// 还没有打开书。
    NoBook,
    /// 还没有排版，或者排版的章节已经被换掉。
    NoLayout,
    /// 还没有开始连续滚动会话（或者字体换过、会话已失效）。
    NoScroll,
    /// 页号越界。
    PageOutOfRange(u32),
    /// 章节下标越界。
    ChapterOutOfRange(u32),
    /// 格式层/章节解析失败。
    Chapter(String),
    /// 光栅化失败。
    Render(String),
    /// 请求的位图超过 `Limits::max_page_pixels`。
    TooLarge { width: u32, height: u32 },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::NoBook => write!(f, "没有打开的书"),
            EngineError::NoLayout => write!(f, "还没有排版"),
            EngineError::NoScroll => write!(f, "还没有开始滚动会话"),
            EngineError::PageOutOfRange(page) => write!(f, "页号越界: {page}"),
            EngineError::ChapterOutOfRange(spine) => write!(f, "章节下标越界: {spine}"),
            EngineError::Chapter(message) => write!(f, "章节解析失败: {message}"),
            EngineError::Render(message) => write!(f, "光栅化失败: {message}"),
            EngineError::TooLarge { width, height } => {
                write!(f, "位图太大: {width}×{height}")
            }
        }
    }
}

impl std::error::Error for EngineError {}
