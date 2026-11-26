//! 排版前段（盒树 + 断行 + 整形）的复用。
//!
//! 分页依赖页高，断行只依赖**可用宽度**。于是有这么一类输入变化：手机地址栏
//! 收起、旋屏、分页与滚动模式切换、同宽不同高的重排——宽度没变，`flow` 的输出
//! 逐字节相同，重跑一遍纯属浪费。这里把 `flow` 单独缓存住，分页与组装照旧重跑。
//!
//! 边界与 `ebook-cache` 一致：缓存是 caller-owned，键由调用方给的 `rev` 加
//! 宽度、行数预算组成。`rev` 必须覆盖所有参与排版的**文档与字体字节**：同一个
//! `rev` 下 `doc` 与 `env` 必须逐字节等价。它不参与序列化，也不进 IR。

use crate::flow::Flow;

/// 前段排版结果（`Flow`）的一格缓存。
///
/// 只有一格：命中的典型场景是「同一章的同一个宽度，换个页高再排一次」，两次
/// 之间不会插进别的章。留一格不需要淘汰策略，代价是常驻一份 `Flow`
/// （一章几十到几百 KB）——比保留几份完整 `LayoutResult` 便宜，也比重跑断行便宜。
#[derive(Default)]
pub struct FlowCache {
    key: Option<u64>,
    flow: Option<Flow>,
    hits: u64,
    misses: u64,
}

impl FlowCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 命中次数。命中意味着盒树、断行、整形都没重跑。
    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// 缓存中 `Flow` 的近似字节数。用于宿主内存诊断。
    pub fn bytes(&self) -> usize {
        self.flow.as_ref().map_or(0, Flow::approx_bytes)
    }

    /// 丢掉缓存。字体或文档换掉时用它，比记着改 `rev` 更难写错。
    pub fn clear(&mut self) {
        self.key = None;
        self.flow = None;
    }

    /// 命中的结果是克隆：`run` 后面会 `drain` 这份 `items`，缓存要留着下次用。
    /// 一页几千个字形是几十 KB 的 memcpy，比重新整形断行便宜两个数量级。
    pub(crate) fn get(&mut self, key: u64) -> Option<Flow> {
        if self.key == Some(key) {
            if let Some(flow) = self.flow.as_ref() {
                self.hits += 1;
                return Some(flow.clone());
            }
        }
        self.misses += 1;
        None
    }

    pub(crate) fn put(&mut self, key: u64, flow: &Flow) {
        self.key = Some(key);
        self.flow = Some(flow.clone());
    }
}

/// `flow` 的全部外部依赖：调用方版本号、可用宽度、行数预算。
///
/// **刻意不含页高、不含 `Mode`。** 它们只影响分页与页尺寸，不参与断行；把它们
/// 放进键里就等于把「换个页高」变成整章重排，这个缓存也就没有意义了。
pub(crate) fn flow_key(rev: u64, content_w: f32, max_lines: u32) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    // 域分隔：三种输入的字节拼在一起也不会与另一种拼法撞出同一个值。
    eat(b"ebook-layout-flow-v1");
    eat(&rev.to_le_bytes());
    eat(&content_w.to_bits().to_le_bytes());
    eat(&max_lines.to_le_bytes());
    hash
}

#[cfg(test)]
mod tests {
    use super::flow_key;

    #[test]
    fn key_separates_every_input() {
        let base = flow_key(1, 300.0, 0);
        assert_ne!(base, flow_key(2, 300.0, 0));
        assert_ne!(base, flow_key(1, 301.0, 0));
        assert_ne!(base, flow_key(1, 300.0, 1));
        // -0.0 与 0.0 参与排版的宽度相同，但这里仍然分开算：缓存键宁可多失效，
        // 也不能把两份不同输入当成同一份。
        assert_ne!(flow_key(1, 0.0, 0), flow_key(1, -0.0, 0));
        assert_eq!(base, flow_key(1, 300.0, 0));
    }
}
