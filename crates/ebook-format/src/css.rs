use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ebook_ir::{
    Break, Display, FontFamilies, FontStyle, Len, LineHeight, ListStyle, NodeId, Rgba8, StyledDoc,
    Tag, TextAlign, TextTransform, VerticalAlign, WhiteSpace, WordBreak, WritingMode,
};

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
}

/// 结构性伪类。只实现只看元素兄弟的两个：文本节点不参与计数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PseudoClass {
    FirstChild,
    LastChild,
}

#[derive(Clone, Debug, Default)]
struct Compound {
    /// 与本复合选择器前面的那个复合选择器如何连接；第一个复合选择器忽略此字段。
    combinator: Combinator,
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
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
    /// 无法落到 IR 的属性名，按首次出现顺序去重。
    unsupported: Vec<String>,
}

impl StyleSheet {
    pub fn parse_into(&mut self, css: &str) {
        self.parse_level(css);
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.font_faces.is_empty()
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
    fn parse_level(&mut self, css: &str) {
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
                            self.parse_level(body);
                        }
                    }
                    "supports" | "document" | "layer" => self.parse_level(body),
                    "font-face" => {
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
            let decls = parse_decls(body);
            if decls.is_empty() {
                continue;
            }
            for decl in &decls {
                if !decl_supported(decl) {
                    self.note_unsupported(&decl.name);
                }
            }
            for selector_text in selector_text.split(',') {
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

/// 把一个规则块的值拆成声明。`!important` 单独记，不混在值里。
pub fn parse_decls(body: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    for part in body.split(';') {
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

/// 属性名是否认识。认识但 IR 没有对应字段的（`text-decoration`、`border-radius`）
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

fn media_is_screen(prelude: &str) -> bool {
    let lower = prelude.to_ascii_lowercase();
    if lower.contains("print") && !lower.contains("screen") {
        return false;
    }
    true
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
    let mut matched: Vec<(Spec, usize)> = Vec::new();
    for id in order {
        let parent_style = doc
            .node(id)
            .parent
            .map(|p| doc.node(p).style)
            .unwrap_or_default();
        let mut style = inherit(&parent_style);
        ua_apply(&mut style, keys[id as usize].tag);

        matched.clear();
        for (i, rule) in sheet.rules.iter().enumerate() {
            if matches(doc, keys, id, &rule.selector) {
                matched.push((rule.spec, i));
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
        Tag::Em => style.display = Display::Inline,
        Tag::Span | Tag::A | Tag::Code | Tag::Ruby | Tag::Rt => style.display = Display::Inline,
        Tag::Br => style.display = Display::Inline,
        Tag::Img => style.display = Display::InlineBlock,
        Tag::Li => {
            style.display = Display::ListItem;
            style.margin.left = Len::Em(1.5);
        }
        Tag::Ul | Tag::Ol => {
            style.display = Display::Block;
            style.padding.left = Len::Em(1.5);
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
        "list-style-type" => {
            if value != "none" {
                style.list = Some(ListStyle {
                    ordered: matches!(
                        value,
                        "decimal" | "lower-alpha" | "upper-alpha" | "lower-roman" | "upper-roman"
                    ),
                    start: 1,
                });
            }
        }
        "list-style" => {
            let mut ordered = false;
            for token in value.split_whitespace() {
                if matches!(
                    token,
                    "decimal" | "lower-alpha" | "upper-alpha" | "lower-roman" | "upper-roman"
                ) {
                    ordered = true;
                }
            }
            style.list = Some(ListStyle { ordered, start: 1 });
        }
        // IR 没有装饰线与圆角字段：明确跳过，等 IR 提供后再接。
        "text-decoration" | "text-decoration-line" | "border-radius" => {}
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
            // `~` / 属性 / 函数式伪类继续整条丢弃，不降级成通配符。
            b'~' | b'[' | b']' | b'(' | b')' => return None,
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
        while i < bytes.len() {
            let byte = bytes[i];
            if (byte as char).is_ascii_whitespace()
                || matches!(byte, b'>' | b'+' | b'~' | b'[' | b']' | b'(' | b')')
            {
                break;
            }
            i += 1;
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
        if !after.starts_with(':') {
            return None;
        }
        rest = after;
    }
    let mut has_tag = false;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('#') {
            let end = after.find(['.', '#', ':']).unwrap_or(after.len());
            if !valid_compound_name(&after[..end]) {
                return None;
            }
            compound.id = Some(after[..end].to_string());
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '#', ':']).unwrap_or(after.len());
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
            let end = after.find(['.', '#', ':']).unwrap_or(after.len());
            let pseudo = match after[..end].to_ascii_lowercase().as_str() {
                "first-child" => PseudoClass::FirstChild,
                "last-child" => PseudoClass::LastChild,
                _ => return None,
            };
            compound.pseudos.push(pseudo);
            rest = &after[end..];
        } else {
            if has_tag {
                return None;
            }
            let end = rest.find(['.', '#', ':']).unwrap_or(rest.len());
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

fn specificity(selector: &[Compound]) -> Spec {
    let mut ids = 0;
    let mut classes = 0;
    let mut tags = 0;
    for compound in selector {
        if compound.id.is_some() {
            ids += 1;
        }
        // 伪类按 class 级计数。
        classes += compound.classes.len() as u16 + compound.pseudos.len() as u16;
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
    true
}

/// 只看元素兄弟的第一个/最后一个孩子；文本、换行等非元素节点跳过。
fn element_child(doc: &StyledDoc, keys: &[ElemKey], parent: NodeId, last: bool) -> Option<NodeId> {
    let kids = doc.kids(parent);
    if last {
        kids.iter()
            .rev()
            .copied()
            .find(|&kid| keys[kid as usize].is_element)
    } else {
        kids.iter()
            .copied()
            .find(|&kid| keys[kid as usize].is_element)
    }
}

fn previous_element_sibling(doc: &StyledDoc, keys: &[ElemKey], node: NodeId) -> Option<NodeId> {
    let parent = doc.node(node).parent?;
    let kids = doc.kids(parent);
    let index = kids.iter().position(|&kid| kid == node)?;
    kids[..index]
        .iter()
        .rev()
        .copied()
        .find(|&kid| keys[kid as usize].is_element)
}

fn pseudo_matches(doc: &StyledDoc, keys: &[ElemKey], node: NodeId, compound: &Compound) -> bool {
    if compound.pseudos.is_empty() {
        return true;
    }
    // 根元素没有父节点，不属于任何兄弟序列，`*:first-child` 不命中它。
    let Some(parent) = doc.node(node).parent else {
        return false;
    };
    for pseudo in &compound.pseudos {
        let hit = match pseudo {
            PseudoClass::FirstChild => element_child(doc, keys, parent, false),
            PseudoClass::LastChild => element_child(doc, keys, parent, true),
        };
        if hit != Some(node) {
            return false;
        }
    }
    true
}

/// 从右往左匹配。遇到 `>` 固定检查直接父节点，遇到 `+` 只检查紧邻的元素兄弟；
/// 后代组合器逐个向上找，找不到就返回 false。
fn matches(doc: &StyledDoc, keys: &[ElemKey], node: NodeId, selector: &[Compound]) -> bool {
    let Some(last) = selector.len().checked_sub(1) else {
        return false;
    };
    match_at(doc, keys, node, selector, last)
}

fn match_at(
    doc: &StyledDoc,
    keys: &[ElemKey],
    node: NodeId,
    selector: &[Compound],
    index: usize,
) -> bool {
    let compound = &selector[index];
    if !keys[node as usize].is_element
        || !compound_matches(&keys[node as usize], compound)
        || !pseudo_matches(doc, keys, node, compound)
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
                if match_at(doc, keys, candidate, selector, index - 1) {
                    return true;
                }
                ancestor = doc.node(candidate).parent;
            }
            false
        }
        Combinator::Child => doc
            .node(node)
            .parent
            .is_some_and(|parent| match_at(doc, keys, parent, selector, index - 1)),
        Combinator::AdjacentSibling => previous_element_sibling(doc, keys, node)
            .is_some_and(|sibling| match_at(doc, keys, sibling, selector, index - 1)),
    }
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
    fn unsupported_selectors_are_rejected_whole() {
        for selector in [
            "p ~ span",
            "p[class]",
            ":not(.x)",
            "p::before",
            "div > > p",
            "> p",
            "h1 +",
        ] {
            assert!(
                parse_selector(selector).is_none(),
                "{selector} 应当整条丢弃"
            );
        }
        // 支持的形式仍能解析出组合器。
        let selector = parse_selector("div > p + h1").expect("selector");
        assert_eq!(selector.len(), 3);
        assert_eq!(selector[1].combinator, Combinator::Child);
        assert_eq!(selector[2].combinator, Combinator::AdjacentSibling);
    }
}
