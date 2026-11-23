use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ebook_ir::{
    Break, Display, FontFamilies, FontStyle, Len, LineHeight, ListStyle, NodeId, Rgba8, StyledDoc,
    Tag, TextAlign, TextTransform, VerticalAlign, WhiteSpace, WordBreak, WritingMode,
};

/// 异常 CSS 的硬边界。真实书远低于这些值；限额把病态输入变成确定性的截断，
/// 而不是让栈溢出或 `cascade` 跑成 O(nodes × rules × depth) 的无底洞。
const MAX_CSS_BYTES: usize = 16 * 1024 * 1024;
const MAX_CSS_NESTING: usize = 32;
const MAX_RULES: usize = 16_384;
const MAX_FONT_FACES: usize = 256;
const MAX_DECLS: usize = 2_048;
const MAX_RULE_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_SELECTOR_BYTES: usize = 4_096;
const MAX_SELECTOR_COMPOUNDS: usize = 32;
const MAX_CASCADE_STEPS: usize = 20_000_000;

#[derive(Clone, Debug)]
pub struct Decl {
    pub name: String,
    pub value: String,
    /// `!important`。规则里带感叹号的声明压过普通声明，内联样式也不例外。
    pub important: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ElemKey {
    pub tag: Tag,
    pub tag_name: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    /// 元素的全部属性，名字小写、值是原文。属性选择器与 `<ol start>` 都用它。
    pub attrs: Vec<(String, String)>,
    pub inline: Vec<Decl>,
    pub is_element: bool,
}

/// 一条 `@font-face`。`src` 是第一条 `url(...)` 的原文，调用方负责相对路径解析。
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
    pub src: Option<String>,
}

impl Default for FontFace {
    fn default() -> Self {
        Self {
            family: String::new(),
            weight: 400,
            style: FontStyle::Normal,
            src: None,
        }
    }
}

/// 相邻两个复合选择器之间的组合器。默认是后代组合器（空格）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Combinator {
    #[default]
    Descendant,
    Child,
    AdjacentSibling,
    GeneralSibling,
}

/// 结构性伪类。只实现只看元素兄弟的两个：文本节点不参与计数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PseudoClass {
    FirstChild,
    LastChild,
}

/// 属性选择器的六种匹配方式（加上「只要存在」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttrOp {
    Exists,
    Equals,
    Includes,
    Dash,
    Prefix,
    Suffix,
    Substring,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AttrTest {
    name: String,
    op: AttrOp,
    value: String,
    /// `[attr="v" i]`：值按 ASCII 大小写不敏感比较。
    insensitive: bool,
}

#[derive(Clone, Debug, Default)]
struct Compound {
    /// 与本复合选择器前面的那个复合选择器如何连接；第一个复合选择器忽略此字段。
    combinator: Combinator,
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<AttrTest>,
    pseudos: Vec<PseudoClass>,
}

type Spec = (u16, u16, u16);

#[derive(Clone, Debug)]
struct Rule {
    selector: Vec<Compound>,
    decls: Vec<Decl>,
    spec: Spec,
}

#[derive(Default)]
pub struct StyleSheet {
    rules: Vec<Rule>,
    font_faces: Vec<FontFace>,
    /// 顶层 `@import` 的目标，按出现顺序；由调用方解析相对路径后递归加载。
    imports: Vec<String>,
    /// 无法落到 IR 的属性名，按首次出现顺序去重。
    unsupported: Vec<String>,
}

impl StyleSheet {
    pub fn parse_into(&mut self, css: &str) {
        self.parse_level(truncate_utf8(css, MAX_CSS_BYTES), 0);
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.font_faces.is_empty() && self.imports.is_empty()
    }

    /// 顶层 `@import` 的目标；取走之后调用方负责递归加载并 `absorb`。
    /// 只记录第一层：导入文件里的 `@import` 由下一轮 `parse_into` 再收。
    pub(crate) fn take_imports(&mut self) -> Vec<String> {
        core::mem::take(&mut self.imports)
    }

    /// 把另一张表并进来，规则追加在后面（调用方保证「导入的在前、本文件的在后」）。
    /// 规则数仍受 `MAX_RULES` 约束，避免多个 `@import` 叠加绕过单表上限。
    pub(crate) fn absorb(&mut self, mut other: StyleSheet) {
        let room = MAX_RULES.saturating_sub(self.rules.len());
        if other.rules.len() > room {
            other.rules.truncate(room);
        }
        self.rules.append(&mut other.rules);
        let faces = MAX_FONT_FACES.saturating_sub(self.font_faces.len());
        if other.font_faces.len() > faces {
            other.font_faces.truncate(faces);
        }
        self.font_faces.append(&mut other.font_faces);
        for name in other.unsupported {
            if !self.unsupported.iter().any(|n| n == &name) {
                self.unsupported.push(name);
            }
        }
        self.imports.append(&mut other.imports);
    }

    pub fn font_faces(&self) -> &[FontFace] {
        &self.font_faces
    }

    pub fn unsupported(&self) -> &[String] {
        &self.unsupported
    }

    fn note_unsupported(&mut self, name: &str) {
        if !name.is_empty() && !self.unsupported.iter().any(|n| n == name) {
            self.unsupported.push(name.to_string());
        }
    }

    /// 逐层解析。`@media` 只取 screen（或无条件）分支，`@font-face` 收集起来，
    /// 其余块级 at-rule（`@supports` / `@document` / `@layer`）按无条件处理。
    fn parse_level(&mut self, css: &str, depth: usize) {
        if self.rules.len() >= MAX_RULES {
            self.note_unsupported("@rules");
            return;
        }
        if depth > MAX_CSS_NESTING {
            self.note_unsupported("@nesting");
            return;
        }
        let bytes = css.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            i = skip_space_and_comments(bytes, i);
            while i < bytes.len() && bytes[i] == b';' {
                i += 1;
                i = skip_space_and_comments(bytes, i);
            }
            if i >= bytes.len() {
                break;
            }
            if bytes[i] == b'@' {
                let mut j = i + 1;
                while j < bytes.len()
                    && ((bytes[j] as char).is_ascii_alphanumeric()
                        || bytes[j] == b'-'
                        || bytes[j] == b'_')
                {
                    j += 1;
                }
                let name = css[i + 1..j].to_ascii_lowercase();
                let mut k = j;
                while k < bytes.len() && bytes[k] != b'{' && bytes[k] != b';' {
                    k += 1;
                }
                // `@import` 是语句不是块：记下目标，由调用方解析相对路径。
                // 嵌套在 `@media` 里的 `@import` 非法，也一并忽略。
                if name == "import" && depth == 0 {
                    let end = if k < bytes.len() && bytes[k] == b';' {
                        k
                    } else {
                        bytes.len()
                    };
                    if let Some((href, media)) = parse_import(&css[j..end]) {
                        if media_is_screen(media) {
                            self.imports.push(href);
                        }
                    }
                    i = end.saturating_add(1);
                    continue;
                }
                if k >= bytes.len() {
                    break;
                }
                if bytes[k] == b';' {
                    i = k + 1;
                    continue;
                }
                let end = match_brace(bytes, k);
                let body = &css[k + 1..end];
                match name.as_str() {
                    "media" => {
                        if media_is_screen(&css[j..k]) {
                            self.parse_level(body, depth + 1);
                        }
                    }
                    "supports" | "document" | "layer" => self.parse_level(body, depth + 1),
                    "font-face"
                        if self.font_faces.len() < MAX_FONT_FACES
                            && body.len() <= MAX_RULE_BODY_BYTES =>
                    {
                        if let Some(face) = parse_font_face(body) {
                            self.font_faces.push(face);
                        }
                    }
                    _ => {}
                }
                i = end.saturating_add(1);
                continue;
            }
            if bytes[i] == b'}' {
                i += 1;
                continue;
            }
            let selector_start = i;
            while i < bytes.len() && bytes[i] != b'{' {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            let selector_text = &css[selector_start..i];
            let end = match_brace(bytes, i);
            let body = &css[i + 1..end];
            i = end.saturating_add(1);
            if body.len() > MAX_RULE_BODY_BYTES {
                self.note_unsupported("@rule");
                continue;
            }
            let decls = parse_decls(body);
            if decls.is_empty() {
                continue;
            }
            for decl in &decls {
                if !decl_supported(decl) {
                    self.note_unsupported(&decl.name);
                }
            }
            for selector_text in split_selectors(selector_text) {
                if self.rules.len() >= MAX_RULES {
                    self.note_unsupported("@rules");
                    break;
                }
                if selector_text.len() > MAX_SELECTOR_BYTES {
                    self.note_unsupported("@selector");
                    continue;
                }
                if let Some(selector) = parse_selector(selector_text) {
                    let spec = specificity(&selector);
                    self.rules.push(Rule {
                        selector,
                        decls: decls.clone(),
                        spec,
                    });
                }
            }
        }
    }
}

/// 按顶层逗号拆选择器列表：属性值里的逗号（`[title="a,b"]`）不是分隔符。
fn split_selectors(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut bracket = 0usize;
    let mut quote = 0u8;
    let mut i = 0usize;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\\' {
            i += 2;
            continue;
        }
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => {
                quote = byte;
                i += 1;
            }
            b'[' => {
                bracket += 1;
                i += 1;
            }
            b']' => {
                bracket = bracket.saturating_sub(1);
                i += 1;
            }
            b',' if bracket == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push(&text[start..]);
    out
}

/// 把一个规则块的值拆成声明。`!important` 单独记，不混在值里。
pub fn parse_decls(body: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    for part in body.split(';') {
        if out.len() >= MAX_DECLS {
            break;
        }
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((name, value)) = part.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let (value, important) = split_important(value);
            if !name.is_empty() && !value.is_empty() {
                out.push(Decl {
                    name,
                    value: value.to_string(),
                    important,
                });
            }
        }
    }
    out
}

fn split_important(value: &str) -> (&str, bool) {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(idx) = lower.rfind("!important") {
        if lower[idx + "!important".len()..].trim().is_empty() {
            return (trimmed[..idx].trim_end(), true);
        }
    }
    (trimmed, false)
}

/// 属性名是否认识。认识但 IR 没有对应字段的（`border-radius`）
/// 也算认识：它们被安静跳过，而不是报告成未知属性。
pub fn property_supported(name: &str) -> bool {
    matches!(
        name,
        "display"
            | "color"
            | "background-color"
            | "background"
            | "font"
            | "font-size"
            | "font-weight"
            | "font-style"
            | "font-family"
            | "line-height"
            | "text-align"
            | "text-indent"
            | "letter-spacing"
            | "word-spacing"
            | "white-space"
            | "word-break"
            | "hyphens"
            | "text-spacing-trim"
            | "hanging-punctuation"
            | "text-autospace"
            | "text-transform"
            | "vertical-align"
            | "writing-mode"
            | "direction"
            | "margin"
            | "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "border"
            | "border-width"
            | "border-color"
            | "border-style"
            | "border-top"
            | "border-right"
            | "border-bottom"
            | "border-left"
            | "border-radius"
            | "width"
            | "height"
            | "page-break-before"
            | "page-break-after"
            | "page-break-inside"
            | "break-before"
            | "break-after"
            | "break-inside"
            | "orphans"
            | "widows"
            | "list-style-type"
            | "list-style"
            | "text-decoration"
            | "text-decoration-line"
    )
}

/// 值是否能被理解。只对真正会影响排版的属性做检查，复合简写（`border`、
/// `background`）从宽处理，避免误报。
pub fn value_supported(name: &str, value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    if v.is_empty() {
        return false;
    }
    if matches!(v.as_str(), "inherit" | "initial" | "unset" | "revert") {
        return true;
    }
    match name {
        "display" => parse_display(&v).is_some(),
        "color" | "background-color" => parse_color(&v).is_some() || v == "currentcolor",
        "font-size" => parse_len(&v).is_some() || font_size_keyword(&v).is_some(),
        "font-weight" => parse_weight(&v, 400).is_some(),
        "font-style" => parse_font_style(&v).is_some(),
        "line-height" => v == "normal" || parse_number(&v).is_some() || parse_len(&v).is_some(),
        "text-align" => matches!(
            v.as_str(),
            "left" | "start" | "right" | "end" | "center" | "justify"
        ),
        "text-indent" | "letter-spacing" | "word-spacing" | "width" | "height" => {
            v == "normal" || v == "auto" || parse_len(&v).is_some()
        }
        "white-space" => {
            matches!(
                v.as_str(),
                "normal" | "pre" | "pre-wrap" | "pre-line" | "nowrap" | "break-spaces"
            )
        }
        "word-break" => matches!(v.as_str(), "normal" | "break-all" | "keep-all"),
        "hyphens" => matches!(v.as_str(), "auto" | "manual" | "none"),
        // CSS Text 4 的标点政策。只认我们真正实现的取值，其余（space-all /
        // trim-start / force-end 组合等）留给 issue 报告，而不是静默当成关闭。
        "text-spacing-trim" => matches!(v.as_str(), "normal" | "none"),
        // 布局只实现 allow-end（行末标点悬挂）。其余取值要报不支持，
        // 不能让 "first"/"last" 静默变成关闭。
        "hanging-punctuation" => {
            v == "none" || v.split_whitespace().any(|part| part == "allow-end")
        }
        "text-autospace" => matches!(v.as_str(), "normal" | "none"),
        "text-transform" => matches!(
            v.as_str(),
            "none" | "uppercase" | "lowercase" | "capitalize"
        ),
        "vertical-align" => {
            matches!(
                v.as_str(),
                "baseline"
                    | "super"
                    | "sub"
                    | "top"
                    | "middle"
                    | "bottom"
                    | "text-top"
                    | "text-bottom"
            ) || parse_len(&v).is_some()
        }
        "writing-mode" => matches!(v.as_str(), "horizontal-tb" | "vertical-rl" | "vertical-lr"),
        "direction" => matches!(v.as_str(), "ltr" | "rtl"),
        "margin" | "padding" => {
            let parts: Vec<&str> = v.split_whitespace().collect();
            (1..=4).contains(&parts.len()) && parts.iter().all(|p| parse_box_len(p).is_some())
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            parse_box_len(&v).is_some()
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            parse_len(&v).is_some()
        }
        "page-break-before" | "page-break-after" | "page-break-inside" | "break-before"
        | "break-after" | "break-inside" => matches!(
            v.as_str(),
            "auto" | "always" | "page" | "left" | "right" | "recto" | "verso" | "avoid"
        ),
        "orphans" | "widows" => v.parse::<u16>().is_ok(),
        _ => true,
    }
}

pub fn decl_supported(decl: &Decl) -> bool {
    property_supported(&decl.name) && value_supported(&decl.name, &decl.value)
}

/// 从一段 CSS 里收集所有 `@font-face`（跳过 `@media` 的 print 分支）。
pub fn parse_font_faces(css: &str) -> Vec<FontFace> {
    let mut sheet = StyleSheet::default();
    sheet.parse_into(css);
    sheet.font_faces
}

pub fn parse_font_face(body: &str) -> Option<FontFace> {
    let decls = parse_decls(body);
    let family = decls
        .iter()
        .find(|d| d.name == "font-family")
        .map(|d| unquote(&d.value))
        .filter(|f| !f.is_empty())?;
    let mut face = FontFace {
        family,
        ..FontFace::default()
    };
    for decl in &decls {
        match decl.name.as_str() {
            "font-weight" => {
                if let Some(weight) = parse_weight(&decl.value, 400) {
                    face.weight = weight;
                }
            }
            "font-style" => {
                if let Some(style) = parse_font_style(&decl.value) {
                    face.style = style;
                }
            }
            "src" if face.src.is_none() => face.src = first_css_url(&decl.value),
            _ => {}
        }
    }
    Some(face)
}

fn first_css_url(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    if let Some(idx) = lower.find("url(") {
        let rest = &value[idx + 4..];
        if let Some(end) = rest.find(')') {
            let url = unquote(rest[..end].trim());
            if !url.is_empty() {
                return Some(url);
            }
        }
    }
    None
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '\'' || c == '"')
        .trim()
        .to_string()
}

/// 拆 `@import` 的 prelude：`url(...)` / 引号字符串 / 裸 URL + 媒体查询。
fn parse_import(prelude: &str) -> Option<(String, &str)> {
    let text = prelude.trim();
    let (href, rest) = if text.len() >= 4 && text[..4].eq_ignore_ascii_case("url(") {
        let after = &text[4..];
        let close = after.find(')')?;
        (after[..close].trim(), &after[close + 1..])
    } else if let Some(quote) = text.chars().next().filter(|c| matches!(c, '"' | '\'')) {
        let after = &text[quote.len_utf8()..];
        let close = after.find(quote)?;
        (&after[..close], &after[close + quote.len_utf8()..])
    } else {
        let end = text.find(char::is_whitespace).unwrap_or(text.len());
        (&text[..end], &text[end..])
    };
    let href = href.trim().trim_matches(|c| matches!(c, '"' | '\'')).trim();
    if href.is_empty() {
        return None;
    }
    Some((href.to_string(), rest.trim()))
}

fn media_is_screen(prelude: &str) -> bool {
    let lower = prelude.to_ascii_lowercase();
    // 只有 print 且不带 screen 的媒体查询才跳过；其余（含 all/空）都算屏幕。
    !lower.contains("print") || lower.contains("screen")
}

/// 从 `open`（必须指向 `{`）找到配对的 `}`，返回它的下标；没有闭合就返回长度。
fn match_brace(bytes: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

pub fn cascade(doc: &mut StyledDoc, keys: &[ElemKey], sheet: &StyleSheet) {
    let order: Vec<NodeId> = doc.walk().collect();
    let aux = MatchAux::new(doc, keys);
    let mut matched: Vec<(Spec, usize)> = Vec::new();
    let mut budget = MAX_CASCADE_STEPS;
    let mut exhausted = false;
    for id in order {
        let parent_style = doc
            .node(id)
            .parent
            .map(|p| doc.node(p).style)
            .unwrap_or_default();
        let mut style = inherit(&parent_style);
        ua_apply(&mut style, keys[id as usize].tag);
        // `list-style-type` 是继承属性，但 `list` 字段不参与 `inherit()`：
        // 只有列表项需要它，复制到每个文本节点会白白改 IR 字节。
        if style.display == Display::ListItem && style.list.is_none() {
            style.list = parent_style.list;
        }
        // `<ol start="3">` 是 UA 级信息，不在 CSS 里；attribute 已在 ElemKey 上。
        if keys[id as usize].tag == Tag::Ol {
            if let Some((_, value)) = keys[id as usize]
                .attrs
                .iter()
                .find(|(name, _)| name == "start")
            {
                if let Ok(start) = value.trim().parse::<u16>() {
                    style.list = Some(ListStyle {
                        ordered: true,
                        start: start.max(1),
                    });
                }
            }
        }

        matched.clear();
        if !exhausted {
            for (i, rule) in sheet.rules.iter().enumerate() {
                if budget == 0 {
                    exhausted = true;
                    break;
                }
                if matches(doc, keys, &aux, id, &rule.selector, &mut budget) {
                    matched.push((rule.spec, i));
                }
            }
        }
        matched.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let parent_size = parent_style.font.size;
        let parent_weight = parent_style.font.weight;
        let parent_color = parent_style.color;
        // 普通声明先按 specificity 叠，再叠内联；带 !important 的声明最后叠，
        // 因而能压过内联的普通声明（CSS 的层叠顺序）。
        for (_, i) in matched.iter() {
            for decl in sheet.rules[*i].decls.iter().filter(|d| !d.important) {
                apply(&mut style, decl, parent_size, parent_weight, parent_color);
            }
        }
        for decl in keys[id as usize].inline.iter().filter(|d| !d.important) {
            apply(&mut style, decl, parent_size, parent_weight, parent_color);
        }
        for (_, i) in matched.iter() {
            for decl in sheet.rules[*i].decls.iter().filter(|d| d.important) {
                apply(&mut style, decl, parent_size, parent_weight, parent_color);
            }
        }
        for decl in keys[id as usize].inline.iter().filter(|d| d.important) {
            apply(&mut style, decl, parent_size, parent_weight, parent_color);
        }
        doc.node_mut(id).style = style;
    }
}

/// 预先算好元素兄弟关系，避免每个节点都重新扫一遍 `kids`：
/// `li + li` / `li:last-child` 这类规则在长列表上原本是 O(n²)。
struct MatchAux {
    prev_element: Vec<Option<NodeId>>,
    first_element: Vec<Option<NodeId>>,
    last_element: Vec<Option<NodeId>>,
}

impl MatchAux {
    fn new(doc: &StyledDoc, keys: &[ElemKey]) -> Self {
        let count = doc.nodes.len();
        let mut aux = Self {
            prev_element: alloc::vec![None; count],
            first_element: alloc::vec![None; count],
            last_element: alloc::vec![None; count],
        };
        let mut last_by_parent = alloc::vec![None; count];
        for id in doc.walk() {
            if !keys[id as usize].is_element {
                continue;
            }
            let Some(parent) = doc.node(id).parent else {
                continue;
            };
            let parent = parent as usize;
            if aux.first_element[parent].is_none() {
                aux.first_element[parent] = Some(id);
            }
            aux.last_element[parent] = Some(id);
            aux.prev_element[id as usize] = last_by_parent[parent];
            last_by_parent[parent] = Some(id);
        }
        aux
    }
}

fn inherit(parent: &ebook_ir::ComputedStyle) -> ebook_ir::ComputedStyle {
    let mut text = parent.text;
    text.vertical_align = VerticalAlign::Baseline;
    ebook_ir::ComputedStyle {
        font: parent.font,
        color: parent.color,
        line_height: parent.line_height,
        text,
        writing_mode: parent.writing_mode,
        dir: parent.dir,
        ..ebook_ir::ComputedStyle::default()
    }
}

fn ua_apply(style: &mut ebook_ir::ComputedStyle, tag: Tag) {
    let size = style.font.size;
    match tag {
        Tag::Head | Tag::Style | Tag::Title | Tag::Meta | Tag::Link | Tag::Script => {
            style.display = Display::None
        }
        Tag::Strong => {
            style.display = Display::Inline;
            style.font.weight = 700;
        }
        Tag::Em => {
            style.display = Display::Inline;
            style.font.style = FontStyle::Italic;
        }
        Tag::A => {
            style.display = Display::Inline;
            // 浏览器 UA 默认的链接蓝与下划线；书里的 CSS 仍可覆盖。
            style.color = ebook_ir::Rgba8::new(26, 95, 180, 255);
            style.text.underline = true;
        }
        Tag::Span | Tag::Code | Tag::Ruby | Tag::Rt => style.display = Display::Inline,
        Tag::Br => style.display = Display::Inline,
        Tag::Img => style.display = Display::InlineBlock,
        Tag::Li => {
            style.display = Display::ListItem;
            style.margin.left = Len::Em(1.5);
        }
        Tag::Ul => {
            style.display = Display::Block;
            style.padding.left = Len::Em(1.5);
            style.list = Some(ListStyle {
                ordered: false,
                start: 1,
            });
        }
        Tag::Ol => {
            style.display = Display::Block;
            style.padding.left = Len::Em(1.5);
            style.list = Some(ListStyle {
                ordered: true,
                start: 1,
            });
        }
        Tag::Blockquote => {
            style.display = Display::Block;
            style.margin.left = Len::Em(1.5);
            style.margin.right = Len::Em(1.5);
        }
        Tag::Pre => {
            style.display = Display::Block;
            style.text.white_space = WhiteSpace::Pre;
        }
        Tag::H1 => heading(style, size, 2.0, 0.9),
        Tag::H2 => heading(style, size, 1.5, 0.8),
        Tag::H3 => heading(style, size, 1.3, 0.7),
        Tag::H4 => heading(style, size, 1.15, 0.6),
        Tag::H5 => heading(style, size, 1.0, 0.5),
        Tag::H6 => heading(style, size, 0.9, 0.5),
        Tag::P => {
            style.display = Display::Block;
            style.margin.bottom = Len::Em(0.6);
        }
        Tag::Table => style.display = Display::Block,
        Tag::Tr | Tag::Td => style.display = Display::Block,
        _ => style.display = Display::Block,
    }
}

fn heading(style: &mut ebook_ir::ComputedStyle, size: f32, scale: f32, margin: f32) {
    style.display = Display::Block;
    style.brk.after = Break::Avoid;
    style.font.size = (size * scale).max(1.0);
    style.font.weight = 700;
    style.margin.top = Len::Em(margin);
    style.margin.bottom = Len::Em(margin * 0.6);
}

fn apply(
    style: &mut ebook_ir::ComputedStyle,
    decl: &Decl,
    parent_size: f32,
    parent_weight: u16,
    parent_color: Rgba8,
) {
    let value = decl.value.trim();
    let base = style.font.size;
    match decl.name.as_str() {
        "display" => {
            if let Some(display) = parse_display(value) {
                style.display = display;
            }
        }
        // `currentColor` 不是固定颜色值，必须在这里按上下文求值。
        // `color: currentColor` 等价于继承父元素的颜色。
        "color" => {
            if is_current_color(value) {
                style.color = parent_color;
            } else if let Some(color) = parse_color(value) {
                style.color = color;
            }
        }
        // 声明按层叠顺序逐条应用，所以 `background-color: currentColor` 取的是
        // **此刻**元素已经算出的 `color`。若同一条规则里先写 background 再写
        // color，背景用的是写背景之前的颜色。测试固定了这个顺序语义。
        "background-color" | "background" => {
            let color = if is_current_color(value) {
                Some(style.color)
            } else {
                parse_color(value)
            };
            if let Some(color) = color {
                style.background = if color.a == 0 { None } else { Some(color) };
            }
        }
        "font-size" => {
            if let Some(px) = font_size(value, parent_size) {
                style.font.size = px.max(1.0);
            }
        }
        // font 简写：`font: italic 700 12px/1.5 serif`。
        "font" => apply_font_shorthand(style, value, parent_size, parent_weight),
        "font-weight" => {
            if let Some(weight) = parse_weight(value, parent_weight) {
                style.font.weight = weight;
            }
        }
        "font-style" => {
            if let Some(style_value) = parse_font_style(value) {
                style.font.style = style_value;
            }
        }
        "font-family" => {
            let families = parse_font_families(value);
            if !families.is_empty() {
                style.font.families = families;
            }
        }
        "line-height" => {
            if value == "normal" {
                style.line_height = LineHeight::Normal;
            } else if let Some(n) = parse_number(value) {
                style.line_height = LineHeight::Mul(n);
            } else if let Some(px) = parse_len(value).and_then(|l| len_px(l, base)) {
                style.line_height = LineHeight::Px(px);
            }
        }
        "text-align" => {
            style.text.align = match value {
                "center" => TextAlign::Center,
                "right" | "end" => TextAlign::End,
                "justify" => TextAlign::Justify,
                _ => TextAlign::Start,
            }
        }
        "text-indent" => {
            if let Some(l) = parse_len(value) {
                style.text.indent = l;
            }
        }
        "letter-spacing" => {
            if let Some(px) = parse_len(value).and_then(|l| len_px(l, base)) {
                style.text.letter_spacing = px;
            }
        }
        "word-spacing" => {
            if let Some(px) = parse_len(value).and_then(|l| len_px(l, base)) {
                style.text.word_spacing = px;
            }
        }
        "white-space" => {
            style.text.white_space = match value.to_ascii_lowercase().as_str() {
                "pre" => WhiteSpace::Pre,
                "nowrap" => WhiteSpace::Nowrap,
                "pre-wrap" => WhiteSpace::PreWrap,
                "pre-line" => WhiteSpace::PreLine,
                "break-spaces" => WhiteSpace::BreakSpaces,
                _ => WhiteSpace::Normal,
            }
        }
        "word-break" => {
            style.text.word_break = match value {
                "break-all" => WordBreak::BreakAll,
                "keep-all" => WordBreak::KeepAll,
                _ => WordBreak::Normal,
            }
        }
        "hyphens" => style.text.hyphens = value == "auto",
        "text-spacing-trim" => style.text.rules.spacing_trim = value != "none",
        "hanging-punctuation" => {
            style.text.rules.hanging_punctuation =
                value.split_whitespace().any(|part| part == "allow-end");
        }
        "text-autospace" => style.text.rules.autospacing = value != "none",
        "text-transform" => {
            style.text.transform = match value {
                "uppercase" => TextTransform::Uppercase,
                "lowercase" => TextTransform::Lowercase,
                _ => TextTransform::None,
            }
        }
        "vertical-align" => {
            style.text.vertical_align = match value {
                "baseline" => VerticalAlign::Baseline,
                "super" => VerticalAlign::Super,
                "sub" => VerticalAlign::Sub,
                "top" => VerticalAlign::Top,
                "middle" => VerticalAlign::Middle,
                "bottom" => VerticalAlign::Bottom,
                "text-top" => VerticalAlign::TextTop,
                "text-bottom" => VerticalAlign::TextBottom,
                other => parse_len(other).map(VerticalAlign::Len).unwrap_or_default(),
            }
        }
        "writing-mode" => {
            style.writing_mode = match value {
                "vertical-rl" => WritingMode::VerticalRl,
                "vertical-lr" => WritingMode::VerticalLr,
                _ => WritingMode::HorizontalTb,
            }
        }
        "direction" => {
            style.dir = if value == "rtl" {
                ebook_ir::Dir::Rtl
            } else {
                ebook_ir::Dir::Ltr
            }
        }
        "margin" => set_sides(&mut style.margin, value),
        "margin-top" => set_one(&mut style.margin.top, value),
        "margin-right" => set_one(&mut style.margin.right, value),
        "margin-bottom" => set_one(&mut style.margin.bottom, value),
        "margin-left" => set_one(&mut style.margin.left, value),
        "padding" => set_sides(&mut style.padding, value),
        "padding-top" => set_one(&mut style.padding.top, value),
        "padding-right" => set_one(&mut style.padding.right, value),
        "padding-bottom" => set_one(&mut style.padding.bottom, value),
        "padding-left" => set_one(&mut style.padding.left, value),
        "border" => {
            apply_border(style, value);
        }
        "border-width" => set_sides(&mut style.border.width, value),
        "border-color" => {
            if let Some(color) = parse_color(value) {
                style.border.color = color;
            }
        }
        "width" => {
            if let Some(l) = parse_len(value) {
                style.width = l;
            }
        }
        "height" => {
            if let Some(l) = parse_len(value) {
                style.height = l;
            }
        }
        "page-break-before" | "break-before" => style.brk.before = parse_break(value),
        "page-break-after" | "break-after" => style.brk.after = parse_break(value),
        "page-break-inside" | "break-inside" => style.brk.inside = parse_break(value),
        "orphans" => {
            if let Ok(n) = value.parse::<u16>() {
                style.brk.orphans = n.max(1);
            }
        }
        "widows" => {
            if let Ok(n) = value.parse::<u16>() {
                style.brk.widows = n.max(1);
            }
        }
        "list-style-type" => match value {
            "none" => style.list = None,
            "decimal" | "lower-alpha" | "upper-alpha" | "lower-roman" | "upper-roman" => {
                // 只改类型，不重置 `<ol start>` 已经给出的起点。
                let start = style.list.map_or(1, |list| list.start);
                style.list = Some(ListStyle {
                    ordered: true,
                    start,
                })
            }
            "disc" | "circle" | "square" => {
                style.list = Some(ListStyle {
                    ordered: false,
                    start: 1,
                })
            }
            _ => {}
        },
        "list-style" => {
            let tokens: Vec<&str> = value.split_whitespace().collect();
            if tokens.contains(&"none") {
                style.list = None;
            } else {
                let ordered = tokens.iter().any(|token| {
                    matches!(
                        *token,
                        "decimal" | "lower-alpha" | "upper-alpha" | "lower-roman" | "upper-roman"
                    )
                });
                let start = style.list.map_or(1, |list| list.start);
                style.list = Some(ListStyle { ordered, start });
            }
        }
        // IR 只有下划线，`line-through` / `overline` / 圆角暂不支持。
        "text-decoration" | "text-decoration-line" => {
            style.text.underline = value
                .split_whitespace()
                .any(|part| part.eq_ignore_ascii_case("underline"));
        }
        "border-radius" => {}
        _ => {}
    }
}

fn font_size_keyword(value: &str) -> Option<f32> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "xx-small" => 9.0,
        "x-small" => 10.0,
        "small" => 13.0,
        "medium" => 16.0,
        "large" => 18.0,
        "x-large" => 24.0,
        "xx-large" => 32.0,
        "smaller" => 13.0,
        "larger" => 19.0,
        _ => return None,
    })
}

fn font_size(value: &str, parent_size: f32) -> Option<f32> {
    if let Some(px) = parse_len(value).and_then(|l| len_px(l, parent_size)) {
        return Some(px);
    }
    font_size_keyword(value)
}

/// `font` 简写。尺寸之前的词按 style/weight/variant 解释，尺寸之后的全部当字体栈。
fn apply_font_shorthand(
    style: &mut ebook_ir::ComputedStyle,
    value: &str,
    parent_size: f32,
    parent_weight: u16,
) {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    let mut size_at = None;
    let mut size_len = None;
    let mut line_height = None;
    for (index, token) in tokens.iter().enumerate() {
        let (size_text, lh_text) = match token.split_once('/') {
            Some((size, lh)) => (size, Some(lh)),
            None => (*token, None),
        };
        let parsed = parse_len(size_text);
        if parsed.is_some() || font_size_keyword(size_text).is_some() {
            size_at = Some(index + 1);
            size_len = Some(size_text.to_string());
            if let Some(lh) = lh_text {
                line_height = Some(lh.to_string());
            }
            break;
        }
        match token.to_ascii_lowercase().as_str() {
            "italic" | "oblique" => style.font.style = parse_font_style(token).unwrap_or_default(),
            "bold" | "bolder" | "lighter" => {
                if let Some(weight) = parse_weight(token, parent_weight) {
                    style.font.weight = weight;
                }
            }
            _ => {
                if let Some(weight) = parse_weight(token, parent_weight) {
                    // 只有纯数字才当字重，避免把字体族吃进前缀。
                    if token.chars().all(|c| c.is_ascii_digit()) {
                        style.font.weight = weight;
                    }
                }
            }
        }
    }
    let Some(size_at) = size_at else {
        return;
    };
    if let Some(text) = size_len {
        if let Some(px) = font_size(&text, parent_size) {
            style.font.size = px.max(1.0);
        }
    }
    if let Some(lh) = line_height {
        if lh.eq_ignore_ascii_case("normal") {
            style.line_height = LineHeight::Normal;
        } else if let Some(n) = parse_number(&lh) {
            style.line_height = LineHeight::Mul(n);
        } else if let Some(px) = parse_len(&lh).and_then(|l| len_px(l, style.font.size)) {
            style.line_height = LineHeight::Px(px);
        }
    }
    let families = parse_font_families(&tokens[size_at..].join(" "));
    if !families.is_empty() {
        style.font.families = families;
    }
}

fn parse_font_style(value: &str) -> Option<FontStyle> {
    let value = value.trim().to_ascii_lowercase();
    if value == "normal" {
        Some(FontStyle::Normal)
    } else if value == "italic" {
        Some(FontStyle::Italic)
    } else if value == "oblique" || value.starts_with("oblique ") {
        Some(FontStyle::Oblique)
    } else {
        None
    }
}

fn parse_weight(value: &str, current: u16) -> Option<u16> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "normal" => Some(400),
        "bold" => Some(700),
        "bolder" => Some(if current < 350 {
            400
        } else if current < 550 {
            700
        } else if current < 900 {
            900
        } else {
            900
        }),
        "lighter" => Some(if current < 550 {
            100
        } else if current < 750 {
            400
        } else {
            700
        }),
        other => other
            .parse::<f32>()
            .ok()
            .filter(|weight| weight.is_finite())
            .map(|weight| (weight.clamp(1.0, 1000.0) + 0.5) as u16),
    }
}

fn parse_font_families(value: &str) -> FontFamilies {
    let mut families = FontFamilies::new();
    for part in value.split(',') {
        let name = part.trim().trim_matches(|c| c == '\'' || c == '"').trim();
        if !name.is_empty() {
            families.push(name);
        }
    }
    families
}

fn apply_border(style: &mut ebook_ir::ComputedStyle, value: &str) {
    for token in value.split_whitespace() {
        if let Some(px) = parse_len(token).and_then(|l| len_px(l, style.font.size)) {
            style.border.width = ebook_ir::Sides::all(Len::Px(px));
        } else if let Some(color) = parse_color(token) {
            style.border.color = color;
        }
    }
}

/// 盒模型的单个分量：`auto` 保留为 `Len::Auto`（`margin: 0 auto` 居中）。
fn parse_box_len(value: &str) -> Option<Len> {
    if value.trim().eq_ignore_ascii_case("auto") {
        Some(Len::Auto)
    } else {
        parse_len(value)
    }
}

fn set_one(target: &mut Len, value: &str) {
    if let Some(l) = parse_box_len(value) {
        *target = l;
    }
}

fn set_sides(sides: &mut ebook_ir::Sides, value: &str) {
    let parts: Vec<&str> = value.split_whitespace().collect();
    // 只要有一个分量读不出来就整条放弃，避免 `margin: 0 auto` 被错读成四边 0。
    let values: Option<Vec<Len>> = parts.iter().map(|p| parse_box_len(p)).collect();
    let Some(values) = values else {
        return;
    };
    match values.len() {
        1 => *sides = ebook_ir::Sides::all(values[0]),
        2 => {
            sides.top = values[0];
            sides.bottom = values[0];
            sides.right = values[1];
            sides.left = values[1];
        }
        3 => {
            sides.top = values[0];
            sides.right = values[1];
            sides.left = values[1];
            sides.bottom = values[2];
        }
        4 => {
            sides.top = values[0];
            sides.right = values[1];
            sides.bottom = values[2];
            sides.left = values[3];
        }
        _ => {}
    }
}

fn parse_break(value: &str) -> Break {
    match value {
        "always" | "page" | "left" | "right" => Break::Always,
        "avoid" => Break::Avoid,
        _ => Break::Auto,
    }
}

fn parse_display(value: &str) -> Option<Display> {
    Some(match value {
        "block" => Display::Block,
        "inline" => Display::Inline,
        "inline-block" => Display::InlineBlock,
        "list-item" => Display::ListItem,
        "none" => Display::None,
        _ => return None,
    })
}

pub fn parse_len(value: &str) -> Option<Len> {
    let value = value.trim();
    if value == "auto" || value == "inherit" || value == "initial" || value == "normal" {
        return None;
    }
    if value == "0" {
        return Some(Len::Px(0.0));
    }
    if let Some(n) = value.strip_suffix("px") {
        return n.trim().parse::<f32>().ok().map(Len::Px);
    }
    // 绝对单位按 CSS 定义直接换成 px：1in = 96px，1pc = 16px，1pt = 4/3 px。
    // 先匹配 pt 再匹配 pc，二者不互相包含，但保持与其它单位一致的显式顺序。
    if let Some(n) = value.strip_suffix("pt") {
        return n.trim().parse::<f32>().ok().map(|n| Len::Px(n * 4.0 / 3.0));
    }
    if let Some(n) = value.strip_suffix("pc") {
        return n.trim().parse::<f32>().ok().map(|n| Len::Px(n * 16.0));
    }
    if let Some(n) = value.strip_suffix("in") {
        return n.trim().parse::<f32>().ok().map(|n| Len::Px(n * 96.0));
    }
    if let Some(n) = value.strip_suffix("cm") {
        return n
            .trim()
            .parse::<f32>()
            .ok()
            .map(|n| Len::Px(n * 96.0 / 2.54));
    }
    if let Some(n) = value.strip_suffix("mm") {
        return n
            .trim()
            .parse::<f32>()
            .ok()
            .map(|n| Len::Px(n * 96.0 / 25.4));
    }
    if let Some(n) = value.strip_suffix("rem") {
        return n.trim().parse::<f32>().ok().map(Len::Rem);
    }
    if let Some(n) = value.strip_suffix("em") {
        return n.trim().parse::<f32>().ok().map(Len::Em);
    }
    if let Some(n) = value.strip_suffix('%') {
        return n.trim().parse::<f32>().ok().map(Len::Pct);
    }
    None
}

fn parse_number(value: &str) -> Option<f32> {
    value.trim().parse::<f32>().ok()
}

fn len_px(len: Len, base: f32) -> Option<f32> {
    match len {
        Len::Px(v) => Some(v),
        Len::Pct(v) => Some(base * v / 100.0),
        Len::Em(v) => Some(base * v),
        Len::Rem(v) => Some(16.0 * v),
        Len::Auto => None,
    }
}

/// `currentColor` 大小写不敏感；它需要元素上下文，所以不归 `parse_color` 管。
fn is_current_color(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("currentcolor")
}

pub fn parse_color(value: &str) -> Option<Rgba8> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("transparent") {
        return Some(Rgba8::TRANSPARENT);
    }
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = value.to_ascii_lowercase();
    if let Some(inner) = lower.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        return parse_rgb(inner);
    }
    if let Some(inner) = lower
        .strip_prefix("rgba(")
        .and_then(|v| v.strip_suffix(')'))
    {
        return parse_rgb(inner);
    }
    if let Some(inner) = lower.strip_prefix("hsl(").and_then(|v| v.strip_suffix(')')) {
        return parse_hsl(inner);
    }
    if let Some(inner) = lower
        .strip_prefix("hsla(")
        .and_then(|v| v.strip_suffix(')'))
    {
        return parse_hsl(inner);
    }
    let named = match lower.as_str() {
        "black" => Rgba8::BLACK,
        "white" => Rgba8::WHITE,
        "red" => Rgba8::new(255, 0, 0, 255),
        "green" => Rgba8::new(0, 128, 0, 255),
        "blue" => Rgba8::new(0, 0, 255, 255),
        "gray" | "grey" => Rgba8::new(128, 128, 128, 255),
        "silver" => Rgba8::new(192, 192, 192, 255),
        "maroon" => Rgba8::new(128, 0, 0, 255),
        "navy" => Rgba8::new(0, 0, 128, 255),
        "orange" => Rgba8::new(255, 165, 0, 255),
        _ => return None,
    };
    Some(named)
}

/// `rgb()` / `rgba()` 的函数体。分量接受 0–255 的数字或百分比，
/// 第 4 个分量（两种函数都接受）是 0–1 的数字或百分比；越界一律钳制。
fn parse_rgb(inner: &str) -> Option<Rgba8> {
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if !matches!(parts.len(), 3 | 4) {
        return None;
    }
    let r = parse_color_component(parts[0])?;
    let g = parse_color_component(parts[1])?;
    let b = parse_color_component(parts[2])?;
    let a = match parts.get(3) {
        Some(part) => parse_alpha(part)?,
        None => 255,
    };
    Some(Rgba8::new(r, g, b, a))
}

fn parse_color_component(part: &str) -> Option<u8> {
    let value = if let Some(percent) = part.strip_suffix('%') {
        parse_finite(percent)? / 100.0 * 255.0
    } else {
        parse_finite(part)?
    };
    // no_std 下没有 f32::round，钳制后的非负数加 0.5 再截断即可四舍五入。
    Some((value.clamp(0.0, 255.0) + 0.5) as u8)
}

fn parse_alpha(part: &str) -> Option<u8> {
    let value = if let Some(percent) = part.strip_suffix('%') {
        parse_finite(percent)? / 100.0
    } else {
        parse_finite(part)?
    };
    Some((value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// `hsl()` / `hsla()` 的函数体。色相接受裸数字与 `deg`，负值绕回；饱和度、
/// 亮度必须是百分比；alpha 与 `rgb()` 共用规则（0–1 或百分比）。越界一律钳制。
fn parse_hsl(inner: &str) -> Option<Rgba8> {
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if !matches!(parts.len(), 3 | 4) {
        return None;
    }
    let hue = parse_hue(parts[0])?;
    let sat = parse_percent(parts[1])?;
    let light = parse_percent(parts[2])?;
    let a = match parts.get(3) {
        Some(part) => parse_alpha(part)?,
        None => 255,
    };
    let (r, g, b) = hsl_to_rgb(hue, sat, light);
    Some(Rgba8::new(r, g, b, a))
}

fn parse_hue(part: &str) -> Option<f32> {
    let value = part.strip_suffix("deg").unwrap_or(part);
    // 绕回 [0, 360)：`hsl(-120, …)` 与 240 等价。no_std 下没有 `rem_euclid`。
    let mut degrees = parse_finite(value)? % 360.0;
    if degrees < 0.0 {
        degrees += 360.0;
    }
    Some(degrees)
}

fn parse_percent(part: &str) -> Option<f32> {
    let percent = part.strip_suffix('%')?;
    Some((parse_finite(percent)? / 100.0).clamp(0.0, 1.0))
}

/// CSS Color 3 的 HSL → RGB：先算色度 `c` 与次级分量 `x`，再加亮度基线 `m`。
fn hsl_to_rgb(hue: f32, sat: f32, light: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let h = hue / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = light - c / 2.0;
    let byte = |value: f32| ((value.clamp(0.0, 1.0) * 255.0) + 0.5) as u8;
    (byte(r + m), byte(g + m), byte(b + m))
}

fn parse_finite(value: &str) -> Option<f32> {
    value.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

fn parse_hex(hex: &str) -> Option<Rgba8> {
    if !hex.is_ascii() {
        return None;
    }
    let expand = |c: u8| -> Option<u8> {
        let v = (c as char).to_digit(16)? as u8;
        Some(v * 17)
    };
    let pair = |text: &str| -> Option<u8> { u8::from_str_radix(text, 16).ok() };
    let bytes = hex.as_bytes();
    match hex.len() {
        3 | 4 => Some(Rgba8::new(
            expand(bytes[0])?,
            expand(bytes[1])?,
            expand(bytes[2])?,
            if hex.len() == 4 {
                expand(bytes[3])?
            } else {
                255
            },
        )),
        6 | 8 => Some(Rgba8::new(
            pair(&hex[0..2])?,
            pair(&hex[2..4])?,
            pair(&hex[4..6])?,
            if hex.len() == 8 {
                pair(&hex[6..8])?
            } else {
                255
            },
        )),
        _ => None,
    }
}

/// 解析一条选择器（逗号已经由外层拆开）。只认识后代 / `>` / `+` 三种
/// 组合器与 `:first-child` / `:last-child`；其它语法返回 `None`，整条规则丢弃。
fn parse_selector(text: &str) -> Option<Vec<Compound>> {
    let bytes = text.as_bytes();
    let mut out: Vec<Compound> = Vec::new();
    let mut pending: Option<Combinator> = None;
    let mut i = 0usize;
    loop {
        while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let combinator = match bytes[i] {
            b'>' => {
                i += 1;
                Combinator::Child
            }
            b'+' => {
                i += 1;
                Combinator::AdjacentSibling
            }
            b'~' => {
                i += 1;
                Combinator::GeneralSibling
            }
            // 函数式伪类（`:not(...)`）继续整条丢弃，不降级成通配符。
            b'(' | b')' => return None,
            _ => Combinator::Descendant,
        };
        if combinator != Combinator::Descendant {
            // 组合器不能出现在开头，也不能连续出现。
            if out.is_empty() || pending.is_some() {
                return None;
            }
            pending = Some(combinator);
            continue;
        }
        let start = i;
        let mut bracket = 0usize;
        let mut quote = 0u8;
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
                b'"' | b'\'' => {
                    quote = byte;
                    i += 1;
                }
                b'[' => {
                    bracket += 1;
                    i += 1;
                }
                b']' => {
                    bracket = bracket.saturating_sub(1);
                    i += 1;
                }
                b'(' if bracket == 0 => return None,
                _ if bracket == 0
                    && ((byte as char).is_ascii_whitespace()
                        || matches!(byte, b'>' | b'+' | b'~')) =>
                {
                    break;
                }
                _ => i += 1,
            }
        }
        if i == start {
            return None;
        }
        let mut compound = parse_compound(&text[start..i])?;
        if out.is_empty() {
            if pending.is_some() {
                return None;
            }
        } else {
            compound.combinator = pending.take().unwrap_or(Combinator::Descendant);
        }
        if out.len() >= MAX_SELECTOR_COMPOUNDS {
            return None;
        }
        out.push(compound);
    }
    if pending.is_some() || out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn valid_compound_name(value: &str) -> bool {
    !value.is_empty()
        && !value.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '.' | '#' | ':' | '>' | '+' | '~' | '[' | ']' | '(' | ')' | '*'
                )
        })
}

fn parse_compound(token: &str) -> Option<Compound> {
    let mut compound = Compound::default();
    let mut rest = token;
    // 通配符只允许单独出现或后面直接跟伪类（`*:first-child`）。
    if let Some(after) = rest.strip_prefix('*') {
        if after.is_empty() {
            return Some(compound);
        }
        // `*` 之后只能接类/ID/属性/伪类，不能再接另一个 tag。
        if !matches!(after.as_bytes().first(), Some(b':' | b'[' | b'.' | b'#')) {
            return None;
        }
        rest = after;
    }
    let mut has_tag = false;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('#') {
            let end = after.find(['.', '#', ':', '[']).unwrap_or(after.len());
            if !valid_compound_name(&after[..end]) {
                return None;
            }
            compound.id = Some(after[..end].to_string());
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '#', ':', '[']).unwrap_or(after.len());
            if !valid_compound_name(&after[..end]) {
                return None;
            }
            compound.classes.push(after[..end].to_string());
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix(':') {
            // `::before` 这类伪元素不在本轮范围。
            if after.starts_with(':') {
                return None;
            }
            let end = after.find(['.', '#', ':', '[']).unwrap_or(after.len());
            let pseudo = match after[..end].to_ascii_lowercase().as_str() {
                "first-child" => PseudoClass::FirstChild,
                "last-child" => PseudoClass::LastChild,
                _ => return None,
            };
            compound.pseudos.push(pseudo);
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let end = attr_end(after)?;
            compound.attrs.push(parse_attr_test(&after[..end])?);
            rest = &after[end + 1..];
        } else {
            if has_tag {
                return None;
            }
            let end = rest.find(['.', '#', ':', '[']).unwrap_or(rest.len());
            let name = &rest[..end];
            if !name.chars().all(|c| c.is_alphanumeric() || c == '-') {
                return None;
            }
            compound.tag = Some(name.to_ascii_lowercase());
            has_tag = true;
            rest = &rest[end..];
        }
    }
    Some(compound)
}

/// `[` 之后到匹配的 `]` 的字节下标，引号里的 `]` 不算结束。
fn attr_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut quote = 0u8;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\\' {
            i += 2;
            continue;
        }
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => {
                quote = byte;
                i += 1;
            }
            b']' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// 读一个 CSS 转义并写入 `out`，返回吃掉的字节数（`text` 必须以 `\` 开头）。
fn css_unescape_into(out: &mut String, text: &str) -> usize {
    let bytes = text.as_bytes();
    if bytes.len() < 2 {
        return 1;
    }
    if bytes[1] == b'\n' {
        return 2;
    }
    if (bytes[1] as char).is_ascii_hexdigit() {
        let mut value = 0u32;
        let mut consumed = 1usize;
        let mut digits = 0;
        while consumed < bytes.len() && digits < 6 {
            let Some(digit) = (bytes[consumed] as char).to_digit(16) else {
                break;
            };
            value = value.saturating_mul(16).saturating_add(digit);
            consumed += 1;
            digits += 1;
        }
        // 一个可选的空白属于转义的一部分。
        if let Some(ch) = text[consumed..].chars().next() {
            if ch.is_ascii_whitespace() {
                consumed += ch.len_utf8();
            }
        }
        if let Some(ch) = char::from_u32(value) {
            out.push(ch);
        }
        return consumed;
    }
    let ch = text[1..].chars().next().unwrap_or('\u{FFFD}');
    out.push(ch);
    1 + ch.len_utf8()
}

/// 解析 `[ ... ]` 里的内容。支持存在、`=`、`~=`、`|=`、`^=`、`$=`、`*=` 与 `i` 标志。
fn parse_attr_test(text: &str) -> Option<AttrTest> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let mut name = String::new();
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += css_unescape_into(&mut name, &text[i..]);
            continue;
        }
        let ch = text[i..].chars().next()?;
        if ch.is_ascii_whitespace() || matches!(ch, '=' | '~' | '^' | '$' | '*') {
            break;
        }
        // `ns|attr` 的命名空间前缀没有实际用途：只留本地名。
        if ch == '|' && bytes.get(i + 1) != Some(&b'=') {
            name.clear();
            i += 1;
            continue;
        }
        if ch == '|' {
            break;
        }
        name.push(ch);
        i += ch.len_utf8();
    }
    if name.is_empty() {
        return None;
    }
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return Some(AttrTest {
            name: name.to_ascii_lowercase(),
            op: AttrOp::Exists,
            value: String::new(),
            insensitive: false,
        });
    }
    let op = match bytes[i] {
        b'=' => AttrOp::Equals,
        b'~' if bytes.get(i + 1) == Some(&b'=') => AttrOp::Includes,
        b'|' if bytes.get(i + 1) == Some(&b'=') => AttrOp::Dash,
        b'^' if bytes.get(i + 1) == Some(&b'=') => AttrOp::Prefix,
        b'$' if bytes.get(i + 1) == Some(&b'=') => AttrOp::Suffix,
        b'*' if bytes.get(i + 1) == Some(&b'=') => AttrOp::Substring,
        _ => return None,
    };
    i += if op == AttrOp::Equals { 1 } else { 2 };
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let (value, consumed) = read_attr_value(&text[i..])?;
    i += consumed;
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let mut insensitive = false;
    if i < bytes.len() {
        match bytes[i] {
            b'i' | b'I' => insensitive = true,
            b's' | b'S' => {}
            _ => return None,
        }
        i += 1;
        while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() {
            return None;
        }
    }
    Some(AttrTest {
        name: name.to_ascii_lowercase(),
        op,
        value,
        insensitive,
    })
}

/// 读属性值：引号字符串（含转义）或到空白为止的裸值。
fn read_attr_value(text: &str) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    if let Some(&quote) = bytes.first().filter(|b| matches!(b, b'"' | b'\'')) {
        let mut i = 1usize;
        while i < bytes.len() {
            if bytes[i] == quote {
                return Some((out, i + 1));
            }
            if bytes[i] == b'\\' {
                i += css_unescape_into(&mut out, &text[i..]);
                continue;
            }
            let ch = text[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
        }
        return None;
    }
    let mut i = 0usize;
    while i < bytes.len() && !(bytes[i] as char).is_ascii_whitespace() {
        if bytes[i] == b'\\' {
            i += css_unescape_into(&mut out, &text[i..]);
            continue;
        }
        let ch = text[i..].chars().next()?;
        out.push(ch);
        i += ch.len_utf8();
    }
    if out.is_empty() {
        None
    } else {
        Some((out, i))
    }
}

fn starts_with_ci(hay: &str, needle: &str) -> bool {
    hay.len() >= needle.len()
        && hay.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle.as_bytes())
}

fn ends_with_ci(hay: &str, needle: &str) -> bool {
    hay.len() >= needle.len()
        && hay.as_bytes()[hay.len() - needle.len()..].eq_ignore_ascii_case(needle.as_bytes())
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    (0..=hay.len() - needle.len())
        .any(|at| hay.as_bytes()[at..at + needle.len()].eq_ignore_ascii_case(needle.as_bytes()))
}

impl AttrTest {
    fn matches(&self, value: &str) -> bool {
        let expected = self.value.as_str();
        let equal = |a: &str, b: &str| {
            if self.insensitive {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        };
        match self.op {
            AttrOp::Exists => true,
            AttrOp::Equals => equal(value, expected),
            AttrOp::Includes => {
                // 规范要求匹配值本身不含空白；`~=` 空值不命中任何东西。
                !expected.is_empty()
                    && !expected.chars().any(char::is_whitespace)
                    && value.split_whitespace().any(|part| equal(part, expected))
            }
            AttrOp::Dash => {
                equal(value, expected)
                    || (value.len() > expected.len()
                        && value.is_char_boundary(expected.len())
                        && equal(&value[..expected.len()], expected)
                        && value.as_bytes()[expected.len()] == b'-')
            }
            AttrOp::Prefix => {
                !expected.is_empty()
                    && if self.insensitive {
                        starts_with_ci(value, expected)
                    } else {
                        value.starts_with(expected)
                    }
            }
            AttrOp::Suffix => {
                !expected.is_empty()
                    && if self.insensitive {
                        ends_with_ci(value, expected)
                    } else {
                        value.ends_with(expected)
                    }
            }
            AttrOp::Substring => {
                !expected.is_empty()
                    && if self.insensitive {
                        contains_ci(value, expected)
                    } else {
                        value.contains(expected)
                    }
            }
        }
    }
}

/// 取属性值。`[epub\:type]` 的转义名是 `epub:type`，而 XML 解析把前缀拆掉了，
/// 所以带前缀的名字回退到本地名比较。
fn attr_value<'a>(key: &'a ElemKey, name: &str) -> Option<&'a str> {
    if let Some((_, value)) = key.attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(value.as_str());
    }
    let (_, local) = name.rsplit_once(':')?;
    key.attrs
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(local))
        .map(|(_, value)| value.as_str())
}

fn specificity(selector: &[Compound]) -> Spec {
    let mut ids = 0;
    let mut classes = 0;
    let mut tags = 0;
    for compound in selector {
        if compound.id.is_some() {
            ids += 1;
        }
        // 伪类、属性选择器都按 class 级计数。
        classes += compound.classes.len() as u16
            + compound.pseudos.len() as u16
            + compound.attrs.len() as u16;
        if compound.tag.is_some() {
            tags += 1;
        }
    }
    (ids, classes, tags)
}

fn compound_matches(key: &ElemKey, compound: &Compound) -> bool {
    if let Some(tag) = &compound.tag {
        if !key.tag_name.eq_ignore_ascii_case(tag) {
            return false;
        }
    }
    if let Some(id) = &compound.id {
        if key.id.as_deref() != Some(id.as_str()) {
            return false;
        }
    }
    for class in &compound.classes {
        if !key.classes.iter().any(|c| c == class) {
            return false;
        }
    }
    for test in &compound.attrs {
        let Some(value) = attr_value(key, &test.name) else {
            return false;
        };
        if !test.matches(value) {
            return false;
        }
    }
    true
}

fn pseudo_matches(doc: &StyledDoc, aux: &MatchAux, node: NodeId, compound: &Compound) -> bool {
    if compound.pseudos.is_empty() {
        return true;
    }
    // 根元素没有父节点，不属于任何兄弟序列，`*:first-child` 不命中它。
    let Some(parent) = doc.node(node).parent else {
        return false;
    };
    for pseudo in &compound.pseudos {
        let hit = match pseudo {
            PseudoClass::FirstChild => aux.first_element[parent as usize],
            PseudoClass::LastChild => aux.last_element[parent as usize],
        };
        if hit != Some(node) {
            return false;
        }
    }
    true
}

/// 从右往左匹配。遇到 `>` 固定检查直接父节点，遇到 `+` 只检查紧邻的元素兄弟；
/// 后代组合器逐个向上找，找不到就返回 false。
fn matches(
    doc: &StyledDoc,
    keys: &[ElemKey],
    aux: &MatchAux,
    node: NodeId,
    selector: &[Compound],
    budget: &mut usize,
) -> bool {
    let Some(last) = selector.len().checked_sub(1) else {
        return false;
    };
    match_at(doc, keys, aux, node, selector, last, budget)
}

fn match_at(
    doc: &StyledDoc,
    keys: &[ElemKey],
    aux: &MatchAux,
    node: NodeId,
    selector: &[Compound],
    index: usize,
    budget: &mut usize,
) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    let compound = &selector[index];
    if !keys[node as usize].is_element
        || !compound_matches(&keys[node as usize], compound)
        || !pseudo_matches(doc, aux, node, compound)
    {
        return false;
    }
    if index == 0 {
        return true;
    }
    match compound.combinator {
        Combinator::Descendant => {
            let mut ancestor = doc.node(node).parent;
            while let Some(candidate) = ancestor {
                if match_at(doc, keys, aux, candidate, selector, index - 1, budget) {
                    return true;
                }
                ancestor = doc.node(candidate).parent;
            }
            false
        }
        Combinator::Child => doc
            .node(node)
            .parent
            .is_some_and(|parent| match_at(doc, keys, aux, parent, selector, index - 1, budget)),
        Combinator::AdjacentSibling => aux.prev_element[node as usize]
            .is_some_and(|sibling| match_at(doc, keys, aux, sibling, selector, index - 1, budget)),
        Combinator::GeneralSibling => {
            let mut sibling = aux.prev_element[node as usize];
            while let Some(candidate) = sibling {
                if match_at(doc, keys, aux, candidate, selector, index - 1, budget) {
                    return true;
                }
                sibling = aux.prev_element[candidate as usize];
            }
            false
        }
    }
}

fn truncate_utf8(value: &str, max: usize) -> &str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn skip_space_and_comments(bytes: &[u8], mut i: usize) -> usize {
    loop {
        while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        return i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ebook_ir::{ComputedStyle, Node, NodeKind, Span};

    fn styled(css: &str) -> ebook_ir::ComputedStyle {
        let mut style = ebook_ir::ComputedStyle::default();
        let parent_color = style.color;
        for decl in parse_decls(css) {
            apply(&mut style, &decl, 16.0, 400, parent_color);
        }
        style
    }

    fn element(
        doc: &mut StyledDoc,
        keys: &mut Vec<ElemKey>,
        parent: Option<NodeId>,
        tag: Tag,
        name: &str,
        id: Option<&str>,
        classes: &[&str],
    ) -> NodeId {
        let node = doc.push_node(Node {
            kind: NodeKind::Element(tag),
            style: ComputedStyle::default(),
            parent,
            first_child: 0,
            child_len: 0,
            src: None,
            link: None,
        });
        keys.push(ElemKey {
            tag,
            tag_name: name.to_string(),
            id: id.map(|s| s.to_string()),
            classes: classes.iter().map(|s| s.to_string()).collect(),
            attrs: Vec::new(),
            inline: Vec::new(),
            is_element: true,
        });
        node
    }

    fn text_node(
        doc: &mut StyledDoc,
        keys: &mut Vec<ElemKey>,
        parent: NodeId,
        text: &str,
    ) -> NodeId {
        let start = doc.text.len() as u32;
        doc.text.push_str(text);
        let node = doc.push_node(Node {
            kind: NodeKind::Text(Span::new(start, text.len() as u32)),
            style: ComputedStyle::default(),
            parent: Some(parent),
            first_child: 0,
            child_len: 0,
            src: None,
            link: None,
        });
        keys.push(ElemKey {
            is_element: false,
            ..ElemKey::default()
        });
        node
    }

    fn cascade_css(doc: &mut StyledDoc, keys: &[ElemKey], css: &str) {
        let mut sheet = StyleSheet::default();
        sheet.parse_into(css);
        cascade(doc, keys, &sheet);
    }

    fn simple_paragraph() -> (StyledDoc, Vec<ElemKey>, NodeId) {
        let mut doc = StyledDoc::default();
        let mut keys = Vec::new();
        let body = element(&mut doc, &mut keys, None, Tag::Body, "body", None, &[]);
        doc.root = body;
        let p = element(&mut doc, &mut keys, Some(body), Tag::P, "p", None, &[]);
        doc.set_kids(body, &[p]);
        (doc, keys, p)
    }

    #[test]
    fn text_rules_default_off() {
        let style = styled("");
        assert!(!style.text.rules.spacing_trim);
        assert!(!style.text.rules.hanging_punctuation);
        assert!(!style.text.rules.autospacing);
    }

    #[test]
    fn text_rules_parse_css_text_4() {
        let style = styled(
            "text-spacing-trim: normal; hanging-punctuation: allow-end; text-autospace: normal;",
        );
        assert!(style.text.rules.spacing_trim);
        assert!(style.text.rules.hanging_punctuation);
        assert!(style.text.rules.autospacing);

        let off =
            styled("text-spacing-trim: none; hanging-punctuation: none; text-autospace: none;");
        assert!(!off.text.rules.spacing_trim);
        assert!(!off.text.rules.hanging_punctuation);
        assert!(!off.text.rules.autospacing);
    }

    #[test]
    fn text_rules_reject_unimplemented_values() {
        // 我们只实现可判定的部分：其它取值要报 UnsupportedCss，而不是静默变开关。
        for (name, value) in [
            ("text-spacing-trim", "space-all"),
            ("text-spacing-trim", "trim-start"),
            ("hanging-punctuation", "force-end first"),
            ("text-autospace", "ideograph-alpha"),
        ] {
            let decl = Decl {
                name: name.to_string(),
                value: value.to_string(),
                important: false,
            };
            assert!(!decl_supported(&decl), "{name}: {value} 应当被报为不支持");
        }
    }

    #[test]
    fn hanging_punctuation_accepts_lists() {
        let style = styled("hanging-punctuation: first allow-end;");
        assert!(style.text.rules.hanging_punctuation);
    }

    fn assert_px(value: &str, expected: f32) {
        match parse_len(value) {
            Some(Len::Px(px)) => assert!(
                (px - expected).abs() < 0.01,
                "{value}: expected {expected}, got {px}"
            ),
            other => panic!("{value}: expected px, got {other:?}"),
        }
    }

    #[test]
    fn absolute_units_convert_to_px() {
        assert_px("12pt", 16.0);
        assert_px("1pc", 16.0);
        assert_px("1in", 96.0);
        assert_px("2.54cm", 96.0);
        assert_px("25.4mm", 96.0);
        // 原有相对单位保持不变。
        assert_eq!(parse_len("1.5em"), Some(Len::Em(1.5)));
        assert_eq!(parse_len("2rem"), Some(Len::Rem(2.0)));
        assert_eq!(parse_len("50%"), Some(Len::Pct(50.0)));
    }

    #[test]
    fn absolute_units_reach_computed_style() {
        let (mut doc, keys, p) = simple_paragraph();
        cascade_css(
            &mut doc,
            &keys,
            "p { font-size: 12pt; margin-left: 1in; margin-right: 1pc; \
             padding-top: 2.54cm; text-indent: 25.4mm; }",
        );
        let style = doc.node(p).style;
        assert!((style.font.size - 16.0).abs() < 0.01, "12pt = 16px");
        assert!(matches!(style.margin.left, Len::Px(v) if (v - 96.0).abs() < 0.01));
        assert!(matches!(style.margin.right, Len::Px(v) if (v - 16.0).abs() < 0.01));
        assert!(matches!(style.padding.top, Len::Px(v) if (v - 96.0).abs() < 0.01));
        assert!(matches!(style.text.indent, Len::Px(v) if (v - 96.0).abs() < 0.01));
    }

    #[test]
    fn parses_hex_colors_with_alpha() {
        assert_eq!(parse_color("#f008"), Some(Rgba8::new(255, 0, 0, 0x88)));
        assert_eq!(parse_color("#ff000080"), Some(Rgba8::new(255, 0, 0, 0x80)));
        assert_eq!(parse_color("#abc"), Some(Rgba8::new(0xaa, 0xbb, 0xcc, 255)));
        assert_eq!(parse_color("#0f08"), Some(Rgba8::new(0, 255, 0, 0x88)));
    }

    #[test]
    fn parses_rgb_and_rgba_components() {
        assert_eq!(
            parse_color("rgb(100%,0%,0%)"),
            Some(Rgba8::new(255, 0, 0, 255))
        );
        assert_eq!(
            parse_color("rgba(0,0,0,.5)"),
            Some(Rgba8::new(0, 0, 0, 128))
        );
        assert_eq!(
            parse_color("rgba(0, 0, 0, 50%)"),
            Some(Rgba8::new(0, 0, 0, 128))
        );
        // 越界分量钳制到合法范围。
        assert_eq!(
            parse_color("rgb(300,-20,128)"),
            Some(Rgba8::new(255, 0, 128, 255))
        );
        assert_eq!(
            parse_color("rgba(0,0,0,150%)"),
            Some(Rgba8::new(0, 0, 0, 255))
        );
        assert_eq!(parse_color("rgba(0,0,0,0)"), Some(Rgba8::TRANSPARENT));
    }

    #[test]
    fn parses_hsl_and_hsla_components() {
        assert_eq!(
            parse_color("hsl(0, 100%, 50%)"),
            Some(Rgba8::new(255, 0, 0, 255))
        );
        assert_eq!(
            parse_color("hsl(120deg, 100%, 25%)"),
            Some(Rgba8::new(0, 128, 0, 255))
        );
        assert_eq!(
            parse_color("hsl(240, 100%, 50%)"),
            Some(Rgba8::new(0, 0, 255, 255))
        );
        // 色相绕回：负角与超过一圈等价。
        assert_eq!(
            parse_color("hsl(-120, 100%, 50%)"),
            Some(Rgba8::new(0, 0, 255, 255))
        );
        assert_eq!(
            parse_color("hsl(360, 100%, 50%)"),
            Some(Rgba8::new(255, 0, 0, 255))
        );
        // 无饱和度：亮度决定灰阶。
        assert_eq!(
            parse_color("hsl(0, 0%, 100%)"),
            Some(Rgba8::new(255, 255, 255, 255))
        );
        assert_eq!(
            parse_color("hsla(0, 100%, 50%, 25%)"),
            Some(Rgba8::new(255, 0, 0, 64))
        );
        assert_eq!(
            parse_color("hsl(0, 100%, 50%, .5)"),
            Some(Rgba8::new(255, 0, 0, 128))
        );
        // 饱和度/亮度必须是百分比，分量个数不对就整条丢。
        assert_eq!(parse_color("hsl(0, 100, 50)"), None);
        assert_eq!(parse_color("hsl(0, 100%)"), None);
    }

    #[test]
    fn current_color_resolves_in_cascade() {
        let mut doc = StyledDoc::default();
        let mut keys = Vec::new();
        let body = element(&mut doc, &mut keys, None, Tag::Body, "body", None, &[]);
        doc.root = body;
        let div = element(&mut doc, &mut keys, Some(body), Tag::Div, "div", None, &[]);
        let bg = element(&mut doc, &mut keys, Some(div), Tag::P, "p", Some("bg"), &[]);
        let inherit = element(
            &mut doc,
            &mut keys,
            Some(div),
            Tag::P,
            "p",
            Some("inherit"),
            &[],
        );
        doc.set_kids(div, &[bg, inherit]);
        doc.set_kids(body, &[div]);

        cascade_css(
            &mut doc,
            &keys,
            "div { color: #aa0000; } \
             p { color: #00bb00; background-color: currentColor; } \
             #inherit { color: currentColor; }",
        );

        // background-color 取元素此刻的 color，而不是父元素的颜色。
        assert_eq!(doc.node(bg).style.color, Rgba8::new(0x00, 0xbb, 0x00, 255));
        assert_eq!(
            doc.node(bg).style.background,
            Some(Rgba8::new(0x00, 0xbb, 0x00, 255))
        );
        // color: currentColor 等价于继承父元素颜色。
        assert_eq!(
            doc.node(inherit).style.color,
            Rgba8::new(0xaa, 0x00, 0x00, 255)
        );
        // 背景在 color 被后面的声明改写之前已经取值，固定顺序语义。
        assert_eq!(
            doc.node(inherit).style.background,
            Some(Rgba8::new(0x00, 0xbb, 0x00, 255))
        );
    }

    struct SelectorDoc {
        doc: StyledDoc,
        keys: Vec<ElemKey>,
        outer: NodeId,
        direct: NodeId,
        grand: NodeId,
        head: NodeId,
        adjacent: NodeId,
        not_adjacent: NodeId,
        list: NodeId,
        first: NodeId,
        middle: NodeId,
        last: NodeId,
    }

    fn selector_document() -> SelectorDoc {
        let mut doc = StyledDoc::default();
        let mut keys = Vec::new();
        let body = element(&mut doc, &mut keys, None, Tag::Body, "body", None, &[]);
        doc.root = body;
        let outer = element(
            &mut doc,
            &mut keys,
            Some(body),
            Tag::Div,
            "div",
            Some("outer"),
            &[],
        );
        let direct = element(
            &mut doc,
            &mut keys,
            Some(outer),
            Tag::P,
            "p",
            Some("direct"),
            &[],
        );
        let section = element(
            &mut doc,
            &mut keys,
            Some(outer),
            Tag::Section,
            "section",
            None,
            &[],
        );
        let grand = element(
            &mut doc,
            &mut keys,
            Some(section),
            Tag::P,
            "p",
            Some("grand"),
            &[],
        );
        let head = element(
            &mut doc,
            &mut keys,
            Some(body),
            Tag::H1,
            "h1",
            Some("head"),
            &[],
        );
        let adjacent = element(
            &mut doc,
            &mut keys,
            Some(body),
            Tag::P,
            "p",
            Some("adjacent"),
            &[],
        );
        let not_adjacent = element(
            &mut doc,
            &mut keys,
            Some(body),
            Tag::P,
            "p",
            Some("not-adjacent"),
            &[],
        );
        let list = element(
            &mut doc,
            &mut keys,
            Some(body),
            Tag::Div,
            "div",
            Some("list"),
            &[],
        );
        let lead = text_node(&mut doc, &mut keys, list, "x");
        let first = element(
            &mut doc,
            &mut keys,
            Some(list),
            Tag::P,
            "p",
            Some("first"),
            &[],
        );
        let span = element(
            &mut doc,
            &mut keys,
            Some(list),
            Tag::Span,
            "span",
            None,
            &[],
        );
        let middle = element(
            &mut doc,
            &mut keys,
            Some(list),
            Tag::P,
            "p",
            Some("middle"),
            &["note"],
        );
        let last = element(
            &mut doc,
            &mut keys,
            Some(list),
            Tag::P,
            "p",
            Some("last"),
            &[],
        );
        doc.set_kids(section, &[grand]);
        doc.set_kids(outer, &[direct, section]);
        doc.set_kids(list, &[lead, first, span, middle, last]);
        doc.set_kids(body, &[outer, head, adjacent, not_adjacent, list]);
        SelectorDoc {
            doc,
            keys,
            outer,
            direct,
            grand,
            head,
            adjacent,
            not_adjacent,
            list,
            first,
            middle,
            last,
        }
    }

    #[test]
    fn child_combinator_matches_only_direct_children() {
        let mut fixture = selector_document();
        cascade_css(&mut fixture.doc, &fixture.keys, "div > p { color: red; }");
        assert_eq!(
            fixture.doc.node(fixture.direct).style.color,
            Rgba8::new(255, 0, 0, 255)
        );
        assert_eq!(
            fixture.doc.node(fixture.grand).style.color,
            Rgba8::BLACK,
            "孙代不应命中 div > p"
        );
        assert_eq!(fixture.doc.node(fixture.outer).style.color, Rgba8::BLACK);
    }

    #[test]
    fn adjacent_sibling_matches_only_the_next_element() {
        let mut fixture = selector_document();
        cascade_css(&mut fixture.doc, &fixture.keys, "h1 + p { color: green; }");
        assert_eq!(
            fixture.doc.node(fixture.adjacent).style.color,
            Rgba8::new(0, 128, 0, 255)
        );
        assert_eq!(
            fixture.doc.node(fixture.not_adjacent).style.color,
            Rgba8::BLACK
        );
        assert_eq!(fixture.doc.node(fixture.head).style.color, Rgba8::BLACK);
    }

    #[test]
    fn first_and_last_child_skip_text_nodes() {
        let mut fixture = selector_document();
        cascade_css(
            &mut fixture.doc,
            &fixture.keys,
            "#list > p:first-child { color: blue; } \
             #list > p:last-child { color: maroon; }",
        );
        assert_eq!(
            fixture.doc.node(fixture.first).style.color,
            Rgba8::new(0, 0, 255, 255)
        );
        assert_eq!(
            fixture.doc.node(fixture.last).style.color,
            Rgba8::new(128, 0, 0, 255)
        );
        assert_eq!(fixture.doc.node(fixture.middle).style.color, Rgba8::BLACK);
        assert_eq!(fixture.doc.node(fixture.list).style.color, Rgba8::BLACK);
    }

    #[test]
    fn specificity_orders_pseudo_classes_like_classes() {
        let mut fixture = selector_document();
        // 高 specificity 写在前面，仍然需要压过后面的低 specificity 规则。
        cascade_css(
            &mut fixture.doc,
            &fixture.keys,
            "#middle { color: navy; } \
             p.note { color: blue; } \
             p:first-child { color: red; } \
             p { color: green; }",
        );
        assert_eq!(
            fixture.doc.node(fixture.middle).style.color,
            Rgba8::new(0, 0, 128, 255),
            "id 选择器应压过 class / 伪类"
        );
        assert_eq!(
            fixture.doc.node(fixture.first).style.color,
            Rgba8::new(255, 0, 0, 255),
            ":first-child 与 class 同级，压过 tag"
        );
        assert_eq!(
            fixture.doc.node(fixture.last).style.color,
            Rgba8::new(0, 128, 0, 255)
        );
    }

    #[test]
    fn links_get_ua_blue_and_underline_but_css_can_override() {
        let (mut doc, mut keys, p) = simple_paragraph();
        let a = element(&mut doc, &mut keys, Some(p), Tag::A, "a", None, &[]);
        let t = text_node(&mut doc, &mut keys, a, "link");
        doc.set_kids(p, &[a]);
        doc.set_kids(a, &[t]);

        cascade_css(&mut doc, &keys, "");
        assert_eq!(
            doc.node(a).style.color,
            Rgba8::new(26, 95, 180, 255),
            "没有样式表时链接应当是蓝色"
        );
        assert!(doc.node(a).style.text.underline, "链接默认带下划线");

        cascade_css(
            &mut doc,
            &keys,
            "a { color: #123456; text-decoration: none; }",
        );
        assert_eq!(doc.node(a).style.color, Rgba8::new(0x12, 0x34, 0x56, 255));
        assert!(!doc.node(a).style.text.underline);
    }

    fn with_attrs(attrs: &[(&str, &str)]) -> (StyledDoc, Vec<ElemKey>, NodeId) {
        let (doc, mut keys, p) = simple_paragraph();
        keys[p as usize].attrs = attrs
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
            .collect();
        (doc, keys, p)
    }

    #[test]
    fn attribute_selectors_match_all_operators() {
        let cases: [(&[(&str, &str)], &str); 7] = [
            (&[("data-note", "x")], "[data-note]"),
            (&[("data-note", "x")], "[data-note=\"x\"]"),
            (&[("class", "lead big")], "[class~=\"big\"]"),
            (&[("lang", "en-US")], "[lang|=\"en\"]"),
            (&[("href", "https://a/b.pdf")], "[href^=\"https\"]"),
            (&[("href", "https://a/b.pdf")], "[href$=\".pdf\"]"),
            (&[("href", "https://a/b.pdf")], "[href*=\"a/b\"]"),
        ];
        for (attrs, selector) in cases {
            let (mut doc, keys, p) = with_attrs(attrs);
            cascade_css(
                &mut doc,
                &keys,
                &format!("p{selector} {{ color: #ff0000; }}"),
            );
            assert_eq!(
                doc.node(p).style.color,
                Rgba8::new(255, 0, 0, 255),
                "{selector} with {attrs:?}"
            );
        }
    }

    #[test]
    fn attribute_selectors_respect_values_and_flags() {
        let (mut doc, keys, p) = with_attrs(&[("data-x", "abc")]);
        cascade_css(&mut doc, &keys, "p[data-x=\"ABC\" i] { color: #00ff00; }");
        assert_eq!(doc.node(p).style.color, Rgba8::new(0, 255, 0, 255));

        let (mut doc, keys, p) = with_attrs(&[("data-x", "abc")]);
        cascade_css(&mut doc, &keys, "p[data-x=\"ABC\"] { color: #00ff00; }");
        assert_eq!(doc.node(p).style.color, Rgba8::BLACK, "默认大小写敏感");

        // `^=` / `*=` 的空值不命中任何东西。
        let (mut doc, keys, p) = with_attrs(&[("data-x", "abc")]);
        cascade_css(&mut doc, &keys, "p[data-x^=\"\"] { color: #00ff00; }");
        cascade_css(&mut doc, &keys, "p[data-x*=\"\"] { color: #0000ff; }");
        assert_eq!(doc.node(p).style.color, Rgba8::BLACK);

        let (mut doc, keys, p) = with_attrs(&[("lang", "en-US")]);
        cascade_css(&mut doc, &keys, "p[lang=\"en\"] { color: #00ff00; }");
        assert_eq!(doc.node(p).style.color, Rgba8::BLACK, "= 不是前缀匹配");
    }

    #[test]
    fn commas_inside_attribute_values_do_not_split_rules() {
        let (mut doc, keys, p) = with_attrs(&[("title", "a,b")]);
        cascade_css(
            &mut doc,
            &keys,
            "p[title=\"a,b\"], p[title=\"no\"] { color: #ff0000; }",
        );
        assert_eq!(doc.node(p).style.color, Rgba8::new(255, 0, 0, 255));
    }

    #[test]
    fn universal_selector_can_carry_attributes_and_classes() {
        let selector = parse_selector("*[data-x]").expect("universal attr");
        assert_eq!(selector.len(), 1);
        assert_eq!(selector[0].attrs.len(), 1);
        let selector = parse_selector("*.lead[data-x]").expect("universal class attr");
        assert_eq!(selector[0].classes, vec!["lead"]);
        assert_eq!(selector[0].attrs.len(), 1);
        let selector = parse_selector("p[data-x][data-y]").expect("two attributes");
        assert_eq!(selector[0].attrs.len(), 2);
    }

    #[test]
    fn namespaced_attribute_selectors_match_the_local_name() {
        let (mut doc, keys, p) = with_attrs(&[("type", "cover")]);
        cascade_css(
            &mut doc,
            &keys,
            "p[epub\\:type=\"cover\"] { color: #ff0000; }",
        );
        assert_eq!(doc.node(p).style.color, Rgba8::new(255, 0, 0, 255));
    }

    #[test]
    fn import_statements_are_collected_but_not_rules() {
        let mut sheet = StyleSheet::default();
        sheet.parse_into(
            "@import url(\"a.css\") screen; @import 'b.css' print; @import \"c.css\";             p { color: #ff0000; }",
        );
        assert_eq!(sheet.take_imports(), vec!["a.css", "c.css"]);
        assert_eq!(sheet.rules.len(), 1);
        assert!(sheet.take_imports().is_empty());
    }

    #[test]
    fn unsupported_selectors_are_rejected_whole() {
        for selector in [":not(.x)", "p::before", "div > > p", "> p", "h1 +"] {
            assert!(
                parse_selector(selector).is_none(),
                "{selector} 应当整条丢弃"
            );
        }
        // 支持的形式仍能解析出组合器。
        let selector = parse_selector("div > p + h1 ~ span").expect("selector");
        assert_eq!(selector.len(), 4);
        assert_eq!(selector[1].combinator, Combinator::Child);
        assert_eq!(selector[2].combinator, Combinator::AdjacentSibling);
        assert_eq!(selector[3].combinator, Combinator::GeneralSibling);
    }

    /// `~` 命中前面**任意**一个元素兄弟；文本节点不参与计数。
    #[test]
    fn general_sibling_matches_every_preceding_element() {
        let (mut doc, mut keys, first) = simple_paragraph();
        let body = doc.node(first).parent.expect("body");
        let second = element(&mut doc, &mut keys, Some(body), Tag::P, "p", None, &[]);
        let third = element(&mut doc, &mut keys, Some(body), Tag::P, "p", None, &[]);
        let text = text_node(&mut doc, &mut keys, first, "x");
        doc.set_kids(first, &[text]);
        doc.set_kids(body, &[first, second, third]);

        cascade_css(&mut doc, &keys, "p ~ p { color: #ff0000; }");
        assert_eq!(
            doc.node(first).style.color,
            Rgba8::BLACK,
            "第一个 p 前面没有元素兄弟"
        );
        assert_eq!(doc.node(second).style.color, Rgba8::new(255, 0, 0, 255));
        assert_eq!(doc.node(third).style.color, Rgba8::new(255, 0, 0, 255));
    }
}
