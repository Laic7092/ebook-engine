use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{OnceCell, RefCell};

use ebook_ir::{SizeF, StyledDoc};

use crate::css;
use crate::xhtml::{self, AssetResolver};
use crate::zip::ZipArchive;
use crate::{
    Asset, Book, ByteSource, ChapterAnchor, ChapterLinks, EmbeddedFont, EpubIssue, EpubIssueCode,
    FormatAdapter, FormatError, ImageAsset, LinkTarget, Probe, SpineItem, TocEntry,
};

/// 单个字体文件的上限。内嵌字体是排版入参，过大就直接当坏资产报告。
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
/// 包文档（container/OPF）与导航文档的上限。正常书远小于这些值；限额只用来
/// 挡住病态输入，触限时返回可读错误或跳过该导航源。
const MAX_PACKAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_NAV_BYTES: usize = 16 * 1024 * 1024;
const MAX_CHAPTER_BYTES: usize = 32 * 1024 * 1024;
const MAX_XML_NODES: u32 = 1_000_000;
const MAX_XML_DEPTH: usize = 128;
/// 目录递归与条目上限。深度上限优先于 XML 解析器的节点上限，避免栈溢出。
const MAX_TOC_DEPTH: u16 = 64;
const MAX_TOC_ENTRIES: usize = 100_000;
const MAX_NAV_DOCS: usize = 32;
const MAX_COVER_CANDIDATES: usize = 32;
const MAX_COVER_BYTES: usize = 2 * 1024 * 1024;
const MAX_FONTS: usize = 1_024;

pub struct Epub;

pub struct EpubReader<'a> {
    zip: ZipArchive<'a>,
    book: Book,
    opf_path: String,
    /// 资产字节的惰性缓存。键是 manifest 里的归档路径，`None` 表示读不出来。
    cells: BTreeMap<String, OnceCell<Option<Vec<u8>>>>,
    issues: RefCell<Vec<EpubIssue>>,
}

impl<'a> EpubReader<'a> {
    pub fn open(raw: &'a [u8]) -> Result<Self, FormatError> {
        let zip = ZipArchive::parse(raw).map_err(|_| FormatError::Malformed("zip"))?;
        let mut issues = Vec::new();
        for name in zip.rejected() {
            push_issue(
                &mut issues,
                EpubIssueCode::UnsafePath,
                Some(name.clone()),
                "archive entry escapes the package root and was ignored",
            );
        }

        // container.xml 缺失或指向不存在的 OPF 时，退一步在归档里找一个 .opf。
        // 这比整本书白屏更接近用户预期，同时把降级原因作为 issue 带出去。
        let from_container = container_opf(&zip);
        let opf_path = match from_container {
            Some(path) if zip.has(&path) => path,
            Some(path) => {
                push_issue(
                    &mut issues,
                    EpubIssueCode::MissingAsset,
                    Some(path),
                    "container.xml points at a missing OPF; falling back to an archive scan",
                );
                find_package(&zip).ok_or(FormatError::Missing("opf"))?
            }
            None => match find_package(&zip) {
                Some(path) => {
                    push_issue(
                        &mut issues,
                        EpubIssueCode::MissingAsset,
                        Some(String::from("META-INF/container.xml")),
                        "container.xml is missing or malformed; falling back to an archive scan",
                    );
                    path
                }
                None => return Err(FormatError::Missing("container.xml")),
            },
        };
        let opf = zip
            .read_text(&opf_path)
            .filter(|text| text.len() <= MAX_PACKAGE_BYTES)
            .ok_or(FormatError::Missing("opf"))?;

        let mut package = parse_package(&opf, &opf_path, &mut issues)?;
        let mut book = core::mem::take(&mut package.book);

        let mut cells = BTreeMap::new();
        collect_assets(&zip, &package, &mut book, &mut cells, &mut issues);
        book.cover_href = find_cover(&zip, &package, &book);
        collect_fonts(&zip, &package, &mut book, &mut issues);
        collect_toc(&zip, &package, &mut book, &mut issues);
        book.issues = issues.clone();

        Ok(Self {
            zip,
            book,
            opf_path,
            cells,
            issues: RefCell::new(issues),
        })
    }

    pub fn book(&self) -> &Book {
        &self.book
    }

    /// 打开阶段与逐章解析收集到的诊断，按首次出现去重。
    pub fn issues(&self) -> Vec<EpubIssue> {
        self.issues.borrow().clone()
    }

    pub fn chapter(&self, index: usize) -> Result<StyledDoc, FormatError> {
        self.chapter_with_links(index).map(|(doc, _)| doc)
    }

    /// 与 `chapter` 相同，但把 `<a href>` 与 `id → Anchor` 的 side table 一起返回。
    /// IR 里没有链接字段，也故意不存 id 表（那属于格式层），所以由这里一并带出。
    pub fn chapter_with_anchors(
        &self,
        index: usize,
    ) -> Result<(StyledDoc, Vec<ChapterLinks>, Vec<ChapterAnchor>), FormatError> {
        let href = self.book.spine.get(index).map(|item| item.href.clone());
        let built = match build_chapter(&self.zip, &self.book, index) {
            Ok(built) => built,
            Err(err) => {
                match err {
                    FormatError::Malformed(_) => self.record(
                        EpubIssueCode::MalformedXhtml,
                        href,
                        "chapter XHTML is malformed or exceeds the format layer limits",
                    ),
                    FormatError::Missing("chapter") => self.record(
                        EpubIssueCode::MissingAsset,
                        href,
                        "chapter file is missing from the archive or could not be decompressed",
                    ),
                    _ => {}
                }
                return Err(err);
            }
        };
        for name in &built.unsupported_css {
            self.record(
                EpubIssueCode::UnsupportedCss,
                href.clone(),
                &format!("CSS property {name} cannot be represented in the IR and was dropped"),
            );
        }
        for id in &built.duplicate_ids {
            self.record(
                EpubIssueCode::DuplicateId,
                href.clone(),
                &format!("duplicate element id {id}"),
            );
        }
        if built.repaired {
            self.record(
                EpubIssueCode::RepairedXhtml,
                href.clone(),
                "chapter is not well-formed XML; parsed with the HTML-tolerant repair pass",
            );
        }
        if built.text_fallback {
            self.record(
                EpubIssueCode::TextFallback,
                href,
                "chapter XHTML could not be parsed; only plain text was kept",
            );
        }
        Ok((built.doc, built.links, built.anchors))
    }

    /// 与 `chapter` 相同，但把 `<a href>` 的 side table 一起返回。
    pub fn chapter_with_links(
        &self,
        index: usize,
    ) -> Result<(StyledDoc, Vec<ChapterLinks>), FormatError> {
        self.chapter_with_anchors(index)
            .map(|(doc, links, _)| (doc, links))
    }

    /// 读取一个 manifest 资产，返回字节与 media type。
    ///
    /// 相对路径按包根目录（OPF 所在目录）解析；查找对大小写不敏感，与 zip 读取
    /// 的兜底一致。
    pub fn asset(&self, href: &str) -> Option<(&[u8], &str)> {
        let asset = self.book.asset(href).or_else(|| {
            if is_external(href) {
                None
            } else {
                self.book.asset(&resolve_href(&self.opf_path, href))
            }
        })?;
        let key = asset.href.clone();
        let cell = self.cells.get(&key)?;
        let bytes = cell.get_or_init(|| self.zip.read(&key));
        match bytes.as_ref() {
            Some(bytes) => Some((bytes.as_slice(), asset.media_type.as_str())),
            None => {
                self.record(
                    EpubIssueCode::MissingAsset,
                    Some(key),
                    "asset is missing from the archive or could not be decompressed",
                );
                None
            }
        }
    }

    /// 把书内 href 解析到 `spine` + `fragment`。没有章节上下文，因此 `#id`
    /// 形式会取第一个包含该 id 的章节。
    pub fn resolve_link(&self, href: &str) -> Option<LinkTarget> {
        let href = href.trim();
        if is_external(href) {
            return Some(LinkTarget {
                spine: 0,
                fragment: fragment_of(href),
                external: true,
            });
        }
        if let Some(fragment) = same_document_fragment(href) {
            let spine = self.spine_with_id(&fragment)?;
            return Some(LinkTarget {
                spine,
                fragment: Some(fragment),
                external: false,
            });
        }
        let (path, fragment) = split_href(&self.opf_path, href);
        // 先按包根目录解析；再退一步接受已经解析好的归档路径。
        let spine = self.book.spine_index(&path).or_else(|| {
            let bare = href.split(['#', '?']).next().unwrap_or("");
            self.book.spine_index(&percent_decode(bare))
        })?;
        Some(LinkTarget {
            spine,
            fragment,
            external: false,
        })
    }

    /// 在某一章的上下文里解析链接：`#id` 指向本章，`../` 从本章目录起算。
    pub fn resolve_link_from(&self, spine: usize, href: &str) -> Option<LinkTarget> {
        let href = href.trim();
        if is_external(href) {
            return Some(LinkTarget {
                spine: 0,
                fragment: fragment_of(href),
                external: true,
            });
        }
        if let Some(fragment) = same_document_fragment(href) {
            self.book.spine.get(spine)?;
            return Some(LinkTarget {
                spine,
                fragment: Some(fragment),
                external: false,
            });
        }
        let base = self
            .book
            .spine
            .get(spine)
            .map(|item| item.href.as_str())
            .unwrap_or(self.opf_path.as_str());
        let (path, fragment) = split_href(base, href);
        let target = self.book.spine_index(&path)?;
        Some(LinkTarget {
            spine: target,
            fragment,
            external: false,
        })
    }

    /// 解析 CSS 的 `@font-face`（供 `Book::fonts` 之外的按需查询使用）。
    pub fn font_faces(&self, css_href: &str) -> Vec<css::FontFace> {
        let Some(text) = self.zip.read_text(css_href) else {
            return Vec::new();
        };
        css::parse_font_faces(&text)
    }

    fn spine_with_id(&self, id: &str) -> Option<usize> {
        self.book
            .spine
            .iter()
            .enumerate()
            .find_map(|(index, item)| {
                let text = self.zip.read_text(&item.href)?;
                if text.len() > MAX_CHAPTER_BYTES {
                    return None;
                }
                let expanded = crate::entities::normalized(&text);
                if crate::entities::xml_too_deep(&expanded, MAX_XML_DEPTH) {
                    return None;
                }
                let options = roxmltree::ParsingOptions {
                    allow_dtd: true,
                    nodes_limit: MAX_XML_NODES,
                };
                let (found, _) =
                    crate::html_repair::parse_with_repair(&expanded, options, |doc| {
                        doc.descendants()
                            .any(|node| node.is_element() && attr(node, "id") == Some(id))
                    })?;
                found.then_some(index)
            })
    }

    fn record(&self, code: EpubIssueCode, href: Option<String>, detail: &str) {
        let issue = EpubIssue {
            code,
            href,
            detail: String::from(detail),
        };
        let mut issues = self.issues.borrow_mut();
        if !issues.iter().any(|existing| {
            existing.code == issue.code
                && existing.href == issue.href
                && existing.detail == issue.detail
        }) {
            issues.push(issue);
        }
    }
}

impl FormatAdapter for Epub {
    fn name(&self) -> &'static str {
        "epub"
    }

    fn probe(&self, src: &dyn ByteSource) -> Probe {
        let Some(raw) = src.document() else {
            return Probe::No;
        };
        if !raw.starts_with(b"PK\x03\x04") {
            return Probe::No;
        }
        let Ok(zip) = ZipArchive::parse(&raw) else {
            return Probe::Maybe;
        };
        match zip.read("mimetype") {
            Some(bytes) if bytes.starts_with(b"application/epub+zip") => Probe::Yes,
            _ => Probe::Maybe,
        }
    }

    fn open(&self, src: &dyn ByteSource) -> Result<Book, FormatError> {
        let raw = src.document().ok_or(FormatError::Missing("epub"))?;
        let reader = EpubReader::open(&raw)?;
        Ok(reader.book)
    }

    fn chapter(
        &self,
        src: &dyn ByteSource,
        book: &Book,
        index: usize,
    ) -> Result<StyledDoc, FormatError> {
        let raw = src.document().ok_or(FormatError::Missing("epub"))?;
        let reader = EpubReader::open(&raw)?;
        let _ = book;
        reader.chapter(index)
    }
}

fn build_chapter(
    zip: &ZipArchive<'_>,
    book: &Book,
    index: usize,
) -> Result<xhtml::Built, FormatError> {
    let item = book.spine.get(index).ok_or(FormatError::OutOfRange)?;
    let text = zip
        .read_text(&item.href)
        .ok_or(FormatError::Missing("chapter"))?;
    let mut resolver = ZipAssets {
        zip,
        base: dirname(&item.href).to_string(),
    };
    let mut built = match xhtml::build_document(&text, index as u32, &mut resolver) {
        Ok(built) => built,
        // 容错解析仍失败（通常意味着触到格式层硬上限）。最后一道兜底是
        // 纯文本：宁可丢结构，也不要整章白屏。
        Err(_) => xhtml::Built {
            doc: xhtml::text_document(&text, index as u32),
            links: Vec::new(),
            anchors: Vec::new(),
            duplicate_ids: Vec::new(),
            unsupported_css: Vec::new(),
            repaired: false,
            text_fallback: true,
        },
    };
    // 固有尺寸与 MIME 属于格式层：排版只认入参，不应该自己读图。
    // 同一张图可能在一章里出现很多次；按 src 缓存，避免反复解压与线性查 manifest。
    let mut mime_exact: BTreeMap<&str, &str> = BTreeMap::new();
    let mut mime_folded: BTreeMap<String, &str> = BTreeMap::new();
    for asset in &book.assets {
        mime_exact
            .entry(asset.href.as_str())
            .or_insert(&asset.media_type);
        mime_folded
            .entry(asset.href.to_ascii_lowercase())
            .or_insert(&asset.media_type);
    }
    let mut images: BTreeMap<String, (Option<SizeF>, Option<String>)> = BTreeMap::new();
    for meta in &mut built.doc.images {
        let Some(src) = meta.src.clone() else {
            continue;
        };
        let (size, mime) = images.entry(src.clone()).or_insert_with(|| {
            let bytes = zip.read(&src);
            let declared = mime_exact
                .get(src.as_str())
                .copied()
                .or_else(|| mime_folded.get(&src.to_ascii_lowercase()).copied());
            let size = bytes
                .as_ref()
                .and_then(|bytes| crate::image::image_size(bytes));
            let mime = declared.map(str::to_string).or_else(|| {
                bytes
                    .as_ref()
                    .and_then(|bytes| crate::image::image_mime(bytes))
                    .map(str::to_string)
            });
            (size, mime)
        });
        if meta.size.is_none() {
            meta.size = *size;
        }
        if meta.mime.is_none() {
            meta.mime = mime.clone();
        }
    }
    built.doc.meta.title = book.title.clone();
    built.doc.meta.lang = book.lang.clone();
    Ok(built)
}

struct ZipAssets<'a, 'b> {
    zip: &'a ZipArchive<'b>,
    base: String,
}

impl AssetResolver for ZipAssets<'_, '_> {
    fn css(&mut self, href: &str) -> Option<String> {
        self.zip.read_text(&join(&self.base, href))
    }

    fn resolve(&mut self, href: &str) -> Option<String> {
        let href = href.trim();
        if is_external(href) {
            return None;
        }
        let path = href.split(['#', '?']).next().unwrap_or("");
        let path = percent_decode(path);
        if path.is_empty() {
            return None;
        }
        Some(join(&self.base, &normalize_href(&path)))
    }
}

// ---------------------------------------------------------------------------
// 包文档
// ---------------------------------------------------------------------------

struct ManifestItem {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

struct Package {
    book: Book,
    manifest: Vec<ManifestItem>,
    spine_toc: Option<String>,
    guide: Vec<(String, String)>,
    meta_cover: Option<String>,
}

fn container_opf(zip: &ZipArchive<'_>) -> Option<String> {
    let text = zip.read_text("META-INF/container.xml")?;
    if text.len() > MAX_PACKAGE_BYTES {
        return None;
    }
    let text = super::entities::normalized(&text);
    if super::entities::xml_too_deep(&text, MAX_XML_DEPTH) {
        return None;
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 4_096,
    };
    let (path, _) = crate::html_repair::parse_with_repair(&text, options, |doc| {
        doc.descendants()
            .find(|n| tag_is(*n, "rootfile"))
            .and_then(|n| attr(n, "full-path"))
            .map(path_from_href)
            .filter(|s| !s.is_empty())
    })?;
    path
}

/// 没有可用的 container 时按归档内容找包文档：优先浅层路径，再按名字稳定排序。
fn find_package(zip: &ZipArchive<'_>) -> Option<String> {
    let mut best: Option<&str> = None;
    for name in zip.names() {
        if !name.to_ascii_lowercase().ends_with(".opf") {
            continue;
        }
        if name.eq_ignore_ascii_case("META-INF/container.xml") {
            continue;
        }
        let key = (name.matches('/').count(), name.len(), name);
        if best.is_none_or(|current| {
            let current_key = (current.matches('/').count(), current.len(), current);
            key < current_key
        }) {
            best = Some(name);
        }
    }
    best.map(str::to_string)
}

/// container.xml 的 `full-path` 也是 URL 路径：百分号解码、反斜杠归一化。
fn path_from_href(value: &str) -> String {
    let value = percent_decode(value.trim());
    join("", value.replace('\\', "/").trim_start_matches('/'))
}

fn parse_package(
    text: &str,
    opf_path: &str,
    issues: &mut Vec<EpubIssue>,
) -> Result<Package, FormatError> {
    if text.len() > MAX_PACKAGE_BYTES {
        return Err(FormatError::Malformed("opf"));
    }
    let text = super::entities::normalized(text);
    if super::entities::xml_too_deep(&text, MAX_XML_DEPTH) {
        return Err(FormatError::Malformed("opf"));
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: MAX_XML_NODES,
    };
    // OPF 也可能不是合法 XML；容错修复避免“一本坏 OPF 整本打不开”。
    let repaired_text: alloc::borrow::Cow<'_, str>;
    let doc = match roxmltree::Document::parse_with_options(&text, options) {
        Ok(doc) => doc,
        Err(_) => {
            repaired_text = crate::html_repair::repair(&text);
            roxmltree::Document::parse_with_options(&repaired_text, options)
                .map_err(|_| FormatError::Malformed("opf"))?
        }
    };

    let mut manifest: Vec<ManifestItem> = Vec::new();
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_href: BTreeMap<String, usize> = BTreeMap::new();
    for node in doc.descendants() {
        if !node.is_element() || node.tag_name().name() != "item" {
            continue;
        }
        let (Some(id), Some(href)) = (attr(node, "id"), attr(node, "href")) else {
            continue;
        };
        let resolved = resolve_href(opf_path, href);
        if by_id.contains_key(id) {
            push_issue(
                issues,
                EpubIssueCode::DuplicateManifest,
                Some(resolved.clone()),
                &format!("manifest id {id} is declared more than once"),
            );
            continue;
        }
        if by_href.contains_key(&resolved) {
            push_issue(
                issues,
                EpubIssueCode::DuplicateManifest,
                Some(resolved.clone()),
                "manifest href is declared more than once",
            );
        }
        by_id.insert(id.to_string(), manifest.len());
        by_href.entry(resolved.clone()).or_insert(manifest.len());
        manifest.push(ManifestItem {
            id: id.to_string(),
            href: resolved,
            media_type: attr(node, "media-type").unwrap_or("").to_string(),
            properties: attr(node, "properties").unwrap_or("").to_string(),
        });
    }

    let spine_node = doc.descendants().find(|n| tag_is(*n, "spine"));
    let spine_toc = spine_node
        .and_then(|n| attr(n, "toc"))
        .map(|s| s.to_string());

    let mut spine = Vec::new();
    for node in doc.descendants() {
        if !node.is_element() || node.tag_name().name() != "itemref" {
            continue;
        }
        let Some(idref) = attr(node, "idref") else {
            continue;
        };
        let Some(item) = by_id.get(idref).and_then(|&index| manifest.get(index)) else {
            push_issue(
                issues,
                EpubIssueCode::MissingSpineItem,
                None,
                &format!("spine item {idref} has no manifest entry and was skipped"),
            );
            continue;
        };
        spine.push(SpineItem {
            id: item.id.clone(),
            href: item.href.clone(),
            media_type: item.media_type.clone(),
            linear: !matches!(attr(node, "linear"), Some(value) if value.eq_ignore_ascii_case("no")),
        });
    }

    if spine.is_empty() {
        return Err(FormatError::Malformed("spine"));
    }

    let guide: Vec<(String, String)> = doc
        .descendants()
        .filter(|n| tag_is(*n, "reference"))
        .filter_map(|n| {
            let href = attr(n, "href")?;
            let kind = attr(n, "type").unwrap_or("");
            Some((kind.to_string(), resolve_href(opf_path, href)))
        })
        .collect();
    let meta_cover = doc
        .descendants()
        .filter(|n| tag_is(*n, "meta"))
        .find(|n| {
            attr(*n, "name")
                .map(|name| name.eq_ignore_ascii_case("cover"))
                .unwrap_or(false)
        })
        .and_then(|n| attr(n, "content"))
        .map(|s| s.trim().to_string());
    let unique_identifier = doc
        .root_element()
        .attribute("unique-identifier")
        .map(|s| s.to_string());

    let identifier = unique_identifier
        .as_deref()
        .and_then(|wanted| {
            doc.descendants()
                .find(|n| {
                    n.is_element() && tag_is(*n, "identifier") && attr(*n, "id") == Some(wanted)
                })
                .and_then(text_of)
        })
        .or_else(|| meta_text(&doc, &["identifier"]));
    let title = meta_text(&doc, &["title"]);
    let language = meta_text(&doc, &["language", "lang"]);
    let creator = meta_text(&doc, &["creator"]);
    let publisher = meta_text(&doc, &["publisher"]);
    let date = meta_text(&doc, &["date"]);
    let description = meta_text(&doc, &["description"]);

    Ok(Package {
        book: Book {
            title,
            lang: language.clone(),
            language,
            creator,
            publisher,
            identifier,
            date,
            description,
            spine,
            ..Book::default()
        },
        manifest,
        spine_toc,
        guide,
        meta_cover,
    })
}

fn meta_text(doc: &roxmltree::Document<'_>, names: &[&str]) -> Option<String> {
    doc.descendants()
        .find(|n| n.is_element() && names.contains(&n.tag_name().name()))
        .and_then(text_of)
}

fn text_of(node: roxmltree::Node<'_, '_>) -> Option<String> {
    node.text()
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

// ---------------------------------------------------------------------------
// 资产
// ---------------------------------------------------------------------------

fn collect_assets(
    zip: &ZipArchive<'_>,
    package: &Package,
    book: &mut Book,
    cells: &mut BTreeMap<String, OnceCell<Option<Vec<u8>>>>,
    issues: &mut Vec<EpubIssue>,
) {
    for item in &package.manifest {
        book.assets.push(Asset {
            href: item.href.clone(),
            media_type: item.media_type.clone(),
        });
        cells.insert(item.href.clone(), OnceCell::new());
        if item.media_type.to_ascii_lowercase().starts_with("image/") {
            book.images.push(ImageAsset {
                href: item.href.clone(),
                media_type: item.media_type.clone(),
            });
        }
        if !is_external(&item.href) && !zip.has(&item.href) {
            push_issue(
                issues,
                EpubIssueCode::MissingAsset,
                Some(item.href.clone()),
                "manifest item is missing from the archive",
            );
        }
    }
}

fn find_cover(zip: &ZipArchive<'_>, package: &Package, book: &Book) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    for item in &package.manifest {
        if has_property(&item.properties, "cover-image") {
            candidates.push(item.href.clone());
        }
    }
    if let Some(id) = &package.meta_cover {
        if let Some(item) = package.manifest.iter().find(|item| &item.id == id) {
            candidates.push(item.href.clone());
        }
    }
    for (kind, href) in &package.guide {
        if kind.eq_ignore_ascii_case("cover") {
            candidates.push(href.clone());
        }
    }

    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    let mut attempts = 0usize;
    for href in candidates {
        if attempts >= MAX_COVER_CANDIDATES {
            break;
        }
        if seen.insert(href.clone(), ()).is_some() {
            continue;
        }
        attempts += 1;
        if is_image_asset(book, &href) {
            return Some(href);
        }
        // EPUB2 的封面常是一个包装页：跟到它的第一张图。
        if let Some(text) = zip.read_text(&href) {
            if text.len() <= MAX_COVER_BYTES {
                if let Some(src) = first_img_src(&text) {
                    let resolved = resolve_href(&href, &src);
                    if is_image_asset(book, &resolved) || zip.has(&resolved) {
                        return Some(resolved);
                    }
                }
            }
        }
    }
    None
}

fn is_image_asset(book: &Book, href: &str) -> bool {
    book.asset(href)
        .map(|asset| asset.media_type.to_ascii_lowercase().starts_with("image/"))
        .unwrap_or(false)
}

fn first_img_src(text: &str) -> Option<String> {
    if text.len() > MAX_PACKAGE_BYTES {
        return None;
    }
    let expanded = crate::entities::normalized(text);
    if crate::entities::xml_too_deep(&expanded, MAX_XML_DEPTH) {
        return None;
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 100_000,
    };
    let (src, _) = crate::html_repair::parse_with_repair(&expanded, options, |doc| {
        doc.descendants()
            .find(|n| tag_is(*n, "img"))
            .and_then(|n| attr(n, "src"))
            .map(|s| s.to_string())
    })?;
    src
}

fn collect_fonts(
    zip: &ZipArchive<'_>,
    package: &Package,
    book: &mut Book,
    issues: &mut Vec<EpubIssue>,
) {
    let mut seen_css: BTreeMap<String, ()> = BTreeMap::new();
    for item in &package.manifest {
        if !item.media_type.eq_ignore_ascii_case("text/css") {
            continue;
        }
        if seen_css.insert(item.href.clone(), ()).is_some() {
            continue;
        }
        let Some(text) = zip.read_text(&item.href) else {
            continue;
        };
        for face in css::parse_font_faces(&text) {
            let Some(src) = face.src else {
                continue;
            };
            if is_external(&src) {
                continue;
            }
            let href = resolve_href(&item.href, &src);
            let Some(bytes) = zip.read(&href) else {
                push_issue(
                    issues,
                    EpubIssueCode::MissingAsset,
                    Some(href),
                    &format!(
                        "@font-face {} names a font that is missing from the archive",
                        face.family
                    ),
                );
                continue;
            };
            if bytes.len() > MAX_FONT_BYTES {
                push_issue(
                    issues,
                    EpubIssueCode::MissingAsset,
                    Some(href),
                    "embedded font is larger than the format layer accepts",
                );
                continue;
            }
            book.fonts.push(EmbeddedFont {
                family: face.family,
                weight: face.weight,
                style: face.style,
                href,
                bytes,
            });
            if book.fonts.len() >= MAX_FONTS {
                push_issue(
                    issues,
                    EpubIssueCode::MissingAsset,
                    None,
                    "embedded font count exceeds the format layer limit; remaining faces were skipped",
                );
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 目录
// ---------------------------------------------------------------------------

struct TocRow {
    label: String,
    path: String,
    fragment: Option<String>,
    depth: u16,
}

fn collect_toc(
    zip: &ZipArchive<'_>,
    package: &Package,
    book: &mut Book,
    issues: &mut Vec<EpubIssue>,
) {
    let mut rows: Vec<TocRow> = Vec::new();
    let mut nav_docs = 0usize;
    for item in &package.manifest {
        if !has_property(&item.properties, "nav") {
            continue;
        }
        if nav_docs >= MAX_NAV_DOCS {
            push_issue(
                issues,
                EpubIssueCode::BadNav,
                Some(item.href.clone()),
                "too many navigation documents; remaining candidates were skipped",
            );
            break;
        }
        nav_docs += 1;
        match zip.read_text(&item.href) {
            Some(text) => match parse_nav(&text, &item.href) {
                Ok(found) if !found.is_empty() => {
                    rows = found;
                    break;
                }
                Ok(_) => push_issue(
                    issues,
                    EpubIssueCode::BadNav,
                    Some(item.href.clone()),
                    "navigation document has no toc list",
                ),
                Err(_) => push_issue(
                    issues,
                    EpubIssueCode::BadNav,
                    Some(item.href.clone()),
                    "navigation document is not well-formed XHTML",
                ),
            },
            None => push_issue(
                issues,
                EpubIssueCode::BadNav,
                Some(item.href.clone()),
                "navigation document is missing from the archive",
            ),
        }
    }

    if rows.is_empty() {
        if let Some(id) = &package.spine_toc {
            match package.manifest.iter().find(|item| &item.id == id) {
                Some(item) => match zip.read_text(&item.href) {
                    Some(text) => match parse_ncx(&text, &item.href) {
                        Ok(found) if !found.is_empty() => rows = found,
                        Ok(_) => push_issue(
                            issues,
                            EpubIssueCode::BadNcx,
                            Some(item.href.clone()),
                            "NCX document has no navPoints",
                        ),
                        Err(_) => push_issue(
                            issues,
                            EpubIssueCode::BadNcx,
                            Some(item.href.clone()),
                            "NCX document is not well-formed XML",
                        ),
                    },
                    None => push_issue(
                        issues,
                        EpubIssueCode::BadNcx,
                        Some(item.href.clone()),
                        "NCX document is missing from the archive",
                    ),
                },
                None => push_issue(
                    issues,
                    EpubIssueCode::BadNcx,
                    None,
                    &format!("spine names NCX document {id} but the manifest has no such item"),
                ),
            }
        }
    }

    let mut spine_exact: BTreeMap<&str, usize> = BTreeMap::new();
    let mut spine_folded: BTreeMap<String, usize> = BTreeMap::new();
    for (index, item) in book.spine.iter().enumerate() {
        spine_exact.entry(item.href.as_str()).or_insert(index);
        spine_folded
            .entry(item.href.to_ascii_lowercase())
            .or_insert(index);
    }
    for row in rows {
        let folded = row.path.to_ascii_lowercase();
        let spine = spine_exact
            .get(row.path.as_str())
            .or_else(|| spine_folded.get(&folded))
            .copied()
            .unwrap_or(TocEntry::NO_SPINE);
        if spine == TocEntry::NO_SPINE {
            push_issue(
                issues,
                EpubIssueCode::TocTargetMissing,
                Some(row.path.clone()),
                "table of contents points outside the reading order",
            );
        }
        book.toc.push(TocEntry {
            label: row.label,
            href: row.path,
            spine,
            fragment: row.fragment,
            depth: row.depth,
        });
    }
}

fn parse_nav(text: &str, nav_path: &str) -> Result<Vec<TocRow>, ()> {
    if text.len() > MAX_NAV_BYTES {
        return Err(());
    }
    let expanded = crate::entities::normalized(text);
    if crate::entities::xml_too_deep(&expanded, MAX_XML_DEPTH) {
        return Err(());
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: MAX_XML_NODES,
    };
    let (out, _) = crate::html_repair::parse_with_repair(&expanded, options, |doc| {
        let mut out = Vec::new();
        let nav = doc
            .descendants()
            .find(|node| tag_is(*node, "nav") && is_toc_nav(*node));
        let Some(nav) = nav else {
            return out;
        };
        let list = nav
            .descendants()
            .find(|node| tag_is(*node, "ol") || tag_is(*node, "ul"));
        let Some(list) = list else {
            return out;
        };
        nav_list(list, nav_path, 0, &mut out);
        out
    })
    .ok_or(())?;
    Ok(out)
}

fn is_toc_nav(node: roxmltree::Node<'_, '_>) -> bool {
    let type_attr = attr(node, "type").unwrap_or("").to_ascii_lowercase();
    let role = attr(node, "role").unwrap_or("").to_ascii_lowercase();
    has_property(&type_attr, "toc") || has_property(&role, "doc-toc")
}

fn nav_list(list: roxmltree::Node<'_, '_>, base: &str, depth: u16, out: &mut Vec<TocRow>) {
    if depth > MAX_TOC_DEPTH || out.len() >= MAX_TOC_ENTRIES {
        return;
    }
    for item in list.children().filter(|node| tag_is(*node, "li")) {
        if out.len() >= MAX_TOC_ENTRIES {
            break;
        }
        let (label, href) = nav_label(item);
        let mut children = Vec::new();
        for child in item
            .children()
            .filter(|node| tag_is(*node, "ol") || tag_is(*node, "ul"))
        {
            nav_list(child, base, depth.saturating_add(1), &mut children);
        }
        if label.is_empty() && href.as_deref().unwrap_or("").is_empty() {
            append_toc(out, children);
            continue;
        }
        let (path, fragment) = match href {
            Some(href) => split_href(base, &href),
            None => (String::new(), None),
        };
        out.push(TocRow {
            label,
            path,
            fragment,
            depth,
        });
        append_toc(out, children);
    }
}

fn append_toc(out: &mut Vec<TocRow>, rows: Vec<TocRow>) {
    for row in rows {
        if out.len() >= MAX_TOC_ENTRIES {
            break;
        }
        out.push(row);
    }
}

fn nav_label(item: roxmltree::Node<'_, '_>) -> (String, Option<String>) {
    if let Some(link) = first_nav_link(item) {
        let href = attr(link, "href").map(|s| s.trim().to_string());
        return (collapse(&text_content(link)), href);
    }
    // 没有链接的分组 <li>：标签取自身文本，嵌套列表不算。
    (collapse(&text_content(item)), None)
}

/// `<li>` 里第一个不在嵌套列表中的 `<a>`。
fn first_nav_link<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
) -> Option<roxmltree::Node<'a, 'input>> {
    let mut stack: Vec<roxmltree::Node<'a, 'input>> =
        node.children().filter(|child| child.is_element()).collect();
    stack.reverse();
    while let Some(child) = stack.pop() {
        let name = child.tag_name().name();
        if matches!(name, "ol" | "ul") {
            continue;
        }
        if name == "a" {
            return Some(child);
        }
        stack.extend(child.children().filter(|child| child.is_element()).rev());
    }
    None
}

/// 元素里的全部文本，跳过嵌套的 `<ol>` / `<ul>`。
fn text_content(node: roxmltree::Node<'_, '_>) -> String {
    let mut out = String::new();
    let mut stack: Vec<roxmltree::Node<'_, '_>> = alloc::vec![node];
    while let Some(current) = stack.pop() {
        if current.is_text() {
            if let Some(text) = current.text() {
                out.push_str(text);
                out.push(' ');
            }
            continue;
        }
        let children: Vec<roxmltree::Node<'_, '_>> = current.children().collect();
        for child in children.into_iter().rev() {
            if child.is_element() && matches!(child.tag_name().name(), "ol" | "ul") {
                continue;
            }
            stack.push(child);
        }
    }
    out
}

fn parse_ncx(text: &str, ncx_path: &str) -> Result<Vec<TocRow>, ()> {
    if text.len() > MAX_NAV_BYTES {
        return Err(());
    }
    let expanded = crate::entities::normalized(text);
    if crate::entities::xml_too_deep(&expanded, MAX_XML_DEPTH) {
        return Err(());
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: MAX_XML_NODES,
    };
    let (out, _) = crate::html_repair::parse_with_repair(&expanded, options, |doc| {
        let mut out = Vec::new();
        let nav_map = doc.descendants().find(|node| tag_is(*node, "navMap"));
        let Some(nav_map) = nav_map else {
            return out;
        };
        ncx_points(nav_map, ncx_path, 0, &mut out);
        out
    })
    .ok_or(())?;
    Ok(out)
}

fn ncx_points(parent: roxmltree::Node<'_, '_>, base: &str, depth: u16, out: &mut Vec<TocRow>) {
    if depth > MAX_TOC_DEPTH || out.len() >= MAX_TOC_ENTRIES {
        return;
    }
    for point in parent.children().filter(|node| tag_is(*node, "navPoint")) {
        if out.len() >= MAX_TOC_ENTRIES {
            break;
        }
        let label = point
            .children()
            .find(|node| tag_is(*node, "navLabel"))
            .and_then(|label| label.children().find(|node| tag_is(*node, "text")))
            .and_then(|text| text.text())
            .map(collapse)
            .unwrap_or_default();
        let src = point
            .children()
            .find(|node| tag_is(*node, "content"))
            .and_then(|content| attr(content, "src"));
        let (path, fragment) = match src {
            Some(src) => split_href(base, src),
            None => (String::new(), None),
        };
        let mut children = Vec::new();
        ncx_points(point, base, depth.saturating_add(1), &mut children);
        if label.is_empty() && path.is_empty() {
            append_toc(out, children);
            continue;
        }
        out.push(TocRow {
            label,
            path,
            fragment,
            depth,
        });
        append_toc(out, children);
    }
}

// ---------------------------------------------------------------------------
// 路径与链接
// ---------------------------------------------------------------------------

fn same_document_fragment(href: &str) -> Option<String> {
    let href = href.trim();
    let rest = href.strip_prefix('#')?;
    let fragment = percent_decode(rest);
    if fragment.is_empty() {
        None
    } else {
        Some(fragment)
    }
}

fn fragment_of(href: &str) -> Option<String> {
    let (_, fragment) = href.split_once('#')?;
    let fragment = percent_decode(fragment);
    if fragment.is_empty() {
        None
    } else {
        Some(fragment)
    }
}

/// href 是否离开书：任何带 scheme 的 URL，或协议相对 `//host`。
fn is_external(href: &str) -> bool {
    let href = href.trim();
    if href.starts_with("//") {
        return true;
    }
    let mut chars = href.char_indices();
    match chars.next() {
        Some((_, c)) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for (index, c) in href.char_indices().skip(1) {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
            return false;
        }
        let _ = index;
    }
    false
}

/// 读取 CSS/XHTML 里的 URL 时统一做一次百分号解码。
fn percent_decode(value: &str) -> String {
    if !value.contains('%') {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let high = (bytes[i + 1] as char).to_digit(16);
            let low = (bytes[i + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 把 href 解析到归档路径：去掉 query/fragment，按 base 文档所在目录拼接。
fn resolve_href(base: &str, href: &str) -> String {
    let href = href.trim();
    let path = href.split(['#', '?']).next().unwrap_or("");
    let path = percent_decode(path);
    if path.is_empty() {
        return normalize_href(base);
    }
    join(dirname(base), &normalize_href(&path))
}

/// 拆出归档路径与 fragment；没有路径部分时指向 base 文档自身。
fn split_href(base: &str, href: &str) -> (String, Option<String>) {
    let href = href.trim();
    let (path, fragment) = match href.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (href, None),
    };
    let path = path.split('?').next().unwrap_or("");
    let path = percent_decode(path);
    let path = if path.is_empty() {
        normalize_href(base)
    } else {
        join(dirname(base), &normalize_href(&path))
    };
    let fragment = fragment
        .map(percent_decode)
        .filter(|fragment| !fragment.is_empty());
    (path, fragment)
}

fn normalize_href(value: &str) -> String {
    let value = value.trim().trim_start_matches('/');
    if value.contains('\\') {
        value.replace('\\', "/")
    } else {
        value.to_string()
    }
}

fn has_property(properties: &str, wanted: &str) -> bool {
    properties
        .split_whitespace()
        .any(|property| property.eq_ignore_ascii_case(wanted))
}

fn tag_is(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name().eq_ignore_ascii_case(name)
}

fn attr<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_issue(
    issues: &mut Vec<EpubIssue>,
    code: EpubIssueCode,
    href: Option<String>,
    detail: &str,
) {
    if issues
        .iter()
        .any(|issue| issue.code == code && issue.href == href && issue.detail == detail)
    {
        return;
    }
    issues.push(EpubIssue {
        code,
        href,
        detail: String::from(detail),
    });
}

fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "",
        Some(i) => &path[..i],
        None => "",
    }
}

fn join(base: &str, href: &str) -> String {
    let mut combined = String::from(base);
    if !combined.is_empty() {
        combined.push('/');
    }
    combined.push_str(href);

    let mut parts: Vec<&str> = Vec::new();
    for segment in combined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{is_external, percent_decode, split_href};

    #[test]
    fn external_schemes_are_recognized() {
        assert!(is_external("https://example.com/x"));
        assert!(is_external("mailto:a@b.c"));
        assert!(is_external("//example.com/x"));
        assert!(!is_external("ch1.xhtml"));
        assert!(!is_external("#note"));
        assert!(!is_external("../ch2.xhtml#n"));
    }

    #[test]
    fn hrefs_resolve_against_the_document() {
        let (path, frag) = split_href("OEBPS/text/ch1.xhtml", "../ch2.xhtml#top");
        assert_eq!(path, "OEBPS/ch2.xhtml");
        assert_eq!(frag.as_deref(), Some("top"));

        let (path, frag) = split_href("OEBPS/text/ch1.xhtml", "#note");
        assert_eq!(path, "OEBPS/text/ch1.xhtml");
        assert_eq!(frag.as_deref(), Some("note"));

        let (path, _) = split_href("OEBPS/text/ch1.xhtml", "../../../escape");
        assert_eq!(path, "escape");
    }

    #[test]
    fn percent_decoding_is_lenient() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%E9%9B%AA"), "雪");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
