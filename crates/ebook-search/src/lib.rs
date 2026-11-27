//! 全文搜索：把 `StyledDoc` 的正文压成一条可搜索的文本，并保留每个字节
//! 回到原文的映射。
//!
//! 这一层只做纯文本处理：不排版、不碰 I/O、不知道书是哪一章。调用方按章
//! `build`，命中后拿 `node + source_start` 去 `Engine::locate_locator` 换页/行。
//!
//! 关键点：
//! - 块级边界变成 `\n`，行内节点的文本直接拼接，所以 `hello <em>world</em>`
//!   能搜到 `hello world`，跨段落不会误命中。
//! - 连续空白折成一个空格，`ignore_whitespace` 的搜索结果不受原文缩进影响。
//! - 大小写不敏感默认开启；需要精确匹配时把 `case_sensitive` 设为 `true`。
//! - `SearchText` 可以被 [`SearchCache`] 缓存：同一章反复搜只建一次索引。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use ebook_ir::{Display, NodeId, NodeKind, StyledDoc, Tag};

/// 默认最多返回 100 条。宿主可以按 UI 需要调小。
pub const DEFAULT_MAX_HITS: u32 = 100;
/// 默认命中前后各取 36 个字符做上下文。
pub const DEFAULT_CONTEXT_CHARS: u16 = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchOptions {
    /// `false`（默认）时按 Unicode 小写比较。
    pub case_sensitive: bool,
    /// 最多写入多少条；`0` 按 [`DEFAULT_MAX_HITS`]。
    pub max_hits: u32,
    /// 每条命中前后保留多少字符；`0` 表示不要上下文。
    pub context_chars: u16,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            max_hits: DEFAULT_MAX_HITS,
            context_chars: DEFAULT_CONTEXT_CHARS,
        }
    }
}

/// 一条命中。
///
/// `start` / `end` 是 [`SearchText::text`] 里的字节区间，主要给调试与
/// 高亮测试用；宿主应该用 `node` + `source_start` 生成稳定的 `Locator`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub start: u32,
    pub end: u32,
    pub node: NodeId,
    /// 原文 `StyledDoc.text` 里的绝对字节起点。
    pub source_start: u32,
    /// 原文 `StyledDoc.text` 里的绝对字节终点（不含）。
    pub source_end: u32,
    pub before: String,
    pub text: String,
    pub after: String,
    /// 上下文是不是被截断过；UI 用它决定要不要加省略号。
    pub ellipsis_before: bool,
    pub ellipsis_after: bool,
}

/// 索引文本里的一段连续区间，对应一个文本节点的一部分。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    /// 在 `SearchText::text` 里的字节起点。
    start: u32,
    /// 在 `SearchText::text` 里的字节长度。
    len: u32,
    node: NodeId,
    /// 在原文里的字节起点；段内按 `source + (pos - start)` 线性映射。
    source: u32,
}

/// 一章的可搜索文本。
#[derive(Clone, Debug, Default)]
pub struct SearchText {
    text: String,
    runs: Vec<Run>,
}

impl SearchText {
    pub fn build(doc: &StyledDoc) -> Self {
        let mut out = Self::default();
        out.walk(doc, doc.root, 0);
        out
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn len(&self) -> u32 {
        self.text.len() as u32
    }

    /// 近似堆占用，用于缓存淘汰。
    pub fn approx_bytes(&self) -> usize {
        self.text.capacity()
            + self.runs.capacity() * core::mem::size_of::<Run>()
            + core::mem::size_of::<Self>()
    }

    /// 索引位置 → `(文本节点, 原文绝对字节偏移)`。
    ///
    /// `pos` 落在块分隔符上时返回它后面第一段文本的起点；越界返回 `None`。
    pub fn locate(&self, pos: u32) -> Option<(NodeId, u32)> {
        let pos = pos as usize;
        // 找到最后一个 `start <= pos` 的 run；`pos` 落在块分隔符上时它可能
        // 正好等于前一个 run 的末尾，此时也返回前一个 run 的末尾。
        let index = match self
            .runs
            .binary_search_by(|run| run.start.cmp(&(pos as u32)))
        {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        };
        let run = self.runs.get(index)?;
        let delta = pos.saturating_sub(run.start as usize);
        Some((run.node, run.source.saturating_add(delta as u32)))
    }

    /// 区间终点（不含）→ 原文绝对字节偏移。落在两个 run 之间的分隔符上时
    /// 退到前一个 run 的末尾。
    fn locate_end(&self, pos: u32) -> Option<(NodeId, u32)> {
        let pos = pos as usize;
        // 第一个 `end >= pos` 的 run 就是包含 `pos` 的那一个（或其后继）。
        let index = match self
            .runs
            .binary_search_by(|run| (run.start + run.len).cmp(&(pos as u32)))
        {
            Ok(index) => index,
            Err(index) => index,
        };
        let index = index.min(self.runs.len().saturating_sub(1));
        let run = self.runs.get(index)?;
        let run_end = (run.start + run.len) as usize;
        let delta = pos.min(run_end).saturating_sub(run.start as usize);
        Some((run.node, run.source.saturating_add(delta as u32)))
    }

    /// 在索引里找 `query`。返回 `true` 表示命中数超过 `max_hits`，结果被截断。
    pub fn find(&self, query: &str, options: &SearchOptions, out: &mut Vec<Hit>) -> bool {
        let query = collapse_whitespace(query.trim());
        if query.is_empty() || self.text.is_empty() {
            return false;
        }
        let max = if options.max_hits == 0 {
            DEFAULT_MAX_HITS
        } else {
            options.max_hits
        } as usize;
        if out.len() >= max {
            return true;
        }

        let (haystack, map) = if options.case_sensitive {
            (self.text.clone(), Vec::new())
        } else {
            fold(&self.text)
        };
        let needle = if options.case_sensitive {
            query
        } else {
            fold(&query).0
        };
        if needle.is_empty() {
            return false;
        }

        for (fold_start, _) in haystack.match_indices(&needle) {
            let fold_end = fold_start + needle.len();
            if !boundary_ok(&haystack, fold_start, fold_end, &needle) {
                continue;
            }
            let start = map_offset(&haystack, &map, fold_start);
            let end = map_offset(&haystack, &map, fold_end);
            if start >= end {
                continue;
            }
            let Some((node, source_start)) = self.locate(start) else {
                continue;
            };
            let Some((_, source_end)) = self.locate_end(end) else {
                continue;
            };
            if source_end < source_start {
                continue;
            }
            let (before, matched, after, ellipsis_before, ellipsis_after) =
                self.snippet(start, end, options.context_chars as usize);
            if out.len() >= max {
                return true;
            }
            out.push(Hit {
                start,
                end,
                node,
                source_start,
                source_end,
                before,
                text: matched,
                after,
                ellipsis_before,
                ellipsis_after,
            });
        }
        false
    }

    fn snippet(
        &self,
        start: u32,
        end: u32,
        context: usize,
    ) -> (String, String, String, bool, bool) {
        let text = &self.text;
        let start = start as usize;
        let end = (end as usize).min(text.len());
        let mut from = start;
        let mut count = 0;
        while from > 0 && count < context {
            from -= 1;
            while from > 0 && !text.is_char_boundary(from) {
                from -= 1;
            }
            count += 1;
        }
        let mut to = end;
        count = 0;
        while to < text.len() && count < context {
            to += 1;
            while to < text.len() && !text.is_char_boundary(to) {
                to += 1;
            }
            count += 1;
        }
        let before = text[from..start].trim_start();
        let matched = text[start..end].to_string();
        let after = text[end..to].trim_end();
        let ellipsis_before = from > 0 || before.len() != start - from;
        let ellipsis_after = to < text.len() || after.len() != to - end;
        (
            before.to_string(),
            matched,
            after.to_string(),
            ellipsis_before,
            ellipsis_after,
        )
    }

    fn walk(&mut self, doc: &StyledDoc, id: NodeId, depth: u16) {
        const MAX_DEPTH: u16 = 1024;
        if depth > MAX_DEPTH {
            return;
        }
        let Some(node) = doc.nodes.get(id as usize) else {
            return;
        };
        if node.style.display == Display::None {
            return;
        }
        if let NodeKind::Element(tag) = node.kind {
            if matches!(
                tag,
                Tag::Head | Tag::Style | Tag::Script | Tag::Title | Tag::Meta | Tag::Link
            ) {
                return;
            }
        }
        let block = node.style.is_block_level();
        if block {
            self.push_break();
        }
        match &node.kind {
            NodeKind::Text(span) => {
                let text = doc.text_of(*span);
                self.push_text(id, span.start, text);
            }
            NodeKind::Ruby { base, .. } => {
                // 注音是读音，不是正文；搜索只认基文字，避免同一个词出现两次。
                let text = doc.text_of(*base);
                self.push_text(id, base.start, text);
            }
            NodeKind::LineBreak => self.push_break(),
            NodeKind::Image(_) => {}
            NodeKind::Element(_) => {
                let start = node.first_child as usize;
                let end = start.saturating_add(node.child_len as usize);
                if let Some(kids) = doc.children.get(start..end) {
                    for &kid in kids {
                        self.walk(doc, kid, depth + 1);
                    }
                }
            }
        }
        if block {
            self.push_break();
        }
    }

    fn push_break(&mut self) {
        if self.text.is_empty() || self.text.ends_with('\n') {
            return;
        }
        self.text.push('\n');
    }

    fn push_text(&mut self, node: NodeId, source: u32, text: &str) {
        let mut run_start = self.text.len() as u32;
        let mut run_source = source;
        let mut open = false;
        for (offset, ch) in text.char_indices() {
            if ch.is_whitespace() {
                if open {
                    self.runs.push(Run {
                        start: run_start,
                        len: self.text.len() as u32 - run_start,
                        node,
                        source: run_source,
                    });
                    open = false;
                }
                if self.text.is_empty() || self.text.ends_with(' ') || self.text.ends_with('\n') {
                    continue;
                }
                let start = self.text.len() as u32;
                self.text.push(' ');
                self.runs.push(Run {
                    start,
                    len: 1,
                    node,
                    source: source.saturating_add(offset as u32),
                });
                continue;
            }
            if !open {
                run_start = self.text.len() as u32;
                run_source = source.saturating_add(offset as u32);
                open = true;
            }
            self.text.push(ch);
        }
        if open {
            self.runs.push(Run {
                start: run_start,
                len: self.text.len() as u32 - run_start,
                node,
                source: run_source,
            });
        }
    }
}

/// 有界的章索引缓存。键是 `(spine, rev)`：流式书 `replace_chapter` 后 rev
/// 变化，旧条目自然被替换；EPUB/TXT 的 `rev` 固定为 0。
#[derive(Clone, Debug)]
pub struct SearchCache {
    entries: Vec<CacheEntry>,
    bytes: usize,
    cap: usize,
}

#[derive(Clone, Debug)]
struct CacheEntry {
    spine: u32,
    rev: u64,
    text: SearchText,
}

impl Default for SearchCache {
    fn default() -> Self {
        Self::new(32 * 1024 * 1024)
    }
}

impl SearchCache {
    pub const fn new(cap: usize) -> Self {
        Self {
            entries: Vec::new(),
            bytes: 0,
            cap,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn invalidate(&mut self, spine: u32) {
        self.entries.retain(|entry| {
            if entry.spine == spine {
                self.bytes = self.bytes.saturating_sub(entry.text.approx_bytes());
                false
            } else {
                true
            }
        });
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, spine: u32, rev: u64) -> Option<&SearchText> {
        self.entries
            .iter()
            .find(|entry| entry.spine == spine && entry.rev == rev)
            .map(|entry| &entry.text)
    }

    pub fn insert(&mut self, spine: u32, rev: u64, text: SearchText) -> &SearchText {
        let bytes = text.approx_bytes();
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.spine == spine && entry.rev == rev)
        {
            let old = core::mem::replace(&mut self.entries[index].text, text);
            self.bytes = self.bytes.saturating_sub(old.approx_bytes());
            self.bytes = self.bytes.saturating_add(bytes);
            return &self.entries[index].text;
        }
        self.entries.push(CacheEntry { spine, rev, text });
        self.bytes = self.bytes.saturating_add(bytes);
        while self.bytes > self.cap && self.entries.len() > 1 {
            let old = self.entries.remove(0);
            self.bytes = self.bytes.saturating_sub(old.text.approx_bytes());
        }
        &self.entries[self.entries.len() - 1].text
    }
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(ch);
    }
    out
}

/// 小写化；只有在小写展开改变字符数/字节数时才保留逐字符映射。
fn fold(text: &str) -> (String, Vec<u32>) {
    let mut folded = String::with_capacity(text.len());
    let mut offsets = Vec::with_capacity(text.len() / 2);
    let mut complex = false;
    for (index, ch) in text.char_indices() {
        let mut count = 0;
        let mut bytes = 0;
        for lower in ch.to_lowercase() {
            folded.push(lower);
            offsets.push(index as u32);
            count += 1;
            bytes += lower.len_utf8();
        }
        if count != 1 || bytes != ch.len_utf8() {
            complex = true;
        }
    }
    if complex {
        // 末尾哨兵：`folded.len()` 这个位置也能映射回原文长度。
        offsets.push(text.len() as u32);
        (folded, offsets)
    } else {
        (folded, Vec::new())
    }
}

fn map_offset(folded: &str, offsets: &[u32], offset: usize) -> u32 {
    if offsets.is_empty() {
        return offset as u32;
    }
    let index = folded[..offset.min(folded.len())].chars().count();
    offsets.get(index).copied().unwrap_or(folded.len() as u32)
}

fn boundary_ok(text: &str, start: usize, end: usize, query: &str) -> bool {
    let first = query.chars().next();
    let last = query.chars().next_back();
    if first.is_some_and(|ch| ch.is_ascii_alphanumeric())
        && text[..start]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        return false;
    }
    if last.is_some_and(|ch| ch.is_ascii_alphanumeric())
        && text[end..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use ebook_ir::{Anchor, ComputedStyle, Display, DocMeta, Node, NodeKind, Span, StyledDoc, Tag};

    fn doc_with(text: &str) -> StyledDoc {
        let mut doc = StyledDoc {
            meta: DocMeta::default(),
            text: text.to_string(),
            ..StyledDoc::default()
        };
        let root = doc.push_node(Node {
            kind: NodeKind::Element(Tag::Root),
            style: ComputedStyle::default(),
            parent: None,
            first_child: 0,
            child_len: 0,
            src: Some(Anchor::default()),
            link: None,
        });
        doc.root = root;
        doc
    }

    fn push(
        doc: &mut StyledDoc,
        parent: NodeId,
        kind: NodeKind,
        display: Display,
        source: Option<Anchor>,
    ) -> NodeId {
        let style = ComputedStyle {
            display,
            ..ComputedStyle::default()
        };
        doc.push_node(Node {
            kind,
            style,
            parent: Some(parent),
            first_child: 0,
            child_len: 0,
            src: source,
            link: None,
        })
    }

    #[test]
    fn joins_inline_text_and_separates_blocks() {
        let mut doc = doc_with("hello world");
        let root = doc.root;
        let para = push(
            &mut doc,
            root,
            NodeKind::Element(Tag::P),
            Display::Block,
            None,
        );
        let a = push(
            &mut doc,
            para,
            NodeKind::Text(Span::new(0, 8)),
            Display::Inline,
            Some(Anchor {
                spine: 0,
                path: vec![0, 0],
                offset: 0,
            }),
        );
        let b = push(
            &mut doc,
            para,
            NodeKind::Text(Span::new(8, 3)),
            Display::Inline,
            Some(Anchor {
                spine: 0,
                path: vec![0, 1],
                offset: 8,
            }),
        );
        doc.set_kids(para, &[a, b]);
        doc.set_kids(root, &[para]);
        let index = SearchText::build(&doc);
        assert_eq!(index.text(), "hello world\n");
        let mut hits = Vec::new();
        assert!(!index.find("hello world", &SearchOptions::default(), &mut hits));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node, a);
        assert_eq!(hits[0].source_start, 0);
        assert_eq!(hits[0].text, "hello world");
    }

    #[test]
    fn skips_hidden_and_head() {
        let mut doc = doc_with("visible hiddentitle");
        let root = doc.root;
        let head = push(
            &mut doc,
            root,
            NodeKind::Element(Tag::Head),
            Display::Block,
            None,
        );
        let title = push(
            &mut doc,
            head,
            NodeKind::Text(Span::new(8, 9)),
            Display::Inline,
            Some(Anchor::default()),
        );
        doc.set_kids(head, &[title]);
        let hidden = push(
            &mut doc,
            root,
            NodeKind::Text(Span::new(14, 6)),
            Display::None,
            Some(Anchor::default()),
        );
        let visible = push(
            &mut doc,
            root,
            NodeKind::Text(Span::new(0, 7)),
            Display::Inline,
            Some(Anchor::default()),
        );
        doc.set_kids(root, &[head, hidden, visible]);
        let index = SearchText::build(&doc);
        assert_eq!(index.text(), "visible\n");
        let mut hits = Vec::new();
        index.find("hidden", &SearchOptions::default(), &mut hits);
        assert!(hits.is_empty());
        index.find("visible", &SearchOptions::default(), &mut hits);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn case_insensitive_and_word_boundary() {
        let mut doc = doc_with("The theatre cat");
        let root = doc.root;
        let text = push(
            &mut doc,
            root,
            NodeKind::Text(Span::new(0, 15)),
            Display::Inline,
            Some(Anchor::default()),
        );
        doc.set_kids(root, &[text]);
        let index = SearchText::build(&doc);
        let mut hits = Vec::new();
        index.find("the", &SearchOptions::default(), &mut hits);
        assert_eq!(hits.len(), 1, "theatre 不应命中 the");
        assert_eq!(hits[0].text, "The");
        hits.clear();
        index.find("CAT", &SearchOptions::default(), &mut hits);
        assert_eq!(hits.len(), 1);
        hits.clear();
        index.find(
            "the",
            &SearchOptions {
                case_sensitive: true,
                ..Default::default()
            },
            &mut hits,
        );
        assert_eq!(hits.len(), 0);
    }

    #[test]
    fn cache_keys_by_revision_and_invalidates() {
        let mut doc = doc_with("alpha");
        let root = doc.root;
        let text = push(
            &mut doc,
            root,
            NodeKind::Text(Span::new(0, 5)),
            Display::Inline,
            Some(Anchor::default()),
        );
        doc.set_kids(root, &[text]);
        let mut cache = SearchCache::new(1024);
        cache.insert(3, 7, SearchText::build(&doc));
        assert!(cache.get(3, 7).is_some());
        assert!(cache.get(3, 8).is_none());
        assert!(cache.bytes() > 0);
        cache.invalidate(3);
        assert!(cache.get(3, 7).is_none());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn limits_hits() {
        let mut doc = doc_with("a a a a a");
        let root = doc.root;
        let text = push(
            &mut doc,
            root,
            NodeKind::Text(Span::new(0, 9)),
            Display::Inline,
            Some(Anchor::default()),
        );
        doc.set_kids(root, &[text]);
        let index = SearchText::build(&doc);
        let mut hits = Vec::new();
        let truncated = index.find(
            "a",
            &SearchOptions {
                max_hits: 3,
                context_chars: 1,
                ..Default::default()
            },
            &mut hits,
        );
        assert!(truncated);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].source_start, 0);
    }
}
