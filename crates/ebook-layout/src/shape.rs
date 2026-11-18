//! 文本整形：把一段同字体同字号的字符变成一个字形序列。
//!
//! 排版层只认 [`TextMeasurer::shape`](crate::TextMeasurer::shape) 给出的结果，
//! 不关心后端是 HarfBuzz 还是逐字 advance。约定两条：
//!
//! - 字形按**逻辑顺序**输出（RTL run 由实现自己翻回去），这样 `ch` 单调不减；
//! - `ShapeGlyph::ch` 落在簇首字符的**字节偏移**上，一个合字可以有多个字形共享
//!   同一个 `ch`，`fi` 因此仍然映射回原文的 `f`。

use alloc::vec::Vec;
use ebook_ir::FontId;

use crate::measure::TextMeasurer;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShapeGlyph {
    pub gid: u16,
    pub dx: f32,
    pub dy: f32,
    /// 簇首字符在入参 `text` 里的字节偏移。
    pub ch: u32,
}

/// 一个整形簇：共享同一 `ch` 的若干字形，以及它们的总前进宽度。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Cluster {
    /// 在段落字形缓冲里的起点。
    pub(crate) glyph_start: u32,
    pub(crate) glyph_len: u16,
    pub(crate) advance: f32,
    /// 簇首字符在原文里的绝对字节偏移。
    pub(crate) ch: u32,
}

/// 逐字 advance 的整形实现：默认行为，也是没有字体时的兜底。
///
/// 每个字符一个字形、一个簇，等价于升级前的"单字 advance 累加"。
pub fn shape_per_char<M: TextMeasurer + ?Sized>(
    measurer: &M,
    text: &str,
    font: FontId,
    size: f32,
    out: &mut Vec<ShapeGlyph>,
) -> f32 {
    let mut total = 0.0;
    for (offset, ch) in text.char_indices() {
        let dx = measurer.advance(ch, font, size).max(0.0);
        total += dx;
        out.push(ShapeGlyph {
            gid: measurer.glyph_id(ch, font),
            dx,
            dy: 0.0,
            ch: offset as u32,
        });
    }
    total
}

/// 整形一段同字体文本，并把结果切成簇。
///
/// `abs` 是每个字符（`text.char_indices()` 顺序）在原文里的绝对字节偏移，
/// 用来把簇首偏移换回文档坐标。`char_cluster` 输出每个字符所属的簇下标，
/// 由调用方据此决定行宽与断点。
pub(crate) fn shape_run(
    measurer: &dyn TextMeasurer,
    font: FontId,
    size: f32,
    text: &str,
    abs: &[u32],
    glyphs: &mut Vec<ShapeGlyph>,
    clusters: &mut Vec<Cluster>,
    char_cluster: &mut Vec<u32>,
) -> f32 {
    let scrap_start = glyphs.len();
    let mut scrap: Vec<ShapeGlyph> = Vec::new();
    let total = measurer.shape(text, font, size, &mut scrap);

    // 字符起点表：簇的 `ch` 只能落在字符边界上。
    let mut starts: Vec<u32> = Vec::with_capacity(text.chars().count());
    for (offset, _) in text.char_indices() {
        starts.push(offset as u32);
    }
    let char_of = |byte: u32| -> usize {
        match starts.binary_search(&byte) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        }
    };

    glyphs.extend_from_slice(&scrap);

    // 按 `ch` 切簇。RTL run 由后端在返回前翻回逻辑顺序，所以这里可以假设单调。
    let mut first_char = 0usize;
    let mut index = 0usize;
    while index < scrap.len() {
        let byte = scrap[index].ch;
        let head = char_of(byte);
        let mut end = index + 1;
        while end < scrap.len() && scrap[end].ch == byte {
            end += 1;
        }
        let next_head = if end < scrap.len() {
            char_of(scrap[end].ch)
        } else {
            starts.len()
        };
        let mut advance = 0.0f32;
        for glyph in &mut glyphs[scrap_start + index..scrap_start + end] {
            advance += glyph.dx;
            if let Some(&absolute) = abs.get(head) {
                glyph.ch = absolute;
            }
        }
        let abs_head = abs.get(head).copied().unwrap_or(0);
        clusters.push(Cluster {
            glyph_start: (scrap_start + index) as u32,
            glyph_len: (end - index).min(u16::MAX as usize) as u16,
            advance,
            ch: abs_head,
        });
        let cluster_index = (clusters.len() - 1) as u32;
        for _ in first_char..next_head.min(starts.len()) {
            char_cluster.push(cluster_index);
        }
        first_char = next_head.min(starts.len());
        index = end;
    }

    // 没有字形（空文本或整段被丢弃）时，每个字符仍然各自成簇，保证下标对齐。
    while char_cluster.len() < starts.len() {
        let head = char_cluster.len();
        clusters.push(Cluster {
            glyph_start: glyphs.len() as u32,
            glyph_len: 0,
            advance: 0.0,
            ch: abs.get(head).copied().unwrap_or(0),
        });
        char_cluster.push((clusters.len() - 1) as u32);
    }

    total
}

/// 只缩不涨的末字形修正：把簇的总前进宽度写回最后一个字形。
///
/// 整形结果与字距叠加是两件事：字距按字符加，簇宽按字形和。两者对不上时
/// 以簇宽为准，否则断行用的宽度和绘制用的宽度会分叉。
pub(crate) fn pin_cluster(glyphs: &mut [ShapeGlyph], advance: f32) {
    let sum: f32 = glyphs.iter().map(|glyph| glyph.dx).sum();
    let delta = advance - sum;
    if delta.abs() < f32::EPSILON {
        return;
    }
    if let Some(last) = glyphs.last_mut() {
        last.dx += delta;
    }
}
