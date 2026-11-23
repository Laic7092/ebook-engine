use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ebook_ir::{
    Anchor, ComputedStyle, Display, ImageId, ImageMeta, Node, NodeId, NodeKind, Span, StyledDoc,
    Tag,
};

use crate::css::{self, ElemKey, StyleSheet};
use crate::{ChapterAnchor, ChapterLinks, FormatError};

/// 一次章节解析的硬边界。数量级与 `ebook_ir::Limits` 的默认值接近；限额触发的
/// 章节返回可读的 `Malformed("xhtml")`，由阅读器按章报告，不拖垮整本书。
const MAX_XHTML_BYTES: usize = 32 * 1024 * 1024;
const MAX_XHTML_NODES: usize = 1_048_576;
const MAX_XHTML_DEPTH: usize = 64;
const MAX_XHTML_PARSE_DEPTH: usize = 128;
const MAX_XHTML_TEXT_BYTES: usize = 20_000_000;
/// 每节点保存一份锚点路径，宽而深的病态文档会把这份重复开销放大。
const MAX_XHTML_PATH_ENTRIES: usize = 8_000_000;
const MAX_XHTML_IMAGES: usize = u16::MAX as usize;

pub trait AssetResolver {
    fn css(&mut self, href: &str) -> Option<String>;

    /// 把章节里的相对引用（`<img src>`）解析成归档内路径。
    ///
    /// 默认原样返回：没有归档上下文的调用方（txt、单元测试）拿到的是属性原文。
    /// EPUB 的 resolver 会按章节所在目录展开 `../`。
    fn resolve(&mut self, href: &str) -> Option<String> {
        Some(href.to_string())
    }
}

pub struct NoAssets;

impl AssetResolver for NoAssets {
    fn css(&mut self, _href: &str) -> Option<String> {
        None
    }
}

enum CssSource {
    Inline(String),
    Linked(String),
}

/// 一次 XHTML 解析的完整产物。`build()` 只返回文档，`build_document()` 把格式层
/// 需要但 IR 还没有位置的 side table 一起带出去。
pub struct Built {
    pub doc: StyledDoc,
    pub links: Vec<ChapterLinks>,
    /// 带 `id` 的元素 → Anchor，供 `#fragment` 跳转。
    pub anchors: Vec<ChapterAnchor>,
    pub duplicate_ids: Vec<String>,
    pub unsupported_css: Vec<String>,
    /// 严格 XML 解析失败后，用 HTML 容错层归一化并成功解析。
    pub repaired: bool,
    /// 连容错解析也失败，退化成纯文本抽取。内容可能有损。
    pub text_fallback: bool,
}

struct Builder {
    doc: StyledDoc,
    keys: Vec<ElemKey>,
    sources: Vec<CssSource>,
    links: Vec<ChapterLinks>,
    anchors: Vec<ChapterAnchor>,
    ids: Vec<String>,
    duplicate_ids: Vec<String>,
    spine: u32,
    path_entries: usize,
    limit: Option<&'static str>,
}

impl Builder {
    fn push(&mut self, node: Node, key: ElemKey) -> NodeId {
        let id = self.doc.push_node(node);
        self.keys.push(key);
        id
    }

    fn hit_limit(&mut self, reason: &'static str) -> Option<NodeId> {
        if self.limit.is_none() {
            self.limit = Some(reason);
        }
        None
    }

    fn node(
        &mut self,
        xml: roxmltree::Node<'_, '_>,
        parent: Option<NodeId>,
        path: &mut Vec<u16>,
        resolver: &mut dyn AssetResolver,
        depth: usize,
    ) -> Option<NodeId> {
        if self.limit.is_some() {
            return None;
        }
        if depth > MAX_XHTML_DEPTH {
            return self.hit_limit("depth");
        }
        if xml.is_text() {
            let text = xml.text().unwrap_or("");
            if text.is_empty() {
                return None;
            }
            if self
                .doc
                .text
                .len()
                .checked_add(text.len())
                .is_none_or(|len| len > MAX_XHTML_TEXT_BYTES)
            {
                return self.hit_limit("text");
            }
            if self
                .path_entries
                .checked_add(path.len())
                .is_none_or(|entries| entries > MAX_XHTML_PATH_ENTRIES)
            {
                return self.hit_limit("anchors");
            }
            self.path_entries += path.len();
            let start = self.doc.text.len() as u32;
            self.doc.text.push_str(text);
            let key = ElemKey {
                is_element: false,
                ..ElemKey::default()
            };
            return Some(self.push(
                Node {
                    kind: NodeKind::Text(Span::new(start, text.len() as u32)),
                    style: ComputedStyle {
                        display: Display::Inline,
                        ..ComputedStyle::default()
                    },
                    parent,
                    first_child: 0,
                    child_len: 0,
                    link: None,
                    src: Some(Anchor {
                        spine: self.spine,
                        path: path.clone(),
                        offset: 0,
                    }),
                },
                key,
            ));
        }
        if !xml.is_element() {
            return None;
        }
        if self.doc.nodes.len() >= MAX_XHTML_NODES {
            return self.hit_limit("nodes");
        }
        if self
            .path_entries
            .checked_add(path.len())
            .is_none_or(|entries| entries > MAX_XHTML_PATH_ENTRIES)
        {
            return self.hit_limit("anchors");
        }

        let name = xml.tag_name().name();
        // `rp` 是 ruby 的回退括号；引擎自己处理注音，括号不能进正文。
        if name.eq_ignore_ascii_case("rp") {
            return None;
        }
        let tag = map_tag(name);
        let mut key = ElemKey {
            tag,
            tag_name: name.to_ascii_lowercase(),
            id: xml.attribute("id").map(|s| s.to_string()),
            classes: xml
                .attribute("class")
                .map(|c| c.split_whitespace().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
            inline: xml
                .attribute("style")
                .map(css::parse_decls)
                .unwrap_or_default(),
            is_element: true,
        };
        if let Some(lang) = xml.attribute("lang") {
            if self.doc.meta.lang.is_none() {
                self.doc.meta.lang = Some(lang.to_string());
            }
        }
        key.inline.retain(|d| !d.name.is_empty());

        if let Some(id) = key.id.as_deref() {
            if self.ids.iter().any(|existing| existing == id) {
                if !self.duplicate_ids.iter().any(|existing| existing == id) {
                    self.duplicate_ids.push(id.to_string());
                }
            } else {
                self.ids.push(id.to_string());
            }
        }
        if tag == Tag::Style {
            if let Some(text) = xml.text() {
                self.sources.push(CssSource::Inline(text.to_string()));
            }
        }
        if tag == Tag::Link {
            let rel = xml.attribute("rel").unwrap_or("");
            let is_style = rel
                .split_whitespace()
                .any(|part| part.eq_ignore_ascii_case("stylesheet"));
            if is_style {
                if let Some(href) = xml.attribute("href") {
                    self.sources.push(CssSource::Linked(href.to_string()));
                }
            }
        }
        let image = if tag == Tag::Img {
            if self.doc.images.len() >= MAX_XHTML_IMAGES {
                return self.hit_limit("images");
            }
            let id = self.doc.images.len() as ImageId;
            // `src` 由 resolver 解析成归档内路径；解析不了就保留属性原文。
            // SVG 封面用 `<image xlink:href>`，`href` 的命名空间前缀由
            // roxmltree 去掉，所以按本地名匹配即可。
            let src = image_ref(xml)
                .map(|href| resolver.resolve(href).unwrap_or_else(|| href.to_string()));
            self.doc.images.push(ImageMeta {
                // HTML `<img>` 优先用文件里的真实尺寸（build_chapter 会补）；
                // SVG `<image>` 没有文件尺寸可依，先用 width/height 属性。
                size: if name.eq_ignore_ascii_case("image") {
                    attr_size(xml)
                } else {
                    None
                },
                mime: None,
                alt: local_attr(xml, "alt").map(|s| s.to_string()),
                src,
            });
            Some(id)
        } else {
            None
        };

        let kind = match (tag, image) {
            (Tag::Br, _) => NodeKind::LineBreak,
            (_, Some(id)) => NodeKind::Image(id),
            _ => NodeKind::Element(tag),
        };

        // `<a href>` 是排版层发射 `Op::Link` 的唯一来源。空 href 忽略。
        // 这里保存的是**属性原文**（相对本章的路径 + fragment），不做解析：
        // 解析需要章节上下文，留给宿主的 `resolve_link_from`。
        let link = if tag == Tag::A {
            xml.attribute("href")
                .map(str::to_string)
                .filter(|href| !href.trim().is_empty())
        } else {
            None
        };

        let node = Node {
            kind,
            link,
            style: ComputedStyle::default(),
            parent,
            first_child: 0,
            child_len: 0,
            src: Some(Anchor {
                spine: self.spine,
                path: path.clone(),
                offset: 0,
            }),
        };
        self.path_entries += path.len();
        let id = self.push(node, key);
        if let Some(name) = self.keys[id as usize].id.clone() {
            self.anchors.push(ChapterAnchor {
                id: name,
                anchor: Anchor {
                    spine: self.spine,
                    path: path.clone(),
                    offset: 0,
                },
            });
        }
        if tag == Tag::A {
            if let Some(href) = xml.attribute("href") {
                if !href.trim().is_empty() {
                    self.links.push(ChapterLinks {
                        node: id,
                        href: href.to_string(),
                    });
                }
            }
        }

        let mut kids: Vec<NodeId> = Vec::new();
        let mut index: u16 = 0;
        let children: Vec<roxmltree::Node<'_, '_>> = xml.children().collect();
        for child in children {
            if self.limit.is_some() {
                break;
            }
            if !(child.is_element() || child.is_text()) {
                continue;
            }
            path.push(index);
            if let Some(kid) = self.node(child, Some(id), path, resolver, depth + 1) {
                kids.push(kid);
            }
            path.pop();
            index = index.saturating_add(1);
        }
        self.doc.set_kids(id, &kids);
        Some(id)
    }
}

pub fn build(
    xml: &str,
    spine: u32,
    resolver: &mut dyn AssetResolver,
) -> Result<StyledDoc, FormatError> {
    build_document(xml, spine, resolver).map(|built| built.doc)
}

pub fn build_document(
    xml: &str,
    spine: u32,
    resolver: &mut dyn AssetResolver,
) -> Result<Built, FormatError> {
    if xml.len() > MAX_XHTML_BYTES {
        return Err(FormatError::Malformed("xhtml"));
    }
    let xml = super::entities::normalized(xml);
    if xml.len() > MAX_XHTML_BYTES || super::entities::xml_too_deep(&xml, MAX_XHTML_PARSE_DEPTH) {
        return Err(FormatError::Malformed("xhtml"));
    }
    // roxmltree 默认拒绝 DOCTYPE（`allow_dtd: false`），而 pandoc / calibre / Sigil
    // 产出的 XHTML 几乎都带 `<!DOCTYPE html>`。DOCTYPE 只影响实体声明，实体已经
    // 由 `entities::expand` 展开，所以这里放行但忽略。
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: MAX_XHTML_NODES as u32,
    };
    // 严格解析优先：合法 XML 不做任何改写，保持既有行为与性能。失败时用
    // HTML 容错层归一化后重试——这是浏览器级兼容的入口，错误只留在本章内。
    let repaired_text: alloc::borrow::Cow<'_, str>;
    let mut repaired = false;
    let tree = match roxmltree::Document::parse_with_options(&xml, options) {
        Ok(tree) => tree,
        Err(_) => {
            repaired_text = super::html_repair::repair(&xml);
            repaired = true;
            roxmltree::Document::parse_with_options(&repaired_text, options)
                .map_err(|_| FormatError::Malformed("xhtml"))?
        }
    };
    let mut builder = Builder {
        doc: StyledDoc::default(),
        keys: Vec::new(),
        sources: Vec::new(),
        links: Vec::new(),
        anchors: Vec::new(),
        ids: Vec::new(),
        duplicate_ids: Vec::new(),
        spine,
        path_entries: 0,
        limit: None,
    };
    builder.doc.meta.spine = spine;
    let root = builder
        .node(tree.root_element(), None, &mut Vec::new(), resolver, 0)
        .ok_or(FormatError::Malformed("xhtml"))?;
    if builder.limit.is_some() {
        return Err(FormatError::Malformed("xhtml"));
    }
    builder.doc.root = root;

    let mut sheet = StyleSheet::default();
    for source in &builder.sources {
        match source {
            CssSource::Inline(text) => sheet.parse_into(text),
            CssSource::Linked(href) => {
                if let Some(text) = resolver.css(href) {
                    sheet.parse_into(&text);
                }
            }
        }
    }
    css::cascade(&mut builder.doc, &builder.keys, &sheet);

    // 无法落到 IR 的声明按属性名去重后上报，既包含链接样式表也包含内联 style。
    let mut unsupported_css: Vec<String> = sheet.unsupported().to_vec();
    for key in &builder.keys {
        for decl in &key.inline {
            if !css::decl_supported(decl) && !unsupported_css.iter().any(|n| n == &decl.name) {
                unsupported_css.push(decl.name.clone());
            }
        }
    }

    Ok(Built {
        doc: builder.doc,
        links: builder.links,
        anchors: builder.anchors,
        duplicate_ids: builder.duplicate_ids,
        unsupported_css,
        repaired,
        text_fallback: false,
    })
}

/// 最后一道兜底：连容错解析都失败时，至少把文字按行留下来。
///
/// 结构信息（标题、链接、图片、样式）全部丢失，但对读者来说「能读」优于
/// 「白屏」。调用方应当把这次降级作为 issue 上报。
pub fn text_document(xml: &str, spine: u32) -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = spine;

    let root = doc.push_node(Node {
        kind: NodeKind::Element(Tag::Body),
        style: ComputedStyle::default(),
        parent: None,
        first_child: 0,
        child_len: 0,
        src: Some(Anchor {
            spine,
            path: Vec::new(),
            offset: 0,
        }),
        link: None,
    });
    doc.root = root;

    let mut kids: Vec<NodeId> = Vec::new();
    let mut path_index: u16 = 0;
    let mut used = 0usize;
    for line in extract_lines(xml) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let remaining = MAX_XHTML_TEXT_BYTES.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        let mut end = line.len().min(remaining);
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            break;
        }
        let line = &line[..end];

        let path = alloc::vec![path_index];
        path_index = path_index.saturating_add(1);

        let p = doc.push_node(Node {
            kind: NodeKind::Element(Tag::P),
            style: ComputedStyle::default(),
            parent: Some(root),
            first_child: 0,
            child_len: 0,
            src: Some(Anchor {
                spine,
                path: path.clone(),
                offset: 0,
            }),
            link: None,
        });

        let start = doc.text.len() as u32;
        doc.text.push_str(line);
        used += line.len();
        let text = doc.push_node(Node {
            kind: NodeKind::Text(Span::new(start, line.len() as u32)),
            style: ComputedStyle {
                display: Display::Inline,
                ..ComputedStyle::default()
            },
            parent: Some(p),
            first_child: 0,
            child_len: 0,
            src: Some(Anchor {
                spine,
                path,
                offset: 0,
            }),
            link: None,
        });
        doc.set_kids(p, &[text]);
        kids.push(p);
    }
    doc.set_kids(root, &kids);
    doc
}

/// 去标签，按块级边界切行。只用于 `text_document`。
fn extract_lines(xml: &str) -> Vec<String> {
    let bytes = xml.as_bytes();
    let mut out = String::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let ch = xml[i..].chars().next().expect("char boundary");
            if ch != '\r' {
                out.push(if ch == '\t' { ' ' } else { ch });
            }
            i += ch.len_utf8();
            continue;
        }

        let mut j = i + 1;
        let closing = j < bytes.len() && bytes[j] == b'/';
        if closing {
            j += 1;
        }
        let name_start = j;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b':' | b'-' | b'_'))
        {
            j += 1;
        }
        let name = &xml[name_start..j].to_ascii_lowercase();

        // script/style 的内容不是正文，整段跳过。
        if !closing && matches!(name.as_str(), "script" | "style") {
            let lower = xml[i..].to_ascii_lowercase();
            let needle = alloc::format!("</{name}");
            i = match lower.find(&needle) {
                Some(at) => match xml[i + at..].find('>') {
                    Some(gt) => i + at + gt + 1,
                    None => xml.len(),
                },
                None => xml.len(),
            };
            out.push('\n');
            continue;
        }

        if is_text_break(name.as_str()) {
            out.push('\n');
        }

        let mut quote = 0u8;
        while i < bytes.len() {
            let b = bytes[i];
            if quote != 0 {
                if b == quote {
                    quote = 0;
                }
            } else if b == b'"' || b == b'\'' {
                quote = b;
            } else if b == b'>' {
                i += 1;
                break;
            }
            i += 1;
        }
    }
    out.split('\n')
        .map(alloc::string::ToString::to_string)
        .collect()
}

fn is_text_break(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "br"
            | "dd"
            | "div"
            | "dl"
            | "dt"
            | "figcaption"
            | "figure"
            | "footer"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "td"
            | "th"
            | "tr"
            | "ul"
    )
}

/// 图片地址：优先 `src`，其次 `href` / `xlink:href`。
///
/// `roxmltree::Attribute::name()` 返回本地名，所以 `xlink:href` 会以 `href`
/// 出现，不需要自己拆前缀。
fn image_ref<'a, 'input>(node: roxmltree::Node<'a, 'input>) -> Option<&'a str> {
    let mut fallback = None;
    for attr in node.attributes() {
        if attr.name().eq_ignore_ascii_case("src") {
            return Some(attr.value());
        }
        if fallback.is_none() && attr.name().eq_ignore_ascii_case("href") {
            fallback = Some(attr.value());
        }
    }
    fallback
}

/// 按本地名读属性（忽略命名空间），用于 `alt` 这类可能带前缀的属性。
fn local_attr<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attr| attr.name().eq_ignore_ascii_case(name))
        .map(|attr| attr.value())
}

/// SVG `<image>` 的 `width`/`height` 属性（只认无单位或 `px`）。
fn attr_size(node: roxmltree::Node<'_, '_>) -> Option<ebook_ir::SizeF> {
    let w = node.attribute("width").and_then(px_value)?;
    let h = node.attribute("height").and_then(px_value)?;
    (w > 0.0 && h > 0.0).then(|| ebook_ir::SizeF::new(w, h))
}

fn px_value(value: &str) -> Option<f32> {
    let value = value.trim();
    let value = value.strip_suffix("px").unwrap_or(value).trim();
    let number: f32 = value.parse().ok()?;
    number.is_finite().then_some(number)
}

fn map_tag(name: &str) -> Tag {
    match name.to_ascii_lowercase().as_str() {
        "html" => Tag::Html,
        "head" => Tag::Head,
        "body" => Tag::Body,
        "style" => Tag::Style,
        "title" => Tag::Title,
        "meta" => Tag::Meta,
        "link" => Tag::Link,
        "script" => Tag::Script,
        "p" => Tag::P,
        "div" => Tag::Div,
        "section" => Tag::Section,
        "article" => Tag::Article,
        "h1" => Tag::H1,
        "h2" => Tag::H2,
        "h3" => Tag::H3,
        "h4" => Tag::H4,
        "h5" => Tag::H5,
        "h6" => Tag::H6,
        "span" | "u" | "s" | "sup" | "sub" | "small" | "big" | "abbr" | "cite" | "q" | "mark"
        | "bdi" | "bdo" | "label" | "time" => Tag::Span,
        "em" | "i" => Tag::Em,
        "strong" | "b" => Tag::Strong,
        "a" => Tag::A,
        "li" => Tag::Li,
        "ol" => Tag::Ol,
        "ul" => Tag::Ul,
        "blockquote" => Tag::Blockquote,
        "pre" => Tag::Pre,
        "code" => Tag::Code,
        "img" => Tag::Img,
        // SVG 的 `<image>` 与 HTML 的 `<image>`（`img` 的旧别名）都按图片处理。
        "image" => Tag::Img,
        "br" => Tag::Br,
        "ruby" => Tag::Ruby,
        "rt" => Tag::Rt,
        "table" => Tag::Table,
        "tr" => Tag::Tr,
        "td" => Tag::Td,
        _ => Tag::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `rp` 是 ruby 的回退括号；引擎自己处理注音，括号不能进入正文。
    #[test]
    fn ruby_fallback_parentheses_are_dropped() {
        let html = "<html><body><p>\
                    <ruby>漢<rp>(</rp><rt>かん</rt><rp>)</rp></ruby>\
                    </p></body></html>";
        let doc: StyledDoc = build(html, 0, &mut NoAssets).expect("xhtml");
        assert!(doc.text.contains('漢'), "正文丢了: {:?}", doc.text);
        assert!(doc.text.contains("かん"), "注音丢了: {:?}", doc.text);
        assert!(
            !doc.text.contains('(') && !doc.text.contains(')'),
            "rp 括号不该留下: {:?}",
            doc.text
        );
    }
}
