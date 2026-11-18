use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ebook_ir::{
    Anchor, ComputedStyle, Display, ImageId, ImageMeta, Node, NodeId, NodeKind, Span, StyledDoc,
    Tag,
};

use crate::css::{self, ElemKey, StyleSheet};
use crate::{ChapterLinks, FormatError};

pub trait AssetResolver {
    fn css(&mut self, href: &str) -> Option<String>;
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
    pub duplicate_ids: Vec<String>,
    pub unsupported_css: Vec<String>,
}

struct Builder {
    doc: StyledDoc,
    keys: Vec<ElemKey>,
    sources: Vec<CssSource>,
    links: Vec<ChapterLinks>,
    ids: Vec<String>,
    duplicate_ids: Vec<String>,
    spine: u32,
}

impl Builder {
    fn push(&mut self, node: Node, key: ElemKey) -> NodeId {
        let id = self.doc.push_node(node);
        self.keys.push(key);
        id
    }

    fn node(
        &mut self,
        xml: roxmltree::Node<'_, '_>,
        parent: Option<NodeId>,
        path: &mut Vec<u16>,
    ) -> Option<NodeId> {
        if xml.is_text() {
            let text = xml.text().unwrap_or("");
            if text.is_empty() {
                return None;
            }
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

        let name = xml.tag_name().name();
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
            let id = self.doc.images.len() as ImageId;
            self.doc.images.push(ImageMeta {
                size: None,
                mime: None,
                alt: xml.attribute("alt").map(|s| s.to_string()),
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

        let node = Node {
            kind,
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
        let id = self.push(node, key);
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
            if !(child.is_element() || child.is_text()) {
                continue;
            }
            path.push(index);
            if let Some(kid) = self.node(child, Some(id), path) {
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
    let xml = super::entities::expand(xml);
    let tree = roxmltree::Document::parse(&xml).map_err(|_| FormatError::Malformed("xhtml"))?;
    let mut builder = Builder {
        doc: StyledDoc::default(),
        keys: Vec::new(),
        sources: Vec::new(),
        links: Vec::new(),
        ids: Vec::new(),
        duplicate_ids: Vec::new(),
        spine,
    };
    builder.doc.meta.spine = spine;
    let root = builder
        .node(tree.root_element(), None, &mut Vec::new())
        .ok_or(FormatError::Malformed("xhtml"))?;
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
        duplicate_ids: builder.duplicate_ids,
        unsupported_css,
    })
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
        "br" => Tag::Br,
        "ruby" => Tag::Ruby,
        "rt" => Tag::Rt,
        "table" => Tag::Table,
        "tr" => Tag::Tr,
        "td" => Tag::Td,
        _ => Tag::Other,
    }
}
