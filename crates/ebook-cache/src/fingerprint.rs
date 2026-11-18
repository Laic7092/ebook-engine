//! 输入指纹。`CacheKey` 只由排版输入决定：
//!
//! - 文档结构：节点数、每个节点的 kind / 样式全部字段 / parent / 子区间 /
//!   锚点 / 链接、children 顺序、text 字节、图片元数据、meta。
//! - 字体身份：`FontSet` 里每个字体的 family / weight / style / 字节长度和
//!   前 64 字节。
//! - 页面约束、调用方的 `policy_rev`。
//!
//! 哈希用 FNV-1a。它不是密码学哈希，这里也不需要：缓存是 caller-owned，
//! 键碰撞最多导致一次错误命中的风险，而 `CacheKey` 只在此 crate 内流动。
//! 所有 f32 都按 `to_bits` 参与，保证 `-0.0` / `0.0` 这类差别不会被吞掉。

use ebook_ir::{
    Anchor, Border, BreakPolicy, ComputedStyle, FontFamilies, FontSet, FontSpec, ImageMeta, Len,
    LineHeight, ListStyle, Node, NodeKind, SizeF, StyledDoc, TextStyle, VerticalAlign,
};

use ebook_layout::PageConstraint;

/// 输入指纹。同一个输入必然得到同一个值；任何参与排版的字节变了，值就变。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey(u64);

impl CacheKey {
    pub fn value(self) -> u64 {
        self.0
    }
}

/// 一个输入指纹：文档 + 字体集合 + 页面约束 + 调用方策略版本。
pub fn fingerprint(
    doc: &StyledDoc,
    fonts: Option<&FontSet>,
    c: &PageConstraint,
    policy_rev: u32,
) -> CacheKey {
    let font_digest = fonts.map(font_fingerprint);
    fingerprint_with_fonts(doc, font_digest, c, policy_rev)
}

/// 已经算过字体摘要时走这里（`LayoutCache` 缓存的就是这个摘要）。
pub fn fingerprint_with_fonts(
    doc: &StyledDoc,
    fonts: Option<u64>,
    c: &PageConstraint,
    policy_rev: u32,
) -> CacheKey {
    let mut h = Fnv::new();
    // 域分隔：不同输入类型进同一个哈希也不会结构性碰撞。
    h.str("ebook-layout-cache-v1");
    hash_doc(&mut h, doc);
    match fonts {
        Some(digest) => {
            h.u8(1);
            h.u64(digest);
        }
        None => h.u8(0),
    }
    h.f32(c.page.w);
    h.f32(c.page.h);
    h.f32(c.margin.top);
    h.f32(c.margin.right);
    h.f32(c.margin.bottom);
    h.f32(c.margin.left);
    h.u8(c.writing_mode as u8);
    h.u32(policy_rev);
    CacheKey(h.finish())
}

/// 字体集合的身份。只取每个字体的前 64 字节：足够区分不同字体文件，
/// 又不会为了缓存键把几十 MB 的字体重新哈希一遍。
pub fn font_fingerprint(fonts: &FontSet) -> u64 {
    let mut h = Fnv::new();
    h.str("ebook-fonts-v1");
    h.u32(fonts.fonts.len() as u32);
    for font in &fonts.fonts {
        h.str(&font.desc.family);
        h.u16(font.desc.weight);
        h.u8(font.desc.style as u8);
        h.u32(font.bytes.len() as u32);
        h.bytes(&font.bytes[..font.bytes.len().min(64)]);
    }
    h.finish()
}

fn hash_doc(h: &mut Fnv, doc: &StyledDoc) {
    h.u32(doc.meta.spine);
    hash_opt_str(h, doc.meta.title.as_deref());
    hash_opt_str(h, doc.meta.lang.as_deref());
    h.u8(doc.meta.dir as u8);
    h.u8(doc.meta.writing_mode as u8);

    h.u32(doc.nodes.len() as u32);
    for node in &doc.nodes {
        hash_node(h, node);
    }

    h.u32(doc.children.len() as u32);
    for &child in &doc.children {
        h.u32(child);
    }

    h.u32(doc.text.len() as u32);
    h.bytes(doc.text.as_bytes());

    h.u32(doc.images.len() as u32);
    for image in &doc.images {
        hash_image(h, image);
    }

    h.u32(doc.root);
}

fn hash_node(h: &mut Fnv, node: &Node) {
    match &node.kind {
        NodeKind::Element(tag) => {
            h.u8(0);
            h.u8(*tag as u8);
        }
        NodeKind::Text(span) => {
            h.u8(1);
            h.u32(span.start);
            h.u32(span.len);
        }
        NodeKind::LineBreak => h.u8(2),
        NodeKind::Image(id) => {
            h.u8(3);
            h.u16(*id);
        }
        NodeKind::Ruby { base, rt } => {
            h.u8(4);
            h.u32(base.start);
            h.u32(base.len);
            h.u32(rt.start);
            h.u32(rt.len);
        }
    }
    hash_style(h, &node.style);
    match node.parent {
        Some(parent) => {
            h.u8(1);
            h.u32(parent);
        }
        None => h.u8(0),
    }
    h.u32(node.first_child);
    h.u32(node.child_len);
    match &node.src {
        Some(anchor) => {
            h.u8(1);
            hash_anchor(h, anchor);
        }
        None => h.u8(0),
    }
    hash_opt_str(h, node.link.as_deref());
}

fn hash_anchor(h: &mut Fnv, anchor: &Anchor) {
    h.u32(anchor.spine);
    h.u32(anchor.path.len() as u32);
    for step in &anchor.path {
        h.u16(*step);
    }
    h.u32(anchor.offset);
}

fn hash_image(h: &mut Fnv, image: &ImageMeta) {
    match image.size {
        Some(size) => {
            h.u8(1);
            hash_size(h, size);
        }
        None => h.u8(0),
    }
    hash_opt_str(h, image.mime.as_deref());
    hash_opt_str(h, image.alt.as_deref());
}

fn hash_size(h: &mut Fnv, size: SizeF) {
    h.f32(size.w);
    h.f32(size.h);
}

fn hash_style(h: &mut Fnv, style: &ComputedStyle) {
    h.u8(style.display as u8);
    hash_len(h, style.width);
    hash_len(h, style.height);
    hash_sides(h, &style.margin);
    hash_sides(h, &style.padding);
    hash_border(h, &style.border);
    hash_font_spec(h, &style.font);
    hash_line_height(h, style.line_height);
    hash_text_style(h, &style.text);
    hash_color(h, style.color);
    match style.background {
        Some(color) => {
            h.u8(1);
            hash_color(h, color);
        }
        None => h.u8(0),
    }
    h.u8(style.writing_mode as u8);
    h.u8(style.dir as u8);
    hash_break_policy(h, &style.brk);
    match style.list {
        Some(list) => {
            h.u8(1);
            hash_list(h, list);
        }
        None => h.u8(0),
    }
}

fn hash_border(h: &mut Fnv, border: &Border) {
    hash_sides(h, &border.width);
    hash_color(h, border.color);
}

fn hash_list(h: &mut Fnv, list: ListStyle) {
    h.u8(list.ordered as u8);
    h.u16(list.start);
}

fn hash_sides(h: &mut Fnv, sides: &ebook_ir::Sides) {
    hash_len(h, sides.top);
    hash_len(h, sides.right);
    hash_len(h, sides.bottom);
    hash_len(h, sides.left);
}

fn hash_len(h: &mut Fnv, len: Len) {
    match len {
        Len::Auto => h.u8(0),
        Len::Px(v) => {
            h.u8(1);
            h.f32(v);
        }
        Len::Pct(v) => {
            h.u8(2);
            h.f32(v);
        }
        Len::Em(v) => {
            h.u8(3);
            h.f32(v);
        }
        Len::Rem(v) => {
            h.u8(4);
            h.f32(v);
        }
    }
}

fn hash_line_height(h: &mut Fnv, height: LineHeight) {
    match height {
        LineHeight::Normal => h.u8(0),
        LineHeight::Px(v) => {
            h.u8(1);
            h.f32(v);
        }
        LineHeight::Mul(v) => {
            h.u8(2);
            h.f32(v);
        }
    }
}

fn hash_font_spec(h: &mut Fnv, font: &FontSpec) {
    h.u16(font.font.0);
    h.f32(font.size);
    h.u16(font.weight);
    h.u8(font.style as u8);
    hash_families(h, &font.families);
}

fn hash_families(h: &mut Fnv, families: &FontFamilies) {
    h.u32(families.len() as u32);
    for family in families.iter() {
        h.str(family.as_str());
    }
}

fn hash_text_style(h: &mut Fnv, text: &TextStyle) {
    h.u8(text.align as u8);
    hash_len(h, text.indent);
    h.f32(text.letter_spacing);
    h.f32(text.word_spacing);
    h.u8(text.white_space as u8);
    h.u8(text.word_break as u8);
    h.u8(text.hyphens as u8);
    h.u8(text.transform as u8);
    hash_vertical_align(h, text.vertical_align);
}

fn hash_vertical_align(h: &mut Fnv, align: VerticalAlign) {
    match align {
        VerticalAlign::Baseline => h.u8(0),
        VerticalAlign::Super => h.u8(1),
        VerticalAlign::Sub => h.u8(2),
        VerticalAlign::Top => h.u8(3),
        VerticalAlign::Middle => h.u8(4),
        VerticalAlign::Bottom => h.u8(5),
        VerticalAlign::TextTop => h.u8(6),
        VerticalAlign::TextBottom => h.u8(7),
        VerticalAlign::Len(len) => {
            h.u8(8);
            hash_len(h, len);
        }
    }
}

fn hash_break_policy(h: &mut Fnv, policy: &BreakPolicy) {
    h.u8(policy.before as u8);
    h.u8(policy.after as u8);
    h.u8(policy.inside as u8);
    h.u16(policy.orphans);
    h.u16(policy.widows);
}

fn hash_color(h: &mut Fnv, color: ebook_ir::Rgba8) {
    h.u8(color.r);
    h.u8(color.g);
    h.u8(color.b);
    h.u8(color.a);
}

fn hash_opt_str(h: &mut Fnv, value: Option<&str>) {
    match value {
        Some(value) => {
            h.u8(1);
            h.str(value);
        }
        None => h.u8(0),
    }
}

/// FNV-1a，64 位。只在本 crate 内使用，参数固定，跨平台结果一致。
struct Fnv(u64);

impl Fnv {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn u8(&mut self, value: u8) {
        self.0 ^= value as u64;
        self.0 = self.0.wrapping_mul(Self::PRIME);
    }

    fn u16(&mut self, value: u16) {
        for byte in value.to_le_bytes() {
            self.u8(byte);
        }
    }

    fn u32(&mut self, value: u32) {
        for byte in value.to_le_bytes() {
            self.u8(byte);
        }
    }

    fn u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.u8(byte);
        }
    }

    fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }

    fn bytes(&mut self, value: &[u8]) {
        for &byte in value {
            self.u8(byte);
        }
    }

    /// 先哈希长度再哈希字节：`["ab", "c"]` 与 `["a", "bc"]` 不会撞。
    fn str(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.bytes(value.as_bytes());
    }

    fn finish(self) -> u64 {
        self.0
    }
}
