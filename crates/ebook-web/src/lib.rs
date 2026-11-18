pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod glue;

use std::fmt::Write as _;
use std::sync::Mutex;

use ebook_font::FontBook;
use ebook_format::epub::EpubReader;
use ebook_format::Book;
use ebook_ir::{codec, EdgesF, LayoutResult, Location, PointF, StyledDoc};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, Reader, StubMeasurer};

static BOOK: Mutex<Option<EpubReader<'static>>> = Mutex::new(None);
static FONT: Mutex<Option<FontBook>> = Mutex::new(None);
/// 最近一次排版的 IR 与原文。交互查询（命中/选区/进度）只认这一份，
/// 与 JS 侧当前显示的章节保持同步；排版层本身仍然无状态。
static LAST: Mutex<Option<LastLayout>> = Mutex::new(None);

struct LastLayout {
    ir: LayoutResult,
    doc: StyledDoc,
}

impl LastLayout {
    fn reader(&self) -> Reader<'_> {
        Reader::new(&self.ir)
    }
}

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
    *LAST.lock().unwrap() = Some(LastLayout {
        ir: ir.clone(),
        doc: doc.clone(),
    });
    codec::to_bytes(&ir).unwrap_or_default()
}

fn with_last<T>(read: impl FnOnce(&LastLayout) -> T) -> Option<T> {
    let slot = LAST.lock().unwrap();
    slot.as_ref().map(read)
}

/// 页内坐标 → 原文位置。`{"location":{...}}` 或 `{"location":null}`。
pub fn hit_json(page: u32, x: f32, y: f32) -> String {
    let hit = with_last(|last| last.reader().hit(page, PointF::new(x, y))).flatten();
    let mut out = String::from("{\"location\":");
    match hit {
        Some(loc) => push_location(&mut out, loc),
        None => out.push_str("null"),
    }
    out.push('}');
    out
}

/// 页内坐标 → 链接目标。`{"href":"..."}` 或 `{"href":null}`。
pub fn link_at_json(page: u32, x: f32, y: f32) -> String {
    let href = with_last(|last| {
        last.reader()
            .link_at(page, PointF::new(x, y))
            .map(str::to_string)
    })
    .flatten();
    let mut out = String::from("{\"href\":");
    match href {
        Some(href) => push_json_str(&mut out, &href),
        None => out.push_str("null"),
    }
    out.push('}');
    out
}

/// 两个 `Location` → 选区矩形 + 原文。矩形是页内坐标，正序、按页/行返回。
pub fn selection_json(
    a_page: u32,
    a_line: u32,
    a_byte: u32,
    b_page: u32,
    b_line: u32,
    b_byte: u32,
) -> String {
    with_last(|last| {
        let reader = last.reader();
        let sel = reader.selection(
            Location {
                page: a_page,
                line: a_line,
                byte: a_byte,
            },
            Location {
                page: b_page,
                line: b_line,
                byte: b_byte,
            },
        );
        let (start, end) = sel.normalize();
        let mut out = String::from("{\"start\":");
        push_location(&mut out, start);
        out.push_str(",\"end\":");
        push_location(&mut out, end);
        out.push_str(",\"rects\":[");
        for (index, item) in reader.selection_rects(&sel).iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "{{\"page\":{},\"x\":", item.page);
            push_f32(&mut out, item.rect.x);
            out.push_str(",\"y\":");
            push_f32(&mut out, item.rect.y);
            out.push_str(",\"w\":");
            push_f32(&mut out, item.rect.w);
            out.push_str(",\"h\":");
            push_f32(&mut out, item.rect.h);
            out.push('}');
        }
        out.push_str("],\"text\":");
        push_json_str(&mut out, &reader.selection_text(&last.doc, &sel));
        out.push('}');
        out
    })
    .unwrap_or_else(|| String::from("{\"rects\":[],\"text\":\"\"}"))
}

/// 全书进度，0..=1。`{"progress":0.25}`。
pub fn progress_json(page: u32, line: u32, byte: u32) -> String {
    let mut out = String::from("{\"progress\":");
    let progress =
        with_last(|last| last.reader().progress(Location { page, line, byte })).unwrap_or(0.0);
    push_f32(&mut out, progress);
    out.push('}');
    out
}

fn push_location(out: &mut String, loc: Location) {
    let _ = write!(
        out,
        "{{\"page\":{},\"line\":{},\"byte\":{}}}",
        loc.page, loc.line, loc.byte
    );
}

fn push_f32(out: &mut String, value: f32) {
    if value.is_finite() {
        let _ = write!(out, "{value}");
    } else {
        out.push('0');
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 交互 JSON 的形状与时序：排版后 `LAST` 就是查询目标。
    /// 一个测试串起来跑，避免并行测试互相覆盖 `LAST`。
    #[test]
    fn interaction_json_shapes() {
        // txt 排版：内容宽 = 520 - 2×40 = 440，行高由默认字号决定；只要有行就行。
        let bytes = layout_text("hello world hello world", 520.0, 300.0, 40.0);
        assert!(!bytes.is_empty());
        let ir: LayoutResult = ebook_ir::decode(&bytes).unwrap();
        let first = &ir.runs.lines[0];

        // 页内坐标 = 流坐标 + inset（margin 40）。行 0 左上角对应页内 (40, 40)。
        let hit = hit_json(0, 41.0, 41.0);
        assert_eq!(hit, "{\"location\":{\"page\":0,\"line\":0,\"byte\":0}}");
        assert_eq!(hit_json(99, 0.0, 0.0), "{\"location\":null}");
        assert!(first.rect.w > 0.0);

        // txt 里没有链接。
        assert_eq!(link_at_json(0, 41.0, 41.0), "{\"href\":null}");

        // 选区：前两个字节 → "he"，矩形只有一个，页内从 inset 起。
        let sel = selection_json(0, 0, 0, 0, 0, 2);
        assert!(sel.contains(",\"text\":\"he\""), "{sel}");
        assert!(sel.contains("\"rects\":[{\"page\":0,"), "{sel}");
        assert!(
            sel.contains("\"start\":{\"page\":0,\"line\":0,\"byte\":0}"),
            "{sel}"
        );

        // 反向选区先 normalize。
        let reversed = selection_json(0, 0, 2, 0, 0, 0);
        assert!(reversed.contains(",\"text\":\"he\""), "{reversed}");
        assert!(
            reversed.contains("\"start\":{\"page\":0,\"line\":0,\"byte\":0}"),
            "{reversed}"
        );

        // 进度：行首 0，最后一行末尾 1。
        assert_eq!(progress_json(0, 0, 0), "{\"progress\":0}");
        let last = ir.runs.lines.len() as u32 - 1;
        let byte = ir.runs.lines[last as usize].src.len;
        assert_eq!(progress_json(0, last, byte), "{\"progress\":1}");
        // 越界进度不 panic，夹到 1。
        assert_eq!(progress_json(9, 99, 99), "{\"progress\":1}");
    }
}
