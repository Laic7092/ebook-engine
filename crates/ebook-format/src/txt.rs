use core::ops::Range;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ebook_ir::{
    Anchor, ComputedStyle, Dir, Display, DocMeta, Len, LineHeight, Node, NodeId, NodeKind, Span,
    StyledDoc, Tag, TextAlign, WritingMode,
};

use crate::{
    Book, ByteSource, ChapterAnchor, ChapterLinks, FormatAdapter, FormatError, LinkTarget, Probe,
    SpineItem, TocEntry,
};

/// 没有章节标记时的兜底切分：一章最多保留这么多字节。
/// 单个巨大的 TXT 如果整本排版，首次打开会卡住主线程；按行边界切开既能分页，
/// 也让“第 N 页/共 M 页”有确定的章节上下文。
const MAX_CHAPTER_BYTES: usize = 128 * 1024;

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
            ..Book::default()
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
    let blocks = split_lines(&raw);

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
    for (i, block) in blocks.iter().enumerate() {
        // TXT 没有结构信息：认出来的章节标题行包成 H1，其余每一行各自成段。
        let heading = is_heading(block);
        let kind = if heading { Tag::H1 } else { Tag::P };
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
            src: Some(Anchor {
                spine: 0,
                path: alloc::vec![i as u16],
                offset: 0,
            }),
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
                path: alloc::vec![i as u16, 0],
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

pub struct TxtReader {
    text: String,
    book: Book,
    chapters: Vec<Range<usize>>,
}

impl TxtReader {
    /// 把已经解码好的 UTF-8 文本切成章节。编码检测属于宿主（浏览器用
    /// `TextDecoder` 能处理 GBK/Big5/Shift_JIS），格式层只认字符串。
    pub fn from_text(text: &str, name: Option<&str>) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text).to_string();
        let chapters = split_chapters(&text);
        let title = title_of(&text, name);
        let lang = detect_lang(&text);

        let mut spine = Vec::with_capacity(chapters.len());
        let mut toc = Vec::with_capacity(chapters.len());
        for (index, span) in chapters.iter().enumerate() {
            let href = alloc::format!("chapter-{index}.txt");
            let label = chapter_label(&text[span.clone()], index);
            spine.push(SpineItem {
                id: alloc::format!("c{index}"),
                href: href.clone(),
                media_type: String::from("text/plain"),
                linear: true,
            });
            toc.push(TocEntry {
                label,
                href,
                spine: index,
                fragment: None,
                depth: 0,
            });
        }

        let book = Book {
            title: Some(title),
            lang: Some(lang.clone()),
            language: Some(lang),
            spine,
            toc,
            ..Book::default()
        };
        Self {
            text,
            book,
            chapters,
        }
    }

    pub fn book(&self) -> &Book {
        &self.book
    }

    pub fn chapter(&self, index: usize) -> Result<StyledDoc, FormatError> {
        self.chapter_with_anchors(index).map(|(doc, _, _)| doc)
    }

    pub fn chapter_with_anchors(
        &self,
        index: usize,
    ) -> Result<(StyledDoc, Vec<ChapterLinks>, Vec<ChapterAnchor>), FormatError> {
        let span = self.chapters.get(index).ok_or(FormatError::OutOfRange)?;
        let mut doc = build(self.text[span.clone()].as_bytes());
        doc.meta.spine = index as u32;
        doc.meta.lang = self.book.lang.clone();
        doc.meta.title = self.book.toc.get(index).map(|entry| entry.label.clone());
        // `build` 不知道自己在第几章；把文本节点的锚点改到当前 spine，
        // 这样 IR 里的 line.anchor 才能与 `Book::spine` 对齐。
        for id in 0..doc.nodes.len() as u32 {
            if let Some(src) = doc.node_mut(id).src.as_mut() {
                src.spine = index as u32;
            }
        }
        Ok((doc, Vec::new(), Vec::new()))
    }

    pub fn asset<'a>(&'a self, _href: &str) -> Option<(&'a [u8], &'a str)> {
        None
    }

    /// 一章的字节数（切章区间长度）。高度估计只需要这个数，不必先解析正文。
    pub fn chapter_size(&self, index: usize) -> Option<u64> {
        self.chapters.get(index).map(|span| span.len() as u64)
    }

    pub fn resolve_link_from(&self, _spine: usize, _href: &str) -> Option<LinkTarget> {
        None
    }
}

/// 按“标题行 + 超长兜底”切章。返回的是原文里的字节区间，避免为每章复制字符串。
fn split_chapters(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut has_content = false;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let heading = is_heading(trimmed);
        let too_long = has_content && offset - start >= MAX_CHAPTER_BYTES;
        if (heading && has_content) || too_long {
            out.push(start..line_start);
            start = line_start;
            has_content = false;
        }
        if !has_content {
            start = line_start;
            has_content = true;
        }
    }
    if has_content {
        out.push(start..text.len());
    }
    if out.is_empty() {
        out.push(0..text.len());
    }
    // 单行超过上限（例如整本没有换行的 TXT）时上面不会切；这里按字符边界
    // 再兜一层，保证任何一章都不会无限大。
    let mut bounded = Vec::with_capacity(out.len());
    for span in out {
        push_bounded(text, span, &mut bounded);
    }
    bounded
}

fn push_bounded(text: &str, span: Range<usize>, out: &mut Vec<Range<usize>>) {
    if span.start == span.end {
        out.push(span);
        return;
    }
    let mut start = span.start;
    while span.end - start > MAX_CHAPTER_BYTES {
        let mut end = (start + MAX_CHAPTER_BYTES).min(span.end);
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if let Some(pos) = text[start..end].rfind('\n') {
            end = start + pos + 1;
        }
        if end == start {
            end = (start + 1).min(span.end);
            while end < span.end && !text.is_char_boundary(end) {
                end += 1;
            }
        }
        out.push(start..end);
        start = end;
    }
    if start < span.end {
        out.push(start..span.end);
    }
}

fn title_of(text: &str, name: Option<&str>) -> String {
    if let Some(name) = name {
        let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let stem = base.rsplit_once('.').map_or(base, |(stem, _)| stem).trim();
        if !stem.is_empty() {
            return String::from(stem);
        }
    }
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first.is_empty() {
        String::from("Untitled")
    } else {
        truncate_chars(first, 60)
    }
}

fn chapter_label(text: &str, index: usize) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first.is_empty() {
        alloc::format!("Chapter {}", index + 1)
    } else {
        truncate_chars(first, 60)
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (count, ch) in text.chars().enumerate() {
        if count >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

/// 语言只用于挑字体与 `locl`：有假名/谚文就分别标成 ja/ko，汉字占多数标 zh，
/// 其余按英文处理。它不参与排版规则，所以不值得引入完整的语言检测。
fn detect_lang(text: &str) -> String {
    let mut cjk = 0usize;
    let mut kana = 0usize;
    let mut hangul = 0usize;
    let mut latin = 0usize;
    for ch in text.chars().take(20_000) {
        match ch as u32 {
            0x3040..=0x30ff => kana += 1,
            0xac00..=0xd7a3 => hangul += 1,
            0x3400..=0x9fff => cjk += 1,
            _ if ch.is_ascii_alphabetic() => latin += 1,
            _ => {}
        }
    }
    if kana > 0 && kana * 4 >= cjk {
        String::from("ja")
    } else if hangul > 0 && hangul * 2 >= cjk {
        String::from("ko")
    } else if cjk > 0 && cjk * 3 >= latin {
        String::from("zh")
    } else {
        String::from("en")
    }
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

/// 每个非空行都是一个独立的段落。TXT 的换行就是作者给出的分段边界，
/// 不再把连续行合并成一段（那样会在行尾丢换行、还给中英混排补空格）。
fn split_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|line| line.trim_end_matches('\r').trim())
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect()
}

// ---------------------------------------------------------------------------
// 章节流（宿主在线书源）
// ---------------------------------------------------------------------------

/// 宿主按章推送的纯文本小说。没有整本字节、没有 zip：每章独立保存，
/// 追加/替换只影响该章。`title` 由宿主给，正文按“每行一段”构建 IR。
///
/// 与 `TxtReader` 的区别：后者一打开就要整本解码 + 全书扫描切章；
/// 这里 0 章起步，章可以随时 append/replace，内容版本（`rev`）用于页缓存键。
pub struct StreamBook {
    book: Book,
    chapters: Vec<StreamChapter>,
    next_rev: u64,
    fingerprint: u64,
}

struct StreamChapter {
    title: String,
    text: String,
    rev: u64,
}

impl StreamBook {
    pub fn new(title: &str) -> Self {
        let mut book = Book::default();
        book.title = Some(title.to_string());
        book.lang = Some(String::from("zh"));
        book.language = Some(String::from("zh"));
        let mut this = Self {
            book,
            chapters: Vec::new(),
            next_rev: 1,
            fingerprint: 0,
        };
        this.fingerprint = this.compute_fingerprint(title);
        this
    }

    pub fn book(&self) -> &Book {
        &self.book
    }

    pub fn count(&self) -> usize {
        self.chapters.len()
    }

    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// 追加一章，返回新的章数。
    pub fn append(&mut self, title: &str, text: &str) -> usize {
        let rev = self.next_rev;
        self.next_rev = self.next_rev.wrapping_add(1);
        self.chapters.push(StreamChapter {
            title: title.to_string(),
            text: text.to_string(),
            rev,
        });
        self.rebuild_meta();
        self.fingerprint = self.compute_fingerprint(&self.book.title.clone().unwrap_or_default());
        self.chapters.len()
    }

    /// 替换一章。章数与其它章不变；该章 `rev` 变成新值。
    pub fn replace(&mut self, index: usize, title: &str, text: &str) -> bool {
        let rev = self.next_rev;
        let Some(chapter) = self.chapters.get_mut(index) else {
            return false;
        };
        chapter.title = title.to_string();
        chapter.text = text.to_string();
        chapter.rev = rev;
        self.next_rev = self.next_rev.wrapping_add(1);
        self.book.toc[index].label = title.to_string();
        self.fingerprint = self.compute_fingerprint(&self.book.title.clone().unwrap_or_default());
        true
    }

    pub fn clear(&mut self) {
        self.chapters.clear();
        self.rebuild_meta();
        self.fingerprint = self.compute_fingerprint(&self.book.title.clone().unwrap_or_default());
    }

    pub fn chapter_rev(&self, index: usize) -> Option<u64> {
        self.chapters.get(index).map(|chapter| chapter.rev)
    }

    pub fn chapter_size(&self, index: usize) -> Option<u64> {
        self.chapters
            .get(index)
            .map(|chapter| chapter.text.len() as u64)
    }

    pub fn chapter_with_anchors(
        &self,
        index: usize,
    ) -> Result<(StyledDoc, Vec<ChapterLinks>, Vec<ChapterAnchor>), FormatError> {
        let chapter = self.chapters.get(index).ok_or(FormatError::OutOfRange)?;
        let mut doc = build(chapter.text.as_bytes());
        doc.meta.spine = index as u32;
        doc.meta.title = Some(chapter.title.clone());
        doc.meta.lang = self.book.lang.clone();
        for id in 0..doc.nodes.len() as u32 {
            if let Some(src) = doc.node_mut(id).src.as_mut() {
                src.spine = index as u32;
            }
        }
        Ok((doc, Vec::new(), Vec::new()))
    }

    pub fn asset<'a>(&'a self, _href: &str) -> Option<(&'a [u8], &'a str)> {
        None
    }

    pub fn resolve_link_from(&self, _spine: usize, _href: &str) -> Option<LinkTarget> {
        None
    }

    fn rebuild_meta(&mut self) {
        let mut spine = Vec::with_capacity(self.chapters.len());
        let mut toc = Vec::with_capacity(self.chapters.len());
        for (index, chapter) in self.chapters.iter().enumerate() {
            let href = alloc::format!("stream-{index}.txt");
            spine.push(SpineItem {
                id: alloc::format!("s{index}"),
                href: href.clone(),
                media_type: String::from("text/plain"),
                linear: true,
            });
            toc.push(TocEntry {
                label: chapter.title.clone(),
                href,
                spine: index,
                fragment: None,
                depth: 0,
            });
        }
        self.book.spine = spine;
        self.book.toc = toc;
    }

    fn compute_fingerprint(&self, title: &str) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |chunk: &[u8]| {
            for &byte in chunk {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(b"ebook-stream-book-v1");
        eat(title.as_bytes());
        eat(&(self.chapters.len() as u64).to_le_bytes());
        for chapter in &self.chapters {
            eat(&chapter.rev.to_le_bytes());
            eat(&(chapter.title.len() as u64).to_le_bytes());
            eat(chapter.title.as_bytes());
            eat(&(chapter.text.len() as u64).to_le_bytes());
        }
        hash
    }
}
