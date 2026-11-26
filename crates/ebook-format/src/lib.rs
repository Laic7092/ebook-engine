#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod css;
mod entities;
pub mod epub;
mod html_repair;
pub mod image;
pub mod txt;
pub mod xhtml;
pub mod zip;

/// 字节 → 文本的解码入口（UTF-8/UTF-16/GBK/Big5…）。
///
/// wasm 上没有完整编码表，宿主先用 `TextDecoder` 解好再走 `open_text`；
/// native 直接用这个入口，格式层与 web 端因此保持同一套文本语义。
pub use zip::decode_text;

use alloc::string::String;
use alloc::vec::Vec;

use ebook_ir::{Anchor, FontStyle, NodeId, StyledDoc};

/// 目录项。`spine` 是 `Book::spine` 的下标；目标不在阅读顺序里时等于
/// [`TocEntry::NO_SPINE`]（例如封面或 `linear="no"` 的注释页）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TocEntry {
    pub label: String,
    /// 归档内路径，已按导航文档所在目录解析，不含 fragment。
    pub href: String,
    pub spine: usize,
    pub fragment: Option<String>,
    /// 0 起算的嵌套层级。
    pub depth: u16,
}

impl TocEntry {
    pub const NO_SPINE: usize = usize::MAX;

    pub fn in_spine(&self) -> bool {
        self.spine != Self::NO_SPINE
    }
}

/// 书内链接的落点。`external` 为真时 `spine` 无意义（固定为 0），宿主应当交给
/// 自己的 URL 打开器。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkTarget {
    pub spine: usize,
    pub fragment: Option<String>,
    pub external: bool,
}

/// 一章里收集到的一个 `<a href>`。IR 的 `Node` 没有链接字段，所以先放在格式层
/// 的 side table 里；等 `Node.link` 落地后可以直接搬过去。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterLinks {
    pub node: NodeId,
    pub href: String,
}

/// 章节里收集到的一个带 `id` 的元素。fragment 跳转靠它把 `#id` 换成
/// `Anchor`；IR 里没有 id 表，所以由格式层在解析这一章时一并返回。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterAnchor {
    pub id: String,
    pub anchor: Anchor,
}

/// OPF manifest 里的一个资产。`href` 是归档内路径（相对路径已解析）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asset {
    pub href: String,
    pub media_type: String,
}

/// manifest 里的一个图片资产。
///
/// 这里**没有** `ImageId`：`ImageId` 是章内 `StyledDoc::images` 的下标，只有
/// 章节上下文才知道；manifest 顺序与章内出现顺序是两套编号，混用会让同一张图
/// 指向另一张。图片字节统一按 `ImageMeta::src`（归档内路径）查找。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageAsset {
    pub href: String,
    pub media_type: String,
}

/// 书里内嵌的字体。字节来自 zip，宿主直接喂给 `ebook-font`，排版层不自己找字体。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmbeddedFont {
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
    /// 归档内路径，已按声明它的 CSS 文件所在目录解析。
    pub href: String,
    pub bytes: Vec<u8>,
}

/// 格式层的诊断码。坏输入不 panic：要么 `Err`，要么带 issue 的部分结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpubIssueCode {
    /// spine 的 `idref` 在 manifest 里找不到对应项。
    MissingSpineItem,
    /// manifest 声明了文件，但 zip 里没有（或解压失败）。
    MissingAsset,
    /// 章节 XHTML 解析失败。
    MalformedXhtml,
    /// 章节不是合法 XML，但已用 HTML 容错层恢复；内容基本保留。
    RepairedXhtml,
    /// 容错解析也失败，退化成纯文本；结构/样式/链接可能丢失。
    TextFallback,
    /// CSS 属性/值无法落到 IR，已丢弃。
    UnsupportedCss,
    /// EPUB3 导航文档缺失或无法解析。
    BadNav,
    /// EPUB2 NCX 缺失或无法解析。
    BadNcx,
    /// 同一章里出现了重复的 `id`。
    DuplicateId,
    /// 归档路径试图越过包根目录，已拒绝。
    UnsafePath,
    /// manifest 里的 id 或路径重复。
    DuplicateManifest,
    /// 导航文档引用了阅读顺序之外的路径。
    TocTargetMissing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpubIssue {
    pub code: EpubIssueCode,
    pub href: Option<String>,
    pub detail: String,
}

/// 一个 `dc:creator`。`name` 是显示名；`role`/`file_as` 保留 OPF 的
/// `opf:role` / `opf:file-as`，没有就是 `None`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Creator {
    pub name: String,
    pub role: Option<String>,
    pub file_as: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Book {
    pub title: Option<String>,
    pub lang: Option<String>,
    pub spine: Vec<SpineItem>,
    /// 第一作者，等价于 `creators.first().map(|c| &c.name)`；保留它是为了
    /// 单作者宿主不必先取数组。
    pub creator: Option<String>,
    /// 全部 `dc:creator`，按 OPF 出现顺序。
    pub creators: Vec<Creator>,
    pub publisher: Option<String>,
    pub identifier: Option<String>,
    /// 与 `lang` 同义，保留 `dc:language` 的原始字段名。
    pub language: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    /// 封面图片的归档内路径（EPUB2 的包装页会被跟到第一个 `<img>`）。
    pub cover_href: Option<String>,
    pub toc: Vec<TocEntry>,
    pub assets: Vec<Asset>,
    pub images: Vec<ImageAsset>,
    pub fonts: Vec<EmbeddedFont>,
    /// 打开阶段收集到的诊断。逐章解析新增的诊断在 `EpubReader::issues()`。
    pub issues: Vec<EpubIssue>,
}

impl Book {
    pub fn toc(&self) -> &[TocEntry] {
        &self.toc
    }

    pub fn fonts(&self) -> &[EmbeddedFont] {
        &self.fonts
    }

    pub fn issues(&self) -> &[EpubIssue] {
        &self.issues
    }

    pub fn asset(&self, href: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.href == href).or_else(|| {
            self.assets
                .iter()
                .find(|a| a.href.eq_ignore_ascii_case(href))
        })
    }

    pub fn image(&self, href: &str) -> Option<&ImageAsset> {
        self.images
            .iter()
            .find(|image| image.href == href)
            .or_else(|| {
                self.images
                    .iter()
                    .find(|image| image.href.eq_ignore_ascii_case(href))
            })
    }

    pub fn spine_index(&self, href: &str) -> Option<usize> {
        self.spine
            .iter()
            .position(|item| item.href == href)
            .or_else(|| {
                self.spine
                    .iter()
                    .position(|item| item.href.eq_ignore_ascii_case(href))
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpineItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub linear: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemorySource {
    files: Vec<(String, Vec<u8>)>,
}

impl MemorySource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: &str, bytes: Vec<u8>) {
        self.files.push((String::from(name), bytes));
    }

    pub fn insert_text(&mut self, name: &str, text: &str) {
        self.insert(name, text.as_bytes().to_vec());
    }
}

pub trait ByteSource {
    fn read(&self, name: &str) -> Option<Vec<u8>>;
    fn names(&self) -> Vec<&str>;
    fn document(&self) -> Option<Vec<u8>> {
        None
    }
    fn first_name(&self) -> Option<&str> {
        self.names().first().copied()
    }
}

impl ByteSource for MemorySource {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.clone())
    }

    fn names(&self) -> Vec<&str> {
        self.files.iter().map(|(n, _)| n.as_str()).collect()
    }

    fn document(&self) -> Option<Vec<u8>> {
        match self.files.as_slice() {
            [(_, bytes)] => Some(bytes.clone()),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    No,
    Maybe,
    Yes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    Unsupported(&'static str),
    Missing(&'static str),
    Malformed(&'static str),
    OutOfRange,
}

pub trait FormatAdapter: Sync {
    fn name(&self) -> &'static str;
    fn probe(&self, src: &dyn ByteSource) -> Probe;
    fn open(&self, src: &dyn ByteSource) -> Result<Book, FormatError>;
    fn chapter(
        &self,
        src: &dyn ByteSource,
        book: &Book,
        index: usize,
    ) -> Result<StyledDoc, FormatError>;
}

static TXT: txt::Txt = txt::Txt;
static EPUB: epub::Epub = epub::Epub;
static ADAPTERS: [&dyn FormatAdapter; 2] = [&TXT, &EPUB];

pub fn adapters() -> &'static [&'static dyn FormatAdapter] {
    &ADAPTERS
}

pub struct Opened {
    adapter: &'static dyn FormatAdapter,
    pub book: Book,
}

impl Opened {
    pub fn adapter_name(&self) -> &'static str {
        self.adapter.name()
    }

    pub fn chapter(&self, src: &dyn ByteSource, index: usize) -> Result<StyledDoc, FormatError> {
        self.adapter.chapter(src, &self.book, index)
    }

    pub fn len(&self) -> usize {
        self.book.spine.len()
    }

    pub fn is_empty(&self) -> bool {
        self.book.spine.is_empty()
    }
}

pub fn open(src: &dyn ByteSource) -> Result<Opened, FormatError> {
    let mut chosen: Option<&'static dyn FormatAdapter> = None;
    let mut best = Probe::No;
    for adapter in adapters() {
        let probe = adapter.probe(src);
        if probe == Probe::Yes {
            return Ok(Opened {
                adapter: *adapter,
                book: adapter.open(src)?,
            });
        }
        if probe == Probe::Maybe && best == Probe::No {
            best = probe;
            chosen = Some(*adapter);
        }
    }
    match chosen {
        Some(adapter) => Ok(Opened {
            adapter,
            book: adapter.open(src)?,
        }),
        None => Err(FormatError::Unsupported("no adapter")),
    }
}
