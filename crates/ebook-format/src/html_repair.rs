//! 把真实世界的 XHTML/HTML 归一化成格式良好的 XML，供 `roxmltree` 解析。
//!
//! EPUB 规范要求 XHTML，但线上书大量是 HTML 工具链产出的：属性不加引号、裸
//! `&`、`<br>` 不闭合、结束标签错配、一个文件里有多个根节点……浏览器靠 HTML5
//! 的错误恢复全部容忍。`roxmltree` 是严格 XML 解析器，一字节不合规就整章拒绝。
//!
//! 这一层不是针对某几本书的补丁，而是一个通用的 tokenizer + 树构造恢复：
//!
//! - 属性值可以单引号、双引号或不加引号；
//! - 文本/属性里的 `&` 只在构成合法字符引用时保留，否则转义；
//! - void 元素与显式 `/>` 自动自闭合；
//! - 错配/多余的结束标签按 HTML 规则处理（向上找匹配，找不到就丢弃）；
//! - 可选结束标签（`p`/`li`/`td`/`tr`/`option`…）按块级开始标签隐式闭合；
//! - 标签名大小写不敏感，重复属性只保留第一个；
//! - 多个顶层节点或纯片段会补一个合成的 `<html>`/`<body>` 根。
//!
//! 输出永远只有一个根元素，且所有标签/属性名都是合法 XML 名，所以后续
//! `roxmltree` 解析不会再因为“脏输入”失败。语义错误（例如 CSS 写错）不在这里
//! 修，交给原有分层。

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// 先严格解析，失败再用 [`repair`] 归一化后重试。返回值和“是否修复过”。
///
/// `read` 在文档存活期间消费它，因此调用方只能把所有权数据带出去；这样
/// 不需要自引用结构，也不会把 `roxmltree::Document` 借到 `repaired` 之外。
pub(crate) fn parse_with_repair<T>(
    text: &str,
    options: roxmltree::ParsingOptions,
    read: impl FnOnce(&roxmltree::Document<'_>) -> T,
) -> Option<(T, bool)> {
    match roxmltree::Document::parse_with_options(text, options) {
        Ok(doc) => Some((read(&doc), false)),
        Err(_) => {
            let repaired = repair(text);
            let doc = roxmltree::Document::parse_with_options(&repaired, options).ok()?;
            Some((read(&doc), true))
        }
    }
}

/// 修复入口。输入已经过 [`crate::entities::normalized`]，但即使没有也应当健壮。
pub(crate) fn repair(input: &str) -> Cow<'_, str> {
    if input.is_empty() {
        return Cow::Borrowed(input);
    }
    let mut parser = Parser {
        open: Vec::new(),
        top: Vec::new(),
    };
    parser.run(input);
    Cow::Owned(parser.finish())
}

#[derive(Clone)]
struct Elem {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Item>,
    /// void 元素或显式 `/>`；序列化时不写结束标签。
    self_closing: bool,
}

#[derive(Clone)]
enum Item {
    Elem(Elem),
    Text(String),
}

struct Parser {
    open: Vec<Elem>,
    top: Vec<Item>,
}

impl Parser {
    fn run(&mut self, input: &str) {
        let bytes = input.as_bytes();
        let mut i = 0usize;
        let mut text_start = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'<' {
                i += 1;
                continue;
            }
            if i > text_start {
                self.push_text(&input[text_start..i]);
            }

            let rest = &input[i..];
            if rest.starts_with("<!--") {
                i = find_after(bytes, i + 4, b"-->");
                text_start = i;
                continue;
            }
            if rest.starts_with("<![CDATA[") {
                let end = find_after(bytes, i + 9, b"]]>");
                let content_end = end.saturating_sub(3).max(i + 9).min(bytes.len());
                self.push_text(&input[i + 9..content_end]);
                i = end;
                text_start = i;
                continue;
            }
            if rest.starts_with("<!") {
                // DOCTYPE / 声明：整段跳过。实体已在 normalized 里展开。
                i = skip_declaration(bytes, i + 2);
                text_start = i;
                continue;
            }
            if rest.starts_with("<?") {
                // XML 声明或处理指令：跳过。
                i = find_after(bytes, i + 2, b"?>");
                text_start = i;
                continue;
            }
            if rest.starts_with("</") {
                if let Some((name, end)) = parse_end_tag(input, i) {
                    self.end_tag(&name);
                    i = end;
                    text_start = i;
                    continue;
                }
                self.push_text("<");
                i += 1;
                text_start = i;
                continue;
            }

            if let Some((elem, self_closing, end, raw_name)) = parse_start_tag(input, i) {
                let raw_text = !self_closing && is_raw_text(&raw_name);
                self.start_tag(elem, self_closing);
                i = end;
                text_start = i;
                if raw_text {
                    if let Some((content, after)) = take_raw_text(input, i, &raw_name) {
                        self.push_text(&content);
                        self.end_tag(&raw_name);
                        i = after;
                        text_start = i;
                    }
                }
                continue;
            }

            // 不是标签起始的 `<`，按普通文本处理（浏览器也这么做）。
            self.push_text("<");
            i += 1;
            text_start = i;
        }
        if text_start < input.len() {
            self.push_text(&input[text_start..]);
        }
        while !self.open.is_empty() {
            self.close_top();
        }
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let dest = match self.open.last_mut() {
            Some(elem) => &mut elem.children,
            None => &mut self.top,
        };
        if let Some(Item::Text(prev)) = dest.last_mut() {
            prev.push_str(text);
        } else {
            dest.push(Item::Text(text.to_string()));
        }
    }

    fn append_elem(&mut self, elem: Elem) {
        match self.open.last_mut() {
            Some(parent) => parent.children.push(Item::Elem(elem)),
            None => self.top.push(Item::Elem(elem)),
        }
    }

    fn close_top(&mut self) {
        let Some(elem) = self.open.pop() else {
            return;
        };
        self.append_elem(elem);
    }

    fn start_tag(&mut self, mut elem: Elem, self_closing: bool) {
        while let Some(top) = self.open.last() {
            if auto_close(&top.name, &elem.name) {
                self.close_top();
            } else {
                break;
            }
        }
        elem.self_closing = self_closing || is_void(&elem.name);
        if elem.self_closing {
            self.append_elem(elem);
        } else {
            self.open.push(elem);
        }
    }

    fn end_tag(&mut self, name: &str) {
        // 向上找最近的同名开始标签；中间的标签按 HTML 规则隐式闭合。
        // 找不到就是多余的结束标签，直接丢弃。
        if let Some(index) = self
            .open
            .iter()
            .rposition(|elem| elem.name.eq_ignore_ascii_case(name))
        {
            while self.open.len() > index {
                self.close_top();
            }
        }
    }

    fn finish(self) -> String {
        let top = self.top;
        let html_count = top
            .iter()
            .filter(
                |item| matches!(item, Item::Elem(elem) if elem.name.eq_ignore_ascii_case("html")),
            )
            .count();
        let has_extra = top.iter().any(|item| match item {
            Item::Elem(elem) => !elem.name.eq_ignore_ascii_case("html"),
            Item::Text(text) => !text.trim().is_empty(),
        });

        if html_count == 1 && !has_extra {
            let mut root = top
                .into_iter()
                .find_map(|item| match item {
                    Item::Elem(elem) => Some(elem),
                    Item::Text(_) => None,
                })
                .expect("html_count == 1");
            root.self_closing = false;
            let mut out = String::new();
            serialize_elem(&root, &mut out);
            return out;
        }

        // 片段/多根：补一个合成的 html，并尽量保留已有的 head/body。
        let mut head: Option<Elem> = None;
        let mut body: Option<Elem> = None;
        let mut rest: Vec<Item> = Vec::new();
        for item in top {
            match item {
                Item::Elem(elem) if elem.name.eq_ignore_ascii_case("head") && head.is_none() => {
                    head = Some(elem)
                }
                Item::Elem(mut elem)
                    if elem.name.eq_ignore_ascii_case("body") && body.is_none() =>
                {
                    elem.self_closing = false;
                    body = Some(elem);
                }
                other => rest.push(other),
            }
        }
        let mut root = Elem {
            name: "html".to_string(),
            attrs: Vec::new(),
            children: Vec::new(),
            self_closing: false,
        };
        if let Some(head) = head {
            root.children.push(Item::Elem(head));
        }
        let mut body = body.unwrap_or_else(|| Elem {
            name: "body".to_string(),
            attrs: Vec::new(),
            children: Vec::new(),
            self_closing: false,
        });
        body.children.extend(rest);
        root.children.push(Item::Elem(body));

        let mut out = String::new();
        serialize_elem(&root, &mut out);
        out
    }
}

// ---------------------------------------------------------------------------
// 标签解析
// ---------------------------------------------------------------------------

/// 解析 `<name attr=value ...>`。返回 `(元素, 是否自闭合, 结束位置, 原始标签名)`。
fn parse_start_tag(input: &str, at: usize) -> Option<(Elem, bool, usize, String)> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = at + 1;
    if i >= len || !is_name_start(bytes[i]) {
        return None;
    }
    let name_start = i;
    while i < len && !is_name_delim(bytes[i]) {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    let raw_name = &input[name_start..i];
    let name = sanitize_name(raw_name);
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut self_closing = false;

    loop {
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }
        match bytes[i] {
            b'>' => {
                i += 1;
                break;
            }
            b'/' => {
                if i + 1 < len && bytes[i + 1] == b'>' {
                    self_closing = true;
                    i += 2;
                    break;
                }
                i += 1;
                continue;
            }
            _ => {}
        }

        let attr_start = i;
        while i < len + 1 {
            if i >= len {
                break;
            }
            let b = bytes[i];
            if b.is_ascii_whitespace() || b == b'=' || b == b'>' || b == b'/' {
                break;
            }
            i += 1;
        }
        if i == attr_start {
            i += 1;
            continue;
        }
        let attr_name = sanitize_attr_name(&input[attr_start..i]);
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < len && bytes[i] == b'=' {
            i += 1;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < len && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let value_start = i;
                while i < len && bytes[i] != quote {
                    i += 1;
                }
                value.push_str(&input[value_start..i]);
                if i < len {
                    i += 1;
                }
            } else {
                let value_start = i;
                while i < len {
                    let b = bytes[i];
                    if b.is_ascii_whitespace() || b == b'>' {
                        break;
                    }
                    // 未加引号的值里，`/>` 的斜杠按自闭合处理（作者意图优先）。
                    if b == b'/' && i + 1 < len && bytes[i + 1] == b'>' {
                        break;
                    }
                    i += 1;
                }
                value.push_str(&input[value_start..i]);
            }
        }
        if !attr_name.is_empty() && !attrs.iter().any(|(existing, _)| existing == &attr_name) {
            attrs.push((attr_name, value));
        }
    }

    Some((
        Elem {
            name,
            attrs,
            children: Vec::new(),
            self_closing: false,
        },
        self_closing,
        i,
        raw_name.to_string(),
    ))
}

fn parse_end_tag(input: &str, at: usize) -> Option<(String, usize)> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = at + 2;
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let name_start = i;
    if i >= len || !is_name_start(bytes[i]) {
        return None;
    }
    while i < len && !is_name_delim(bytes[i]) {
        i += 1;
    }
    let name = sanitize_name(&input[name_start..i]);
    while i < len && bytes[i] != b'>' {
        i += 1;
    }
    if i < len {
        i += 1;
    }
    Some((name, i))
}

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b':' || byte >= 0x80
}

fn is_name_delim(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || byte == b'/'
        || byte == b'>'
        || byte == b'<'
        || byte == b'='
        || byte == b'"'
        || byte == b'\''
}

fn find_after(bytes: &[u8], from: usize, needle: &[u8]) -> usize {
    if needle.is_empty() || from >= bytes.len() {
        return bytes.len();
    }
    let mut i = from;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            return i + needle.len();
        }
        i += 1;
    }
    bytes.len()
}

fn skip_declaration(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    let mut quote = 0u8;
    let mut bracket = 0usize;
    while i < bytes.len() {
        let byte = bytes[i];
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = byte,
            b'[' => bracket += 1,
            b']' => bracket = bracket.saturating_sub(1),
            b'>' if bracket == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

fn take_raw_text(input: &str, from: usize, name: &str) -> Option<(String, usize)> {
    let lower = input[from..].to_ascii_lowercase();
    let needle = alloc::format!("</{}", name.to_ascii_lowercase());
    let at = lower.find(&needle)?;
    let content_end = from + at;
    let close = input[content_end..].find('>')? + content_end + 1;
    Some((input[from..content_end].to_string(), close))
}

// ---------------------------------------------------------------------------
// HTML 树构造规则
// ---------------------------------------------------------------------------

fn name_in(name: &str, list: &[&str]) -> bool {
    list.iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn is_void(name: &str) -> bool {
    name_in(
        name,
        &[
            "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
            "source", "track", "wbr",
        ],
    )
}

fn is_raw_text(name: &str) -> bool {
    name_in(name, &["script", "style", "title", "textarea"])
}

fn is_block(name: &str) -> bool {
    name_in(
        name,
        &[
            "address",
            "article",
            "aside",
            "blockquote",
            "details",
            "dialog",
            "dir",
            "div",
            "dl",
            "fieldset",
            "figcaption",
            "figure",
            "footer",
            "form",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "header",
            "hgroup",
            "hr",
            "li",
            "main",
            "menu",
            "nav",
            "ol",
            "p",
            "pre",
            "section",
            "table",
            "ul",
        ],
    )
}

/// 遇到 `new` 时，栈顶的 `open` 是否应当隐式闭合。
fn auto_close(open: &str, new: &str) -> bool {
    if open.eq_ignore_ascii_case("p") && (new.eq_ignore_ascii_case("p") || is_block(new)) {
        return true;
    }
    if new.eq_ignore_ascii_case("li") {
        return open.eq_ignore_ascii_case("li");
    }
    if new.eq_ignore_ascii_case("dt") || new.eq_ignore_ascii_case("dd") {
        return open.eq_ignore_ascii_case("dt") || open.eq_ignore_ascii_case("dd");
    }
    if new.eq_ignore_ascii_case("td") || new.eq_ignore_ascii_case("th") {
        return open.eq_ignore_ascii_case("td") || open.eq_ignore_ascii_case("th");
    }
    if new.eq_ignore_ascii_case("tr") {
        return name_in(open, &["td", "th", "tr"]);
    }
    if name_in(new, &["thead", "tbody", "tfoot"]) {
        return name_in(open, &["td", "th", "tr", "thead", "tbody", "tfoot"]);
    }
    if new.eq_ignore_ascii_case("option") {
        return open.eq_ignore_ascii_case("option");
    }
    if new.eq_ignore_ascii_case("optgroup") {
        return name_in(open, &["option", "optgroup"]);
    }
    false
}

// ---------------------------------------------------------------------------
// 名称与序列化
// ---------------------------------------------------------------------------

fn sanitize_name(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') || (ch as u32) >= 0x80
        {
            out.push(ch);
        }
    }
    if out.is_empty() {
        return "span".to_string();
    }
    let first_ok = out.chars().next().is_some_and(|ch| {
        ch.is_ascii_alphabetic() || ch == '_' || ch == ':' || (ch as u32) >= 0x80
    });
    if !first_ok {
        out.insert(0, 'e');
    }
    out
}

fn sanitize_attr_name(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') || (ch as u32) >= 0x80
        {
            // HTML 属性名大小写不敏感；统一小写后 `attribute("class")` 等
            // 查询才能命中 `CLASS`。NCX 里真正区分大小写的属性（如
            // `playOrder`）目前不被读取，保留小写不影响导航结构。
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

fn serialize_elem(elem: &Elem, out: &mut String) {
    out.push('<');
    out.push_str(&elem.name);
    for (name, value) in &elem.attrs {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        escape_markup(value, out, Some('"'));
        out.push('"');
    }
    if elem.self_closing || (elem.children.is_empty() && is_void(&elem.name)) {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in &elem.children {
        match child {
            Item::Elem(child) => serialize_elem(child, out),
            Item::Text(text) => escape_markup(text, out, None),
        }
    }
    out.push_str("</");
    out.push_str(&elem.name);
    out.push('>');
}

/// 转义文本/属性，但保留合法的字符引用（`&#160;`、`&amp;`…），避免二次转义。
fn escape_markup(text: &str, out: &mut String, quote: Option<char>) {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < text.len() {
        match bytes[i] {
            b'<' => {
                out.push_str("&lt;");
                i += 1;
            }
            b'>' => {
                out.push_str("&gt;");
                i += 1;
            }
            b'&' => {
                if let Some(end) = reference_end(text, i) {
                    out.push_str(&text[i..end]);
                    i = end;
                } else {
                    out.push_str("&amp;");
                    i += 1;
                }
            }
            b'"' if quote == Some('"') => {
                out.push_str("&quot;");
                i += 1;
            }
            b'\'' if quote == Some('\'') => {
                out.push_str("&apos;");
                i += 1;
            }
            _ => {
                let ch = text[i..].chars().next().expect("valid utf-8 boundary");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
}

/// `&` 处是否是合法字符引用；返回 `;` 之后的位置。
fn reference_end(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = at + 1;
    if i >= bytes.len() {
        return None;
    }
    if bytes[i] == b'#' {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'x' || bytes[i] == b'X') {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i == start {
                return None;
            }
        } else {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == start {
                return None;
            }
        }
    } else {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    if i < bytes.len() && bytes[i] == b';' {
        Some(i + 1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::repair;

    fn fixed(input: &str) -> alloc::string::String {
        let normalized = crate::entities::normalized(input);
        repair(&normalized).into_owned()
    }

    fn parses(input: &str) -> bool {
        let text = fixed(input);
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: u32::MAX,
        };
        roxmltree::Document::parse_with_options(&text, options).is_ok()
    }

    #[test]
    fn quotes_bare_attributes() {
        let out = fixed("<p><a href=http://example.com/a?x=1>go</a></p>");
        assert!(out.contains("href=\"http://example.com/a?x=1\""), "{out}");
        assert!(parses("<p><a href=http://example.com/a?x=1>go</a></p>"));
    }

    #[test]
    fn closes_void_elements() {
        assert!(parses("<p>a<br>b</p>"));
        assert!(parses("<p><img src=x.png></p>"));
    }

    #[test]
    fn recovers_mismatched_end_tags() {
        assert!(parses("<div><p>a</div>"));
        assert!(parses("<p>a<p>b</p>"));
        assert!(parses("<ul><li>a<li>b</ul>"));
    }

    #[test]
    fn wraps_multiple_roots() {
        assert!(parses("<p>a</p><p>b</p>"));
        assert!(parses("plain text"));
        assert!(parses("<head><title>x</title></head><body><p>y</p></body>"));
    }

    #[test]
    fn keeps_text_with_literal_lt() {
        let out = fixed("<p>a < b and c > d</p>");
        assert!(out.contains("a &lt; b"), "{out}");
    }

    #[test]
    fn deduplicates_attributes() {
        let out = fixed("<p id=a id=b>x</p>");
        assert_eq!(out.matches("id=").count(), 1, "{out}");
    }

    #[test]
    fn preserves_character_references() {
        let out = fixed("<p>a&#160;b &amp; c</p>");
        assert!(out.contains("&#160;"), "{out}");
        assert!(out.contains("&amp;"), "{out}");
    }

    #[test]
    fn keeps_raw_text_elements() {
        let out = fixed("<style>p > a { color: red; }</style>");
        assert!(out.contains("p &gt; a"), "{out}");
    }

    #[test]
    fn repairs_unclosed_document() {
        assert!(parses("<html><body><p>a"));
    }

    #[test]
    fn keeps_xml_tag_case() {
        // NCX 用 camelCase 标签（`navMap`/`navPoint`），XML 大小写敏感，
        // 修复层不能把它们改成小写。
        let out = fixed("<navMap><navPoint><text>x</text></navPoint></navMap>");
        assert!(out.contains("navPoint"), "{out}");
        assert!(out.contains("navMap"), "{out}");
    }

    #[test]
    fn matches_html_names_case_insensitively() {
        assert!(parses("<HTML><BODY><P>a<BR>b</BODY></HTML>"));
        let out = fixed("<UL><LI>a<LI>b</UL>");
        assert_eq!(
            out.to_ascii_lowercase().matches("</li>").count(),
            2,
            "{out}"
        );
    }
}
