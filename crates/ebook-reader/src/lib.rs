//! 平台无关的阅读器核心。
//!
//! 这一层只做三件事：持有书与字体、排版、在排版结果上做交互查询。
//! 它不知道宿主是浏览器、iOS App 还是命令行；没有 JSON、没有全局单例、
//! 不把整本书 `Box::leak` 出去。
//!
//! 适配层：
//! - `ebook-web`：wasm + JSON，保持原有 JS 宿主的行为不变；
//! - `ebook-abi`：C ABI 静态库，给 native（iOS/Android/桌面）用。

mod engine;
mod heights;
mod types;

pub use engine::{insets, Engine};
pub use types::{
    Bitmap, EngineError, Limits, Locator, LocatorRange, MemoryUsage, PageBox, ScrollChapter,
    ScrollLoc, ScrollMetrics, SearchHit, SearchResults, SelectionResult, Style, Viewport,
};

pub use ebook_format::TocEntry;
pub use ebook_ir::{
    default_text_color, Anchor, AnchorRange, BlockBox, EdgesF, LayoutResult, Location, NodeId,
};
pub use ebook_layout::SelectionRect;
pub use ebook_layout::{BlockWindow, Mode};
pub use ebook_render;
