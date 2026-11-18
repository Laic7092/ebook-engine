pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod glue;

use std::fmt::Write as _;
use std::sync::Mutex;

use ebook_font::FontBook;
use ebook_format::epub::EpubReader;
use ebook_format::Book;
use ebook_ir::{codec, EdgesF};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

static BOOK: Mutex<Option<EpubReader<'static>>> = Mutex::new(None);
static FONT: Mutex<Option<FontBook>> = Mutex::new(None);

pub fn layout_text(text: &str, width: f32, height: f32, margin: f32) -> Vec<u8> {
    let doc = ebook_format::txt::build(text.as_bytes());
    layout_doc(&doc, width, height, margin)
}

pub fn open_book(bytes: &[u8]) -> String {
    let leaked: &'static [u8] = Box::leak(bytes.to_vec().into_boxed_slice());
    match EpubReader::open(leaked) {
        Ok(reader) => {
            let json = book_json(reader.book());
            *BOOK.lock().unwrap() = Some(reader);
            json
        }
        Err(error) => error_json(&format!("open: {error:?}")),
    }
}

pub fn register_font(bytes: &[u8]) -> u32 {
    let mut slot = FONT.lock().unwrap();
    let font = slot.get_or_insert_with(FontBook::new);
    match font.add(bytes.to_vec()) {
        Some(id) => id.0 as u32,
        None => u32::MAX,
    }
}

pub fn reset_fonts() {
    *FONT.lock().unwrap() = None;
}

pub fn font_count() -> u32 {
    FONT.lock()
        .unwrap()
        .as_ref()
        .map(|font| font.len() as u32)
        .unwrap_or(0)
}

pub fn font_desc(id: u32) -> String {
    let fonts = FONT.lock().unwrap();
    let Some(book) = fonts.as_ref() else {
        return String::from("{\"family\":null}");
    };
    let font = ebook_ir::FontId(id as u16);
    match book.desc(font) {
        Some(desc) => {
            let mut out = String::from("{\"family\":");
            push_json_str(&mut out, &desc.family);
            out.push_str(",\"weight\":");
            let _ = write!(out, "{}", desc.weight);
            out.push_str(",\"style\":");
            push_json_str(&mut out, font_style_name(desc.style));
            out.push_str(",\"bold\":");
            out.push_str(if desc.weight >= 600 || book.is_synthetic_bold(font) {
                "true"
            } else {
                "false"
            });
            out.push_str(",\"italic\":");
            out.push_str(
                if !matches!(desc.style, ebook_ir::FontStyle::Normal)
                    || book.is_synthetic_italic(font)
                {
                    "true"
                } else {
                    "false"
                },
            );
            out.push('}');
            out
        }
        None => String::from("{\"family\":null}"),
    }
}

fn font_style_name(style: ebook_ir::FontStyle) -> &'static str {
    match style {
        ebook_ir::FontStyle::Normal => "normal",
        ebook_ir::FontStyle::Italic => "italic",
        ebook_ir::FontStyle::Oblique => "oblique",
    }
}

pub fn layout_cached(spine: u32, width: f32, height: f32, margin: f32) -> Vec<u8> {
    let books = BOOK.lock().unwrap();
    let Some(reader) = books.as_ref() else {
        return Vec::new();
    };
    let Ok(doc) = reader.chapter(spine as usize) else {
        return Vec::new();
    };
    let font = FONT.lock().unwrap();
    match font.as_ref() {
        Some(font) => layout_doc_with(&doc, font, width, height, margin),
        None => layout_doc(&doc, width, height, margin),
    }
}

#[cfg(feature = "epub")]
pub fn layout_chapter(
    source: &[u8],
    name: &str,
    spine: usize,
    width: f32,
    height: f32,
    margin: f32,
) -> Vec<u8> {
    let mut src = ebook_format::MemorySource::new();
    src.insert(name, source.to_vec());
    let Ok(opened) = ebook_format::open(&src) else {
        return Vec::new();
    };
    let Ok(doc) = opened.chapter(&src, spine) else {
        return Vec::new();
    };
    layout_doc(&doc, width, height, margin)
}

fn layout_doc(doc: &ebook_ir::StyledDoc, width: f32, height: f32, margin: f32) -> Vec<u8> {
    layout_doc_with(doc, &StubMeasurer, width, height, margin)
}

fn layout_doc_with(
    doc: &ebook_ir::StyledDoc,
    measurer: &dyn ebook_layout::TextMeasurer,
    width: f32,
    height: f32,
    margin: f32,
) -> Vec<u8> {
    let env = LayoutEnv {
        measurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(width, height).with_margin(EdgesF::all(margin));
    let ir = Layout::new().layout(doc, &env, &constraint);
    codec::to_bytes(&ir).unwrap_or_default()
}

fn error_json(message: &str) -> String {
    let mut out = String::from("{\"error\":");
    push_json_str(&mut out, message);
    out.push('}');
    out
}

fn book_json(book: &Book) -> String {
    let mut out = String::from("{\"title\":");
    push_json_opt(&mut out, book.title.as_deref());
    out.push_str(",\"language\":");
    push_json_opt(&mut out, book.lang.as_deref());
    out.push_str(",\"lang\":");
    push_json_opt(&mut out, book.lang.as_deref());
    out.push_str(",\"spine\":[");
    for (index, item) in book.spine.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"index\":");
        let _ = write!(out, "{index}");
        out.push_str(",\"id\":");
        push_json_str(&mut out, &item.id);
        out.push_str(",\"href\":");
        push_json_str(&mut out, &item.href);
        out.push_str(",\"mediaType\":");
        push_json_str(&mut out, &item.media_type);
        out.push_str(",\"media\":");
        push_json_str(&mut out, &item.media_type);
        out.push_str(",\"linear\":");
        out.push_str(if item.linear { "true" } else { "false" });
        out.push('}');
    }
    out.push_str("]}");
    out
}

fn push_json_opt(out: &mut String, value: Option<&str>) {
    match value {
        Some(value) => push_json_str(out, value),
        None => out.push_str("null"),
    }
}

fn push_json_str(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}
