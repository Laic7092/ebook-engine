//! C ABI 的请求/响应形状。
//!
//! 与 wasm 的 `ebook-web::protocol` 是同一份思路（postcard + 枚举），但这边
//! 没有 JSON、没有"当前书"这种隐含状态：除了书本身，每次调用都把参数带全。
//! 这样调用方不需要维护"我上次设置的 viewport 是什么"。

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Book {
    pub title: Option<String>,
    pub language: Option<String>,
    pub creator: Option<String>,
    pub publisher: Option<String>,
    pub identifier: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub cover: Option<String>,
    pub spine: Vec<SpineItem>,
    pub fonts: Vec<FontFace>,
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
    Book,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    /// 成功但没有返回值。
    Ok,
    Pong,
    Book(Book),
    Toc(Vec<TocEntry>),
    Pages(Vec<Page>),
    Bitmap {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Hit(Option<Loc>),
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
}
