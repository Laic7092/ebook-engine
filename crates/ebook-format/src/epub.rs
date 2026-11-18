use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{OnceCell, RefCell};

use ebook_ir::StyledDoc;

use crate::css;
use crate::xhtml::{self, AssetResolver};
use crate::zip::ZipArchive;
use crate::{
    Asset, Book, ByteSource, ChapterLinks, EmbeddedFont, EpubIssue, EpubIssueCode, FormatAdapter,
    FormatError, ImageAsset, LinkTarget, Probe, SpineItem, TocEntry,
};

/// 单个字体文件的上限。内嵌字体是排版入参，过大就直接当坏资产报告。
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

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
        let opf_path = container_opf(&zip).ok_or(FormatError::Missing("container.xml"))?;
        let opf = zip
            .read_text(&opf_path)
            .ok_or(FormatError::Missing("opf"))?;

        let mut issues = Vec::new();
        for name in zip.rejected() {
            push_issue(
                &mut issues,
                EpubIssueCode::UnsafePath,
                Some(name.clone()),
                "archive entry escapes the package root and was ignored",
            );
        }
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

    /// 与 `chapter` 相同，但把 `<a href>` 的 side table 一起返回。IR 的 `Node`
    /// 还没有链接字段，所以链接先由格式层保管。
    pub fn chapter_with_links(
        &self,
        index: usize,
    ) -> Result<(StyledDoc, Vec<ChapterLinks>), FormatError> {
        let href = self.book.spine.get(index).map(|item| item.href.clone());
        let built = match build_chapter(&self.zip, &self.book, index) {
            Ok(built) => built,
            Err(err) => {
                if err == FormatError::Malformed("xhtml") {
                    self.record(
                        EpubIssueCode::MalformedXhtml,
                        href,
                        "chapter XHTML could not be parsed",
                    );
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
        Ok((built.doc, built.links))
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
                let expanded = crate::entities::expand(&text);
                let doc = roxmltree::Document::parse(&expanded).ok()?;
                let found = doc
                    .descendants()
                    .any(|node| node.is_element() && attr(node, "id") == Some(id));
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
    let mut built = xhtml::build_document(&text, index as u32, &mut resolver)
        .map_err(|_| FormatError::Malformed("xhtml"))?;
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
    let text = super::entities::expand(&text);
    let doc = roxmltree::Document::parse(&text).ok()?;
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "rootfile")
        .and_then(|n| attr(n, "full-path"))
        .map(|s| normalize_href(s))
        .filter(|s| !s.is_empty())
}

fn parse_package(
    text: &str,
    opf_path: &str,
    issues: &mut Vec<EpubIssue>,
) -> Result<Package, FormatError> {
    let text = super::entities::expand(text);
    let doc = roxmltree::Document::parse(&text).map_err(|_| FormatError::Malformed("opf"))?;

    let mut manifest: Vec<ManifestItem> = Vec::new();
    for node in doc.descendants() {
        if !node.is_element() || node.tag_name().name() != "item" {
            continue;
        }
        let (Some(id), Some(href)) = (attr(node, "id"), attr(node, "href")) else {
            continue;
        };
        let resolved = resolve_href(opf_path, href);
        if manifest.iter().any(|item| item.id == id) {
            push_issue(
                issues,
                EpubIssueCode::DuplicateManifest,
                Some(resolved.clone()),
                &format!("manifest id {id} is declared more than once"),
            );
            continue;
        }
        if manifest.iter().any(|item| item.href == resolved) {
            push_issue(
                issues,
                EpubIssueCode::DuplicateManifest,
                Some(resolved.clone()),
                "manifest href is declared more than once",
            );
        }
        manifest.push(ManifestItem {
            id: id.to_string(),
            href: resolved,
            media_type: attr(node, "media-type").unwrap_or("").to_string(),
            properties: attr(node, "properties").unwrap_or("").to_string(),
        });
    }

    let spine_node = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "spine");
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
        let Some(item) = manifest.iter().find(|item| item.id == idref) else {
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
        .filter(|n| n.is_element() && n.tag_name().name() == "reference")
        .filter_map(|n| {
            let href = attr(n, "href")?;
            let kind = attr(n, "type").unwrap_or("");
            Some((kind.to_string(), resolve_href(opf_path, href)))
        })
        .collect();
    let meta_cover = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "meta")
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
                    n.is_element()
                        && n.tag_name().name() == "identifier"
                        && attr(*n, "id") == Some(wanted)
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
            let id = book.images.len().min(u16::MAX as usize) as u16;
            book.images.push(ImageAsset {
                id,
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

    for href in candidates {
        if is_image_asset(book, &href) {
            return Some(href);
        }
        // EPUB2 的封面常是一个包装页：跟到它的第一张图。
        if let Some(text) = zip.read_text(&href) {
            if let Some(src) = first_img_src(&text) {
                let resolved = resolve_href(&href, &src);
                if is_image_asset(book, &resolved) || zip.has(&resolved) {
                    return Some(resolved);
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
    let expanded = crate::entities::expand(text);
    let doc = roxmltree::Document::parse(&expanded).ok()?;
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "img")
        .and_then(|n| attr(n, "src"))
        .map(|s| s.to_string())
}

fn collect_fonts(
    zip: &ZipArchive<'_>,
    package: &Package,
    book: &mut Book,
    issues: &mut Vec<EpubIssue>,
) {
    for item in &package.manifest {
        if !item.media_type.eq_ignore_ascii_case("text/css") {
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
    for item in &package.manifest {
        if !has_property(&item.properties, "nav") {
            continue;
        }
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

    for row in rows {
        let spine = book.spine_index(&row.path).unwrap_or(TocEntry::NO_SPINE);
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
    let expanded = crate::entities::expand(text);
    let doc = roxmltree::Document::parse(&expanded).map_err(|_| ())?;
    let nav = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "nav" && is_toc_nav(*node));
    let Some(nav) = nav else {
        return Ok(Vec::new());
    };
    let list = nav
        .descendants()
        .find(|node| node.is_element() && matches!(node.tag_name().name(), "ol" | "ul"));
    let Some(list) = list else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    nav_list(list, nav_path, 0, &mut out);
    Ok(out)
}

fn is_toc_nav(node: roxmltree::Node<'_, '_>) -> bool {
    let type_attr = attr(node, "type").unwrap_or("").to_ascii_lowercase();
    let role = attr(node, "role").unwrap_or("").to_ascii_lowercase();
    has_property(&type_attr, "toc") || has_property(&role, "doc-toc")
}

fn nav_list(list: roxmltree::Node<'_, '_>, base: &str, depth: u16, out: &mut Vec<TocRow>) {
    for item in list
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "li")
    {
        let (label, href) = nav_label(item);
        let mut children = Vec::new();
        for child in item
            .children()
            .filter(|node| node.is_element() && matches!(node.tag_name().name(), "ol" | "ul"))
        {
            nav_list(child, base, depth.saturating_add(1), &mut children);
        }
        if label.is_empty() && href.as_deref().unwrap_or("").is_empty() {
            out.extend(children);
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
        out.extend(children);
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
    for child in node.children().filter(|child| child.is_element()) {
        let name = child.tag_name().name();
        if matches!(name, "ol" | "ul") {
            continue;
        }
        if name == "a" {
            return Some(child);
        }
        if let Some(found) = first_nav_link(child) {
            return Some(found);
        }
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
    let expanded = crate::entities::expand(text);
    let doc = roxmltree::Document::parse(&expanded).map_err(|_| ())?;
    let nav_map = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "navMap");
    let Some(nav_map) = nav_map else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    ncx_points(nav_map, ncx_path, 0, &mut out);
    Ok(out)
}

fn ncx_points(parent: roxmltree::Node<'_, '_>, base: &str, depth: u16, out: &mut Vec<TocRow>) {
    for point in parent
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "navPoint")
    {
        let label = point
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "navLabel")
            .and_then(|label| {
                label
                    .children()
                    .find(|node| node.is_element() && node.tag_name().name() == "text")
            })
            .and_then(|text| text.text())
            .map(collapse)
            .unwrap_or_default();
        let src = point
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "content")
            .and_then(|content| attr(content, "src"));
        let (path, fragment) = match src {
            Some(src) => split_href(base, src),
            None => (String::new(), None),
        };
        let mut children = Vec::new();
        ncx_points(point, base, depth.saturating_add(1), &mut children);
        if label.is_empty() && path.is_empty() {
            out.extend(children);
            continue;
        }
        out.push(TocRow {
            label,
            path,
            fragment,
            depth,
        });
        out.extend(children);
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
    value.to_string()
}

fn has_property(properties: &str, wanted: &str) -> bool {
    properties
        .split_whitespace()
        .any(|property| property.eq_ignore_ascii_case(wanted))
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
