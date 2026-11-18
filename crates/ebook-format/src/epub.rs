use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ebook_ir::StyledDoc;

use crate::xhtml::{self, AssetResolver};
use crate::zip::ZipArchive;
use crate::{Book, ByteSource, FormatAdapter, FormatError, Probe, SpineItem};

pub struct Epub;

pub struct EpubReader<'a> {
    zip: ZipArchive<'a>,
    book: Book,
}

impl<'a> EpubReader<'a> {
    pub fn open(raw: &'a [u8]) -> Result<Self, FormatError> {
        let zip = ZipArchive::parse(raw).map_err(|_| FormatError::Malformed("zip"))?;
        let opf_path = container_opf(&zip).ok_or(FormatError::Missing("container.xml"))?;
        let opf = zip.read_text(&opf_path).ok_or(FormatError::Missing("opf"))?;
        let book = parse_opf(&opf, &opf_path)?;
        Ok(Self { zip, book })
    }

    pub fn book(&self) -> &Book {
        &self.book
    }

    pub fn chapter(&self, index: usize) -> Result<StyledDoc, FormatError> {
        chapter_from_zip(&self.zip, &self.book, index)
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
        let zip = ZipArchive::parse(&raw).map_err(|_| FormatError::Malformed("zip"))?;
        let opf_path = container_opf(&zip).ok_or(FormatError::Missing("container.xml"))?;
        let opf = zip.read_text(&opf_path).ok_or(FormatError::Missing("opf"))?;
        parse_opf(&opf, &opf_path)
    }

    fn chapter(
        &self,
        src: &dyn ByteSource,
        book: &Book,
        index: usize,
    ) -> Result<StyledDoc, FormatError> {
        let raw = src.document().ok_or(FormatError::Missing("epub"))?;
        let zip = ZipArchive::parse(&raw).map_err(|_| FormatError::Malformed("zip"))?;
        chapter_from_zip(&zip, book, index)
    }
}

fn chapter_from_zip(
    zip: &ZipArchive<'_>,
    book: &Book,
    index: usize,
) -> Result<StyledDoc, FormatError> {
    let item = book.spine.get(index).ok_or(FormatError::OutOfRange)?;
    let text = zip
        .read_text(&item.href)
        .ok_or(FormatError::Missing("chapter"))?;
    let mut resolver = ZipAssets {
        zip,
        base: dirname(&item.href).to_string(),
    };
    let mut doc = xhtml::build(&text, index as u32, &mut resolver)
        .map_err(|_| FormatError::Malformed("xhtml"))?;
    doc.meta.title = book.title.clone();
    doc.meta.lang = book.lang.clone();
    Ok(doc)
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

fn container_opf(zip: &ZipArchive<'_>) -> Option<String> {
    let text = zip.read_text("META-INF/container.xml")?;
    let doc = roxmltree::Document::parse(&text).ok()?;
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(|s| s.to_string())
}

fn parse_opf(text: &str, opf_path: &str) -> Result<Book, FormatError> {
    let doc = roxmltree::Document::parse(text).map_err(|_| FormatError::Malformed("opf"))?;
    let base = dirname(opf_path);

    let mut manifest: Vec<(String, String, String)> = Vec::new();
    for node in doc.descendants() {
        if !node.is_element() || node.tag_name().name() != "item" {
            continue;
        }
        if let (Some(id), Some(href)) = (node.attribute("id"), node.attribute("href")) {
            manifest.push((
                id.to_string(),
                join(base, href),
                node.attribute("media-type").unwrap_or("").to_string(),
            ));
        }
    }

    let mut spine = Vec::new();
    for node in doc.descendants() {
        if !node.is_element() || node.tag_name().name() != "itemref" {
            continue;
        }
        let Some(idref) = node.attribute("idref") else {
            continue;
        };
        let Some((id, href, media)) = manifest.iter().find(|(id, _, _)| id == idref) else {
            continue;
        };
        spine.push(SpineItem {
            id: id.clone(),
            href: href.clone(),
            media_type: media.clone(),
            linear: node.attribute("linear").unwrap_or("yes") != "no",
        });
    }

    if spine.is_empty() {
        return Err(FormatError::Malformed("spine"));
    }

    let title = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "title")
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let lang = doc
        .descendants()
        .find(|n| {
            n.is_element() && matches!(n.tag_name().name(), "language" | "lang")
        })
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Ok(Book { title, lang, spine })
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
