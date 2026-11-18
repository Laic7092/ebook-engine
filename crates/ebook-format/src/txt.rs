use alloc::string::String;
use alloc::vec::Vec;

use ebook_ir::{
    Anchor, ComputedStyle, Dir, Display, DocMeta, Len, LineHeight, Node, NodeId, NodeKind, Span,
    StyledDoc, Tag, TextAlign, WritingMode,
};

use crate::{Book, ByteSource, FormatAdapter, FormatError, Probe, SpineItem};

pub struct Txt;

impl FormatAdapter for Txt {
    fn name(&self) -> &'static str {
        "txt"
    }

    fn probe(&self, src: &dyn ByteSource) -> Probe {
        if src.document().is_some_and(|bytes| bytes.starts_with(b"PK")) {
            return Probe::No;
        }
        if src
            .names()
            .iter()
            .any(|n| n.to_ascii_lowercase().ends_with(".txt"))
        {
            Probe::Maybe
        } else {
            Probe::No
        }
    }

    fn open(&self, src: &dyn ByteSource) -> Result<Book, FormatError> {
        let name = src
            .names()
            .into_iter()
            .find(|n| n.to_ascii_lowercase().ends_with(".txt"))
            .ok_or(FormatError::Missing("txt"))?;
        Ok(Book {
            title: None,
            lang: Some(String::from("zh")),
            spine: alloc::vec![SpineItem {
                id: String::from("txt"),
                href: String::from(name),
                media_type: String::from("text/plain"),
                linear: true,
            }],
        })
    }

    fn chapter(
        &self,
        src: &dyn ByteSource,
        book: &Book,
        index: usize,
    ) -> Result<StyledDoc, FormatError> {
        let item = book.spine.get(index).ok_or(FormatError::OutOfRange)?;
        let bytes = src.read(&item.href).ok_or(FormatError::Missing("txt"))?;
        let mut doc = build(&bytes);
        doc.meta.spine = index as u32;
        Ok(doc)
    }
}

pub fn build(bytes: &[u8]) -> StyledDoc {
    let raw = String::from_utf8_lossy(bytes).into_owned();
    let blocks = split_blocks(&raw);

    let mut doc = StyledDoc::default();
    doc.meta = DocMeta {
        spine: 0,
        title: blocks.first().cloned(),
        lang: Some(String::from("zh")),
        dir: Dir::Ltr,
        writing_mode: WritingMode::HorizontalTb,
    };
    doc.text = String::new();

    let root_style = ComputedStyle::default();
    let root = doc.push_node(Node {
        kind: NodeKind::Element(Tag::Root),
        style: root_style,
        parent: None,
        first_child: 0,
        child_len: 0,
        src: Some(Anchor::default()),
        link: None,
    });
    doc.root = root;

    let mut para_ids: Vec<NodeId> = Vec::new();
    for block in &blocks {
        let heading = is_heading(block);
        let kind = if heading { Tag::H2 } else { Tag::P };
        para_ids.push(doc.push_node(Node {
            kind: NodeKind::Element(kind),
            style: if heading {
                heading_style()
            } else {
                paragraph_style()
            },
            parent: Some(root),
            first_child: 0,
            child_len: 0,
            src: Some(Anchor::default()),
            link: None,
        }));
    }
    doc.set_kids(root, &para_ids);

    for (i, block) in blocks.iter().enumerate() {
        let start = doc.text.len() as u32;
        doc.text.push_str(block);
        let span = Span::new(start, block.len() as u32);
        let para = para_ids[i];
        let text_style = ComputedStyle {
            display: Display::Inline,
            ..doc.node(para).style
        };
        let text_id = doc.push_node(Node {
            kind: NodeKind::Text(span),
            style: text_style,
            parent: Some(para),
            first_child: 0,
            child_len: 0,
            src: Some(Anchor {
                spine: 0,
                path: alloc::vec![i as u16],
                offset: 0,
            }),
            link: None,
        });
        doc.set_kids(para, &[text_id]);
    }

    doc
}

fn paragraph_style() -> ComputedStyle {
    let mut style = ComputedStyle::default();
    style.display = Display::Block;
    style.font.size = 18.0;
    style.line_height = LineHeight::Mul(1.75);
    style.text.align = TextAlign::Justify;
    style.text.indent = Len::Em(2.0);
    style.margin.bottom = Len::Em(0.55);
    style
}

fn heading_style() -> ComputedStyle {
    let mut style = ComputedStyle::default();
    style.display = Display::Block;
    style.font.size = 26.0;
    style.font.weight = 700;
    style.line_height = LineHeight::Mul(1.4);
    style.text.align = TextAlign::Center;
    style.margin.top = Len::Em(1.2);
    style.margin.bottom = Len::Em(1.0);
    style
}

fn is_heading(block: &str) -> bool {
    let count = block.chars().count();
    if count == 0 || count > 40 {
        return false;
    }
    let trimmed = block.trim_start();
    if trimmed.starts_with('\u{7B2C}')
        && (trimmed.contains('\u{7AE0}')
            || trimmed.contains('\u{56DE}')
            || trimmed.contains('\u{8282}')
            || trimmed.contains('\u{5377}'))
    {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("chapter") || lower.starts_with("part ")
}

fn split_blocks(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() {
            if !cur.is_empty() {
                out.push(core::mem::take(&mut cur));
            }
            continue;
        }
        if cur.is_empty() {
            cur.push_str(line);
        } else {
            let last_wide = cur.chars().next_back().map_or(false, is_wide);
            let next_wide = line.chars().next().map_or(false, is_wide);
            if !last_wide && !next_wide {
                cur.push(' ');
            }
            cur.push_str(line);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_wide(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFF00..=0xFF60
    )
}
