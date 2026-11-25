//! 全书高度索引：估计值 + 实测值 + 前缀和。
//!
//! 滚动条需要“全书多高”，但精确高度只能靠逐章排版得到。把这两件事绑在一起，
//! 就会在第一次显示滚动条时排完整本书。这里把高度拆成两层：
//!
//! - 每章先有一个**估计值**（来自章节字节数 + 视口 + 样式），打开书后立刻可得；
//! - 滚到某章时才用排版结果替换该章的估计值，并 O(log n) 更新前缀和。
//!
//! 查询/滚动只依赖当前索引，不要求 `0..i` 都已实测。宿主可以导出/导入实测值
//! （键是书指纹 + 视口 + 样式 + 字体版本），第二次打开直接恢复上次的精度。

use crate::types::{Style, Viewport};

const MAGIC: &[u8; 5] = b"EBHX1";
const VERSION: u8 = 1;
const HEADER: usize = MAGIC.len() + 1 + 8 + 4;

/// 一行正文的基准字号。书里的字号已经写进 `StyledDoc`，但估计发生在解析
/// 之前，只能用一个保守的基准值。
const BASE_FONT_PX: f32 = 16.0;
/// 混合中英文的平均每字符字节数（UTF-8 中文 3 字节，英文 1 字节）。
const BYTES_PER_CHAR: f32 = 2.0;
/// 混合文本的平均字宽（em）。中文 1.0、拉丁约 0.5。
const AVG_CHAR_EM: f32 = 0.85;
const LINE_HEIGHT_MUL: f32 = 1.55;

/// 用章节字节数估一个高度。结果只用于滚动条的初始位置；实测会覆盖它。
pub(crate) fn estimate_height(
    bytes: Option<u64>,
    viewport: Viewport,
    style: Style,
    fallback: f32,
) -> f32 {
    let count = viewport.width - viewport.insets.left - viewport.insets.right;
    let content_w = if count.is_finite() && count > 1.0 {
        count
    } else {
        return fallback.max(1.0);
    };
    let font = BASE_FONT_PX * style.font_scale.max(0.1);
    let line_h = font * LINE_HEIGHT_MUL * style.line_scale.max(0.1);
    if line_h <= 0.0 || !line_h.is_finite() {
        return fallback.max(1.0);
    }
    let Some(bytes) = bytes else {
        return fallback.max(viewport.height.max(1.0));
    };
    let chars = bytes as f32 / BYTES_PER_CHAR;
    let per_line = (content_w / (font * AVG_CHAR_EM)).max(1.0);
    let lines = (chars / per_line).max(1.0);
    // 段间距/标题/图片留 12% 余量；最少一屏，和 `ceil_to_viewport` 的语义一致。
    let raw = lines * line_h * 1.12;
    let floor = viewport.height.max(1.0);
    let estimate = raw.max(floor);
    if estimate.is_finite() {
        estimate
    } else {
        fallback.max(floor)
    }
}

/// 一本书的滚动高度表。
#[derive(Clone)]
pub(crate) struct HeightIndex {
    key: u64,
    /// 每章的实测高度；`None` 表示还在用估计值。
    measured: Vec<Option<f32>>,
    /// 当前使用的高度（实测优先，否则估计）。
    heights: Vec<f32>,
    /// 1 基 Fenwick 树，存 `heights` 的前缀和。用 f64 避免十万章累加的漂移。
    tree: Vec<f64>,
    /// 排版失败的原因。失败章高度按 0 计，不阻塞后面的章。
    errors: Vec<Option<String>>,
    measured_count: usize,
}

impl HeightIndex {
    pub(crate) fn with_estimates(estimates: Vec<f32>, key: u64) -> Self {
        let n = estimates.len();
        let heights: Vec<f32> = estimates
            .into_iter()
            .map(|h| if h.is_finite() && h > 0.0 { h } else { 0.0 })
            .collect();
        let mut tree = vec![0.0f64; n + 1];
        for (index, height) in heights.iter().enumerate() {
            tree[index + 1] += *height as f64;
            let parent = (index + 1) + (index + 1).isolate_lowest_one();
            if parent <= n {
                tree[parent] += tree[index + 1];
            }
        }
        Self {
            key,
            measured: vec![None; n],
            heights,
            tree,
            errors: vec![None; n],
            measured_count: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.heights.len()
    }

    pub(crate) fn set_key(&mut self, key: u64) {
        self.key = key;
    }

    /// 尾部追加一章（append 之后）。索引键由调用方随后更新。
    pub(crate) fn push_estimate(&mut self, height: f32) {
        let height = if height.is_finite() && height > 0.0 {
            height
        } else {
            0.0
        };
        self.heights.push(height);
        self.measured.push(None);
        self.errors.push(None);
        self.tree.push(0.0);
        self.add(self.heights.len() - 1, height as f64);
    }

    pub(crate) fn measured_count(&self) -> usize {
        self.measured_count
    }

    pub(crate) fn is_measured(&self, index: usize) -> bool {
        self.measured.get(index).is_some_and(Option::is_some)
    }

    pub(crate) fn error(&self, index: usize) -> Option<&str> {
        self.errors.get(index).and_then(|entry| entry.as_deref())
    }

    pub(crate) fn height(&self, index: usize) -> f32 {
        self.heights.get(index).copied().unwrap_or(0.0)
    }

    pub(crate) fn total(&self) -> f32 {
        self.prefix(self.len()) as f32
    }

    /// 第 `index` 章在全书的起点。
    pub(crate) fn top(&self, index: usize) -> f32 {
        self.prefix(index.min(self.len())) as f32
    }

    /// 全书流坐标 `y` 落在哪一章。不触发任何排版。
    pub(crate) fn locate(&self, y: f32) -> Option<usize> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        if !y.is_finite() || y < 0.0 {
            return Some(0);
        }
        if y >= self.total() {
            return None;
        }
        let mut index = 0usize;
        let mut rest = y as f64;
        let mut bit = 1usize;
        while (bit << 1) <= n {
            bit <<= 1;
        }
        while bit > 0 {
            let next = index + bit;
            if next <= n && self.tree[next] <= rest {
                index = next;
                rest -= self.tree[next];
            }
            bit >>= 1;
        }
        (index < n).then_some(index)
    }

    /// 记录一章的实测高度。返回是否从“估计”变成了“实测”。
    pub(crate) fn set_measured(&mut self, index: usize, height: f32) -> bool {
        if index >= self.len() {
            return false;
        }
        let height = if height.is_finite() && height >= 0.0 {
            height
        } else {
            0.0
        };
        let first = self.measured[index].is_none();
        self.measured[index] = Some(height);
        self.errors[index] = None;
        let old = self.heights[index];
        self.heights[index] = height;
        self.add(index, height as f64 - old as f64);
        if first {
            self.measured_count += 1;
        }
        first
    }

    pub(crate) fn set_error(&mut self, index: usize, message: String) {
        if index >= self.len() {
            return;
        }
        // 失败章按 0 高计；它也算“量过”，否则每次路过都会重试同一个坏章。
        let old = self.heights[index];
        self.heights[index] = 0.0;
        self.add(index, -(old as f64));
        if self.measured[index].is_none() {
            self.measured_count += 1;
        }
        self.measured[index] = Some(0.0);
        self.errors[index] = Some(message);
    }

    /// 只在章节数不变时可用：把某一章的实测值作废（内容被替换了）。
    pub(crate) fn invalidate(&mut self, index: usize, estimate: f32) {
        if index >= self.len() {
            return;
        }
        if self.measured[index].is_some() {
            self.measured_count = self.measured_count.saturating_sub(1);
        }
        self.measured[index] = None;
        self.errors[index] = None;
        let old = self.heights[index];
        let height = if estimate.is_finite() && estimate > 0.0 {
            estimate
        } else {
            0.0
        };
        self.heights[index] = height;
        self.add(index, height as f64 - old as f64);
    }

    pub(crate) fn export(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.len() * 5);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.key.to_le_bytes());
        out.extend_from_slice(&(self.len() as u32).to_le_bytes());
        for index in 0..self.len() {
            match (self.measured[index], self.errors[index].is_some()) {
                (Some(_), true) => out.push(2),
                (Some(height), false) => {
                    out.push(1);
                    out.extend_from_slice(&height.to_le_bytes());
                }
                (None, _) => out.push(0),
            }
        }
        out
    }

    /// 导入另一个会话导出的索引。键或章数不匹配时返回 `false`，不部分应用。
    pub(crate) fn import(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() < HEADER || &bytes[..MAGIC.len()] != MAGIC {
            return false;
        }
        if bytes[MAGIC.len()] != VERSION {
            return false;
        }
        let key_at = MAGIC.len() + 1;
        let mut key = [0u8; 8];
        key.copy_from_slice(&bytes[key_at..key_at + 8]);
        let key = u64::from_le_bytes(key);
        let count_at = key_at + 8;
        let mut count = [0u8; 4];
        count.copy_from_slice(&bytes[count_at..count_at + 4]);
        let count = u32::from_le_bytes(count) as usize;
        if key != self.key || count != self.len() {
            return false;
        }
        let mut p = HEADER;
        let mut rows: Vec<(bool, f32)> = Vec::with_capacity(count);
        for _ in 0..count {
            let Some(&flag) = bytes.get(p) else {
                return false;
            };
            p += 1;
            match flag {
                1 => {
                    let Some(raw) = bytes.get(p..p + 4) else {
                        return false;
                    };
                    let mut value = [0u8; 4];
                    value.copy_from_slice(raw);
                    rows.push((false, f32::from_le_bytes(value)));
                    p += 4;
                }
                2 => rows.push((true, 0.0)),
                _ => rows.push((false, f32::NAN)),
            }
        }
        for (index, (failed, height)) in rows.into_iter().enumerate() {
            if failed {
                self.set_error(
                    index,
                    String::from("previous session marked this chapter as failed"),
                );
            } else if height.is_nan() {
                if self.measured[index].is_some() {
                    self.measured[index] = None;
                    self.measured_count = self.measured_count.saturating_sub(1);
                }
                self.errors[index] = None;
            } else {
                self.set_measured(index, height);
            }
        }
        true
    }

    fn prefix(&self, index: usize) -> f64 {
        let mut index = index.min(self.len());
        let mut sum = 0.0;
        while index > 0 {
            sum += self.tree[index];
            index &= index - 1;
        }
        sum
    }

    fn add(&mut self, index: usize, delta: f64) {
        if delta == 0.0 || !delta.is_finite() {
            return;
        }
        let mut i = index + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += i.isolate_lowest_one();
        }
    }
}

/// 由引擎维护的键：书内容 + 视口几何 + 阅读样式 + 字体版本。
pub(crate) fn index_key(book: u64, viewport: Viewport, style: Style, font_rev: u32) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(b"ebook-height-index-v1");
    eat(&book.to_le_bytes());
    eat(&viewport.width.to_bits().to_le_bytes());
    eat(&viewport.height.to_bits().to_le_bytes());
    eat(&viewport.insets.top.to_bits().to_le_bytes());
    eat(&viewport.insets.right.to_bits().to_le_bytes());
    eat(&viewport.insets.bottom.to_bits().to_le_bytes());
    eat(&viewport.insets.left.to_bits().to_le_bytes());
    eat(&style.font_scale.to_bits().to_le_bytes());
    eat(&style.line_scale.to_bits().to_le_bytes());
    eat(&style.letter_px.to_bits().to_le_bytes());
    eat(&style.word_px.to_bits().to_le_bytes());
    eat(&style.align.to_le_bytes());
    eat(&font_rev.to_le_bytes());
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> HeightIndex {
        HeightIndex::with_estimates(vec![100.0, 200.0, 50.0, 150.0], 7)
    }

    #[test]
    fn prefix_and_locate_follow_updates() {
        let mut index = index();
        assert_eq!(index.total(), 500.0);
        assert_eq!(index.top(2), 300.0);
        assert_eq!(index.locate(0.0), Some(0));
        assert_eq!(index.locate(99.0), Some(0));
        assert_eq!(index.locate(100.0), Some(1));
        assert_eq!(index.locate(299.0), Some(1));
        assert_eq!(index.locate(300.0), Some(2));
        assert_eq!(index.locate(499.0), Some(3));
        assert_eq!(index.locate(500.0), None);

        assert!(index.set_measured(1, 20.0));
        assert_eq!(index.total(), 320.0);
        assert_eq!(index.top(2), 120.0);
        assert_eq!(index.locate(120.0), Some(2));
        assert_eq!(index.measured_count(), 1);
    }

    #[test]
    fn empty_heights_never_move_the_prefix_to_zero() {
        let mut index = HeightIndex::with_estimates(vec![0.0, 10.0], 1);
        assert_eq!(index.total(), 10.0);
        assert_eq!(index.locate(0.0), Some(1));
        index.set_measured(0, 0.0);
        assert_eq!(index.locate(0.0), Some(1));
        assert_eq!(index.locate(5.0), Some(1));
    }

    #[test]
    fn export_import_round_trips_measured_values_only() {
        let mut index = index();
        index.set_measured(0, 42.0);
        index.set_error(2, String::from("bad chapter"));
        let bytes = index.export();
        let mut restored = HeightIndex::with_estimates(vec![9.0; 4], 7);
        assert!(restored.import(&bytes));
        assert_eq!(restored.height(0), 42.0);
        assert_eq!(restored.height(2), 0.0);
        assert!(restored.error(2).is_some());
        assert_eq!(restored.height(1), 9.0);
        assert_eq!(restored.measured_count(), 2);
        assert!(!restored.import(&bytes[..bytes.len() - 1]));

        let mut wrong_key = HeightIndex::with_estimates(vec![9.0; 4], 8);
        assert!(!wrong_key.import(&bytes));
        let mut wrong_len = HeightIndex::with_estimates(vec![9.0; 3], 7);
        assert!(!wrong_len.import(&bytes));
    }
}
