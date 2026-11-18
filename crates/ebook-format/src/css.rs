use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ebook_ir::{
    Break, Display, Len, LineHeight, ListStyle, NodeId, Rgba8, StyledDoc, Tag, TextAlign,
    TextTransform, VerticalAlign, WhiteSpace, WordBreak, WritingMode,
};

type Spec = (u16, u16, u16);

#[derive(Clone, Debug)]
pub struct Decl {
    pub name: String,
    pub value: String,
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

#[derive(Clone, Debug, Default)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Clone, Debug)]
struct Rule {
    selector: Vec<Compound>,
    decls: Vec<Decl>,
    spec: Spec,
}

#[derive(Default)]
pub struct StyleSheet {
    rules: Vec<Rule>,
}

impl StyleSheet {
    pub fn parse_into(&mut self, css: &str) {
        let bytes = css.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            i = skip_space_and_comments(bytes, i);
            if i >= bytes.len() {
                break;
            }
            if bytes[i] == b'@' {
                i = skip_at_rule(bytes, i);
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
            i += 1;
            let body_start = i;
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            let body = &css[body_start..i];
            i += 1;
            let decls = parse_decls(body);
            if decls.is_empty() {
                continue;
            }
            for selector_text in selector_text.split(',') {
                if let Some(selector) = parse_selector(selector_text) {
                    let spec = specificity(&selector);
                    self.rules.push(Rule { selector, decls: decls.clone(), spec });
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

pub fn parse_decls(body: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    for part in body.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((name, value)) = part.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().trim_end_matches("!important").trim();
            if !name.is_empty() && !value.is_empty() {
                out.push(Decl { name, value: value.to_string() });
            }
        }
    }
    out
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
        for (_, i) in matched.iter() {
            for decl in &sheet.rules[*i].decls {
                apply(&mut style, decl, parent_size);
            }
        }
        for decl in &keys[id as usize].inline {
            apply(&mut style, decl, parent_size);
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

fn apply(style: &mut ebook_ir::ComputedStyle, decl: &Decl, parent_size: f32) {
    let value = decl.value.trim();
    let base = style.font.size;
    match decl.name.as_str() {
        "display" => {
            if let Some(display) = parse_display(value) {
                style.display = display;
            }
        }
        "color" => {
            if let Some(color) = parse_color(value) {
                style.color = color;
            }
        }
        "background-color" | "background" => {
            if let Some(color) = parse_color(value) {
                style.background = if color.a == 0 { None } else { Some(color) };
            }
        }
        "font-size" => {
            if let Some(px) = parse_len(value).and_then(|l| len_px(l, parent_size)) {
                style.font.size = px.max(1.0);
            }
        }
        "font-weight" => {
            let weight = match value {
                "bold" | "bolder" => Some(700),
                "normal" | "lighter" => Some(400),
                other => other.parse::<u16>().ok(),
            };
            if let Some(w) = weight {
                style.font.weight = w;
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
            style.text.white_space = match value {
                "pre" => WhiteSpace::Pre,
                "nowrap" => WhiteSpace::Nowrap,
                "pre-wrap" => WhiteSpace::PreWrap,
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
            style.dir = if value == "rtl" { ebook_ir::Dir::Rtl } else { ebook_ir::Dir::Ltr }
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
        _ => {}
    }
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

fn set_one(target: &mut Len, value: &str) {
    if let Some(l) = parse_len(value) {
        *target = l;
    }
}

fn set_sides(sides: &mut ebook_ir::Sides, value: &str) {
    let parts: Vec<&str> = value.split_whitespace().collect();
    let values: Vec<Len> = parts.iter().filter_map(|p| parse_len(p)).collect();
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

pub fn parse_color(value: &str) -> Option<Rgba8> {
    let value = value.trim();
    if value == "transparent" {
        return Some(Rgba8::TRANSPARENT);
    }
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(inner) = value.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 3 {
            let r = parts[0].trim().parse::<u8>().ok()?;
            let g = parts[1].trim().parse::<u8>().ok()?;
            let b = parts[2].trim().parse::<u8>().ok()?;
            return Some(Rgba8::new(r, g, b, 255));
        }
    }
    let named = match value {
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

fn parse_hex(hex: &str) -> Option<Rgba8> {
    let expand = |c: u8| -> Option<u8> {
        let v = (c as char).to_digit(16)? as u8;
        Some(v * 17)
    };
    if hex.len() == 3 {
        let bytes = hex.as_bytes();
        return Some(Rgba8::new(
            expand(bytes[0])?,
            expand(bytes[1])?,
            expand(bytes[2])?,
            255,
        ));
    }
    if hex.len() == 6 {
        let v = u32::from_str_radix(hex, 16).ok()?;
        return Some(Rgba8::new(
            (v >> 16) as u8,
            ((v >> 8) & 0xff) as u8,
            (v & 0xff) as u8,
            255,
        ));
    }
    None
}

fn parse_selector(text: &str) -> Option<Vec<Compound>> {
    let mut out = Vec::new();
    for token in text.split_whitespace() {
        if token.is_empty() {
            continue;
        }
        if token.contains(['>', '+', '~', ':', '[', ']', '(', ')', '*']) && token != "*" {
            return None;
        }
        out.push(parse_compound(token)?);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn parse_compound(token: &str) -> Option<Compound> {
    if token == "*" {
        return Some(Compound::default());
    }
    let mut compound = Compound::default();
    let mut rest = token;
    let mut has_tag = false;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('#') {
            let end = after.find(['.', '#']).unwrap_or(after.len());
            if end == 0 {
                return None;
            }
            compound.id = Some(after[..end].to_string());
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '#']).unwrap_or(after.len());
            if end == 0 {
                return None;
            }
            compound.classes.push(after[..end].to_string());
            rest = &after[end..];
        } else {
            if has_tag {
                return None;
            }
            let end = rest.find(['.', '#']).unwrap_or(rest.len());
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
        classes += compound.classes.len() as u16;
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

fn matches(doc: &StyledDoc, keys: &[ElemKey], node: NodeId, selector: &[Compound]) -> bool {
    let Some(last) = selector.last() else {
        return false;
    };
    if !keys[node as usize].is_element || !compound_matches(&keys[node as usize], last) {
        return false;
    }
    let mut ancestor = doc.node(node).parent;
    for compound in selector[..selector.len() - 1].iter().rev() {
        let mut found = None;
        while let Some(candidate) = ancestor {
            if keys[candidate as usize].is_element
                && compound_matches(&keys[candidate as usize], compound)
            {
                found = Some(candidate);
                break;
            }
            ancestor = doc.node(candidate).parent;
        }
        match found {
            Some(hit) => ancestor = doc.node(hit).parent,
            None => return false,
        }
    }
    true
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

fn skip_at_rule(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i] != b'{' && bytes[i] != b';' {
        i += 1;
    }
    if i >= bytes.len() {
        return i;
    }
    if bytes[i] == b';' {
        return i + 1;
    }
    i += 1;
    let mut depth = 1;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    i
}
