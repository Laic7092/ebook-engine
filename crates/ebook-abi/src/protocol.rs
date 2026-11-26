//! C ABI 的请求/响应形状。
//!
//! 与 wasm 的 `ebook-web::protocol` 是同一份思路（postcard + 枚举），但这边
//! 没有 JSON、没有"当前书"这种隐含状态：除了书本身，每次调用都把参数带全。
//! 这样调用方不需要维护"我上次设置的 viewport 是什么"。

use ebook_reader::{Anchor, AnchorRange, BlockBox};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    pub insets: Insets,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Style {
    pub font_scale: f32,
    pub line_scale: f32,
    pub letter_px: f32,
    pub word_px: f32,
    pub align: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Loc {
    pub page: u32,
    pub line: u32,
    pub byte: u32,
}

/// 滚动会话里的一章：起点、高度、成没成。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ScrollChapter {
    pub spine: u32,
    pub top: f32,
    pub height: f32,
    pub ok: bool,
    /// `height` 还是估计值（还没有实测）。
    pub estimated: bool,
}

/// 高度索引状态：总高、已实测章数、总章数。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ScrollMetrics {
    pub total: f32,
    pub measured: u32,
    pub count: u32,
}

/// 滚动模式下的命中：章 + 章内位置 + 该行在全书流坐标里的 y。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ScrollLoc {
    /// `-1` = 没有命中。
    pub spine: i32,
    pub line: u32,
    pub byte: u32,
    pub y: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub index: u32,
    pub width: f32,
    pub height: f32,
    pub insets: Insets,
    pub line_from: u32,
    pub line_len: u32,
    pub progress: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpineItem {
    pub index: u32,
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub linear: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FontFace {
    pub family: String,
    pub weight: u16,
    pub href: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TocEntry {
    pub label: String,
    /// `-1` = 不在阅读顺序里（封面、注释页）。
    pub spine: i32,
    pub fragment: Option<String>,
    pub depth: u16,
}

/// 一个 `dc:creator`。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Creator {
    pub name: String,
    pub role: Option<String>,
    pub file_as: Option<String>,
}

/// 书目元数据。和结构（spine/fonts）分开，宿主切书、更新封面时不必重传目录。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub title: Option<String>,
    pub language: Option<String>,
    /// 第一作者，等价于 `creators.first()`；单作者宿主直接用。
    pub creator: Option<String>,
    /// 全部作者，按 OPF 顺序。
    pub creators: Vec<Creator>,
    pub publisher: Option<String>,
    pub identifier: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub cover: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub spine: i32,
    pub fragment: Option<String>,
    pub external: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SelectionRect {
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selection {
    pub start: Loc,
    pub end: Loc,
    pub rects: Vec<SelectionRect>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    /// 一次调用里带全排版参数：ABI 层不保存 viewport/style。
    Layout {
        spine: u32,
        viewport: Viewport,
        style: Style,
    },
    Bitmap {
        page: u32,
        scale: f32,
        ink: u32,
        paper: u32,
        bold: bool,
    },
    Hit {
        page: u32,
        x: f32,
        y: f32,
    },
    /// 页内点 → 不依赖像素的原文锚点（章 + DOM 路径 + 字节偏移）。
    AnchorAt {
        page: u32,
        x: f32,
        y: f32,
    },
    /// 稳定锚点 → 当前排版里的页/行/字节。
    Locate {
        anchor: Anchor,
    },
    /// 两个页内位置 → 稳定区间。
    AnchorRange {
        a: Loc,
        b: Loc,
    },
    /// 最近一次排版的块表。
    Blocks,
    /// 只排一章里的一个块窗口（`from..=to` 的文档节点子树）。
    BlockWindow {
        spine: u32,
        from: u32,
        to: u32,
        viewport: Viewport,
        style: Style,
    },
    LinkAt {
        page: u32,
        x: f32,
        y: f32,
    },
    Selection {
        a: Loc,
        b: Loc,
    },
    Progress {
        loc: Loc,
    },
    FragmentLine {
        fragment: String,
    },
    ResolveLink {
        href: String,
    },
    RegisterFont {
        bytes: Vec<u8>,
    },
    RegisterFontFace {
        index: u32,
    },
    BaseFontSize,
    Metadata,
    Toc,
    Pages,
    Limits {
        max_page_pixels: u64,
        image_cache_bytes: u32,
    },
    /// 连不通时的探活（也让 ABI 的编码/释放路径有个便宜的测试）。
    Ping,
    /// 开始（或重设）跨章连续滚动会话。上下留白由引擎置 0。
    Scroll {
        viewport: Viewport,
        style: Style,
    },
    /// 全书章节表（会量完全书；想分摊开销就用 `ScrollChapter` 逐章取）。
    ScrollChapters,
    /// 全书章节表，但不排版：未实测的章用估计高度。
    ScrollChaptersEstimated,
    /// 高度索引状态；不触发排版。
    ScrollMetrics,
    /// 导出高度索引（实测值 + 键）。
    ScrollIndexExport,
    /// 导入高度索引；`false` = 键/章数不匹配。
    ScrollIndexImport {
        bytes: Vec<u8>,
    },
    /// 流式书：尾部追加一章。
    AppendChapter {
        title: String,
        text: String,
    },
    /// 流式书：替换一章。
    ReplaceChapter {
        spine: u32,
        title: String,
        text: String,
    },
    /// 流式书：清空所有章节。
    ClearChapters,
    /// 章节内容版本；`None` = 没有这一章/不是流式书。
    ChapterRev {
        spine: u32,
    },
    ScrollChapter {
        index: u32,
    },
    /// 全书流坐标总长。
    ScrollHeight,
    /// 全书流坐标落在哪一章。
    ScrollSpineAt {
        y: f32,
    },
    /// 全书流坐标的一段（可能跨章）。
    ScrollBand {
        y: f32,
        height: f32,
        scale: f32,
        ink: u32,
        paper: u32,
        bold: bool,
    },
    /// 全书流坐标 → 原文位置。
    ScrollHit {
        x: f32,
        y: f32,
    },
    /// 行号 → 全书流坐标。
    ScrollLineY {
        spine: u32,
        line: u32,
    },
    /// `#fragment` → 全书流坐标。
    ScrollAnchorY {
        spine: u32,
        fragment: String,
    },
    /// 全书流坐标 → 进度 0..=1。
    ScrollProgress {
        y: f32,
    },
    /// 阅读顺序（manifest 过滤后的线性项）。
    Spine,
    /// 书内嵌字体面。
    Fonts,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    /// 成功但没有返回值。
    Ok,
    Pong,
    Metadata(Metadata),
    Toc(Vec<TocEntry>),
    Pages(Vec<Page>),
    Bitmap {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Hit(Option<Loc>),
    Anchor(Option<Anchor>),
    AnchorRange(Option<AnchorRange>),
    Blocks(Vec<BlockBox>),
    Link(Option<String>),
    Selection(Option<Selection>),
    Progress(Option<f32>),
    Line(Option<u32>),
    Target(Option<Link>),
    Font(u32),
    Number(f32),
    Error(String),
    /// 章节表（`Scroll` / `ScrollChapters`）。
    ScrollChapters(Vec<ScrollChapter>),
    /// 一章；`None` = 下标越界。
    ScrollChapter(Option<ScrollChapter>),
    /// 全书流坐标落在哪一章。
    ScrollSpine(Option<u32>),
    /// 滚动模式的命中。
    ScrollHit(Option<ScrollLoc>),
    /// 位置（行/锚点）换算出的全书流坐标。
    Position(Option<f32>),
    /// 章数（append 之后）。
    Count(u32),
    /// 章节内容版本。
    Rev(u64),
    /// 高度索引状态。
    ScrollMetrics(ScrollMetrics),
    /// 高度索引字节。
    Index(Vec<u8>),
    /// 布尔结果（例如 import 是否匹配）。
    Flag(bool),
    /// 阅读顺序。
    Spine(Vec<SpineItem>),
    /// 内嵌字体面。
    Fonts(Vec<FontFace>),
}
