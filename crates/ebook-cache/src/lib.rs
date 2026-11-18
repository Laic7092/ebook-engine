//! 排版缓存的确定性实现。
//!
//! 缓存键就是排版输入的指纹：文档结构、字体身份、页面约束和调用方给的
//! `policy_rev`。同样的输入永远得到同样的键，所以没有失效逻辑，也没有
//! 全局状态——`LayoutCache` 由调用方持有、由调用方决定生命周期。
//!
//! 这里刻意**不**把缓存字段塞进 `LayoutResult`：缓存是 IR 之外的一层。

mod fingerprint;

use std::collections::{HashMap, VecDeque};

use ebook_ir::{FontSet, LayoutResult, StyledDoc};
use ebook_layout::{Layout, LayoutEnv, PageConstraint};

pub use fingerprint::{fingerprint, font_fingerprint, CacheKey};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

/// 有界 LRU。`capacity` 为 0 时按 1 处理：`layout` 返回引用，必须有一块
/// 由缓存拥有的结果，否则签名就得改成返回拥有值。
pub struct LayoutCache {
    capacity: usize,
    entries: HashMap<CacheKey, LayoutResult>,
    order: VecDeque<CacheKey>,
    policy_rev: u32,
    fonts: Option<u64>,
    stats: CacheStats,
}

impl LayoutCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
            policy_rev: 0,
            fonts: None,
            stats: CacheStats::default(),
        }
    }

    /// 调用方的排版策略版本号。它参与缓存键：策略变了，旧条目自然全部失效。
    pub fn with_policy_rev(mut self, rev: u32) -> Self {
        self.policy_rev = rev;
        self
    }

    pub fn set_policy_rev(&mut self, rev: u32) {
        self.policy_rev = rev;
    }

    /// 登记本次排版使用的字体集合。字体身份参与缓存键：换字体后旧条目
    /// 不会被命中。`LayoutEnv` 里只有 `TextMeasurer`，拿不到 `FontSet`，
    /// 所以字体由调用方显式交给缓存。
    pub fn set_fonts(&mut self, fonts: &FontSet) {
        self.fonts = Some(font_fingerprint(fonts));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 命中就直接返回缓存里的结果；未命中就现场排版并按 LRU 插入/淘汰。
    pub fn layout(
        &mut self,
        doc: &StyledDoc,
        env: &LayoutEnv<'_>,
        c: &PageConstraint,
    ) -> &LayoutResult {
        let key = self.key(doc, c);
        if self.entries.contains_key(&key) {
            self.stats.hits += 1;
            self.touch(key);
            return &self.entries[&key];
        }

        self.stats.misses += 1;
        let ir = Layout::new().layout(doc, env, c);
        while self.order.len() >= self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if self.entries.remove(&oldest).is_some() {
                self.stats.evictions += 1;
            }
        }
        self.entries.insert(key, ir);
        self.order.push_back(key);
        &self.entries[&key]
    }

    fn key(&self, doc: &StyledDoc, c: &PageConstraint) -> CacheKey {
        fingerprint::fingerprint_with_fonts(doc, self.fonts, c, self.policy_rev)
    }

    fn touch(&mut self, key: CacheKey) {
        if let Some(index) = self.order.iter().position(|&k| k == key) {
            self.order.remove(index);
        }
        self.order.push_back(key);
    }
}

impl Default for LayoutCache {
    fn default() -> Self {
        Self::new(16)
    }
}
