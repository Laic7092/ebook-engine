use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

use crate::font::{FontFamilies, FontId, FontStyle};
use crate::geom::{Rgba8, Sides, SizeF};

pub type NodeId = u32;
pub type ImageId = u16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

impl Span {
    pub const EMPTY: Span = Span { start: 0, len: 0 };

    pub const fn new(start: u32, len: u32) -> Self {
        Self { start, len }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn end(&self) -> u32 {
        self.start + self.len
    }

    pub fn as_range(&self) -> core::ops::Range<usize> {
        self.start as usize..self.end() as usize
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub spine: u32,
    pub path: Vec<u16>,
    pub offset: u32,
}

impl Anchor {
    pub fn step(&mut self, index: u16) {
        self.path.push(index);
    }

    pub fn with_offset(&self, offset: u32) -> Self {
        let mut next = self.clone();
        next.offset = offset;
        next
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    #[default]
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WritingMode {
    #[default]
    HorizontalTb,
    VerticalRl,
    VerticalLr,
}

impl WritingMode {
    pub fn is_vertical(&self) -> bool {
        !matches!(self, WritingMode::HorizontalTb)
    }

    pub fn line_dir(&self) -> Dir {
        match self {
            WritingMode::VerticalRl => Dir::Rtl,
            _ => Dir::Ltr,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tag {
    #[default]
    Root,
    Html,
    Head,
    Body,
    Style,
    Title,
    Meta,
    Link,
    Script,
    P,
    Div,
    Section,
    Article,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Span,
    Em,
    Strong,
    A,
    Li,
    Ol,
    Ul,
    Blockquote,
    Pre,
    Code,
    Img,
    Br,
    Ruby,
    Rt,
    Table,
    Tr,
    Td,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Element(Tag),
    Text(Span),
    LineBreak,
    Image(ImageId),
    Ruby { base: Span, rt: Span },
}

impl NodeKind {
    pub fn is_text(&self) -> bool {
        matches!(self, NodeKind::Text(_))
    }

    pub fn is_replaced(&self) -> bool {
        matches!(self, NodeKind::Image(_) | NodeKind::Ruby { .. })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Display {
    #[default]
    Block,
    Inline,
    InlineBlock,
    ListItem,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Len {
    #[default]
    Auto,
    Px(f32),
    Pct(f32),
    Em(f32),
    Rem(f32),
}

impl Len {
    pub const ZERO: Len = Len::Px(0.0);

    pub fn resolve(&self, base: f32, em: f32) -> Option<f32> {
        match *self {
            Len::Auto => None,
            Len::Px(v) => Some(v),
            Len::Pct(v) => Some(base * v / 100.0),
            Len::Em(v) => Some(em * v),
            Len::Rem(v) => Some(em * v),
        }
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, Len::Auto)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineHeight {
    #[default]
    Normal,
    Px(f32),
    Mul(f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhiteSpace {
    #[default]
    Normal,
    Pre,
    Nowrap,
    PreWrap,
    /// CSS `pre-line`：折叠空格，保留硬换行，允许软换行。
    PreLine,
    /// CSS `break-spaces`：保留空格与硬换行，允许软换行。
    BreakSpaces,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordBreak {
    #[default]
    Normal,
    BreakAll,
    KeepAll,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Break {
    #[default]
    Auto,
    Always,
    Avoid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum VerticalAlign {
    #[default]
    Baseline,
    Super,
    Sub,
    Top,
    Middle,
    Bottom,
    TextTop,
    TextBottom,
    Len(Len),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakPolicy {
    pub before: Break,
    pub after: Break,
    pub inside: Break,
    pub orphans: u16,
    pub widows: u16,
}

impl Default for BreakPolicy {
    fn default() -> Self {
        Self {
            before: Break::Auto,
            after: Break::Auto,
            inside: Break::Auto,
            // CSS Fragmentation 3 §4.4 的初始值是 2：单行落在页首/页尾要往回退。
            // 想关闭保护就显式写 1（合法的等效关闭值）。
            orphans: 2,
            widows: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListStyle {
    pub ordered: bool,
    pub start: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontSpec {
    pub font: FontId,
    pub size: f32,
    pub weight: u16,
    pub style: FontStyle,
    pub families: FontFamilies,
}

impl Default for FontSpec {
    fn default() -> Self {
        Self {
            font: FontId(0),
            size: 16.0,
            weight: 400,
            style: FontStyle::Normal,
            families: FontFamilies::new(),
        }
    }
}

impl FontSpec {
    pub fn new(size: f32, weight: u16, style: FontStyle) -> Self {
        Self {
            size,
            weight,
            style,
            ..Self::default()
        }
    }

    pub fn with_families(mut self, families: FontFamilies) -> Self {
        self.families = families;
        self
    }

    pub fn with_family(mut self, family: &str) -> Self {
        self.families.push(family);
        self
    }
}

/// 中文/日文排版政策。零值 = 规范默认：可选政策一律关闭。
///
/// 出处见 `crates/ebook-layout/src/punct.rs`：CSS Text Module Level 4
/// (`text-spacing-trim` / `hanging-punctuation`)、CLREQ §3.2/§3.3、JLREQ §3.1/§3.2。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRules {
    /// 成对标点各收半宽（`text-spacing-trim: normal` 的可判定部分）。
    pub spacing_trim: bool,
    /// 行末标点悬挂（`hanging-punctuation: allow-end`）。
    pub hanging_punctuation: bool,
    /// 中西文/中数字之间加 1/4 em 空隙（CLREQ §3.3 建议项，不是 CSS 初始值）。
    pub autospacing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub align: TextAlign,
    pub indent: Len,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub white_space: WhiteSpace,
    pub word_break: WordBreak,
    pub hyphens: bool,
    pub transform: TextTransform,
    pub vertical_align: VerticalAlign,
    /// 中文排版政策。旧数据缺这个字段时按规范默认（全关）解码。
    #[serde(default)]
    pub rules: TextRules,
    /// 下划线。链接默认带下划线，宿主直接画在字形下方。
    #[serde(default)]
    pub underline: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            align: TextAlign::Start,
            indent: Len::ZERO,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            white_space: WhiteSpace::Normal,
            word_break: WordBreak::Normal,
            hyphens: false,
            transform: TextTransform::None,
            vertical_align: VerticalAlign::Baseline,
            rules: TextRules::default(),
            underline: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Border {
    pub width: Sides,
    pub color: Rgba8,
}

impl Default for Border {
    fn default() -> Self {
        Self {
            width: Sides::all(Len::ZERO),
            color: Rgba8::BLACK,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComputedStyle {
    pub display: Display,
    pub width: Len,
    pub height: Len,
    pub margin: Sides,
    pub padding: Sides,
    pub border: Border,
    pub font: FontSpec,
    pub line_height: LineHeight,
    pub text: TextStyle,
    pub color: Rgba8,
    pub background: Option<Rgba8>,
    pub writing_mode: WritingMode,
    pub dir: Dir,
    pub brk: BreakPolicy,
    pub list: Option<ListStyle>,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            display: Display::Block,
            width: Len::Auto,
            height: Len::Auto,
            margin: Sides::all(Len::ZERO),
            padding: Sides::all(Len::ZERO),
            border: Border::default(),
            font: FontSpec::default(),
            line_height: LineHeight::Normal,
            text: TextStyle::default(),
            color: Rgba8::BLACK,
            background: None,
            writing_mode: WritingMode::HorizontalTb,
            dir: Dir::Ltr,
            brk: BreakPolicy::default(),
            list: None,
        }
    }
}

impl ComputedStyle {
    pub fn inline(font: FontSpec, color: Rgba8, dir: Dir) -> Self {
        Self {
            display: Display::Inline,
            font,
            color,
            dir,
            ..Self::default()
        }
    }

    pub fn is_inline(&self) -> bool {
        matches!(self.display, Display::Inline)
    }

    pub fn is_block_level(&self) -> bool {
        matches!(
            self.display,
            Display::Block | Display::ListItem | Display::InlineBlock
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub kind: NodeKind,
    pub style: ComputedStyle,
    pub parent: Option<NodeId>,
    pub first_child: u32,
    pub child_len: u32,
    pub src: Option<Anchor>,
    /// 链接目标（`<a href>`）。默认空；格式层负责填，排版层只负责发射 `Op::Link`。
    pub link: Option<String>,
}

impl Node {
    pub fn is_leaf(&self) -> bool {
        self.child_len == 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub size: Option<SizeF>,
    pub mime: Option<String>,
    pub alt: Option<String>,
    /// 图片在归档内的路径（由格式层的 resolver 解析，通常是相对包根的路径）。
    ///
    /// 这是 `ImageId` 指向本章 `images` 表的唯一标识：宿主按 `src` 找到字节，
    /// 再交给渲染后端。旧数据缺这个字段时是 `None`（无法定位字节）。
    #[serde(default)]
    pub src: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocMeta {
    pub spine: u32,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub dir: Dir,
    pub writing_mode: WritingMode,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StyledDoc {
    pub meta: DocMeta,
    pub nodes: Vec<Node>,
    pub children: Vec<NodeId>,
    pub text: String,
    pub images: Vec<ImageMeta>,
    pub root: NodeId,
}

impl StyledDoc {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id as usize]
    }

    pub fn kids(&self, id: NodeId) -> &[NodeId] {
        let n = &self.nodes[id as usize];
        let start = n.first_child as usize;
        let end = start + n.child_len as usize;
        &self.children[start..end]
    }

    pub fn text_of(&self, span: Span) -> &str {
        &self.text[span.as_range()]
    }

    pub fn push_node(&mut self, mut node: Node) -> NodeId {
        let id = self.nodes.len() as NodeId;
        node.first_child = 0;
        node.child_len = 0;
        self.nodes.push(node);
        id
    }

    pub fn set_kids(&mut self, id: NodeId, kids: &[NodeId]) {
        let first = self.children.len() as u32;
        self.children.extend_from_slice(kids);
        let n = &mut self.nodes[id as usize];
        n.first_child = first;
        n.child_len = kids.len() as u32;
    }

    pub fn walk(&self) -> Walk<'_> {
        Walk {
            doc: self,
            stack: alloc::vec![self.root],
        }
    }
}

pub struct Walk<'a> {
    doc: &'a StyledDoc,
    stack: Vec<NodeId>,
}

impl<'a> Iterator for Walk<'a> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let id = self.stack.pop()?;
        for &kid in self.doc.kids(id).iter().rev() {
            self.stack.push(kid);
        }
        Some(id)
    }
}
