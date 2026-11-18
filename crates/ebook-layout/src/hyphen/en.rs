//! 美式英语断字：Liang 算法 + ushyphmax 模式（Liang, F. M., Word Hy-phen-a-tion
//! by Com-put-er, 1983）。
//!
//! 模式数据是 `en.pat`，由 `data/gen_hyphen.py` 从 hyph-utf8 的
//! `hyph-en-us.tex`（`~/layx/render/hyphen/en/patterns.go` 是同一份）编译而来。
//! 版权与许可跟随 hyph-utf8：
//!
//! ```text
//! Copyright (C) 1990, 2004, 2005 Gerard D.C. Kuiken.
//! Copying and distribution of this file, with or without modification,
//! are permitted in any medium without royalty provided the copyright
//! notice and this notice are preserved.
//! ```
//!
//! 数据布局见 `data/gen_hyphen.py` 的文件头。解析是零拷贝的：直接按偏移读
//! `include_bytes!` 的切片，没有分配、没有全局表、没有 `OnceLock`。

use alloc::vec::Vec;

use super::Hyphenator;

/// 编好的模式表（`data/gen_hyphen.py` 的输出）。
const DATA: &[u8] = include_bytes!("en.pat");
const VERSION: u8 = 2;
const NODE_SIZE: usize = 6;
const EDGE_SIZE: usize = 3;
const HEADER_SIZE: usize = 17;

/// 英文 TeX 模式的 hyphenmins：断点左边至少 2 个字母、右边至少 3 个。
const LEFT_MIN: usize = 2;
const RIGHT_MIN: usize = 3;
/// 比这短的字不断（`~/layx/render/hyphen/en/en.go` 的 `len(word) < 5`）。
const WORD_MIN: usize = 5;

#[derive(Clone, Copy, Debug, Default)]
pub struct English;

impl English {
    pub const fn new() -> Self {
        Self
    }
}

impl Hyphenator for English {
    fn breaks(&self, word: &str, out: &mut Vec<u16>) {
        out.clear();
        if word.len() < WORD_MIN || word.len() > u16::MAX as usize {
            return;
        }
        let mut letters: Vec<u8> = Vec::with_capacity(word.len());
        for byte in word.bytes() {
            let lower = byte.to_ascii_lowercase();
            if !lower.is_ascii_lowercase() {
                // 模式只对 a-z 有意义：数字、连字符、非 ASCII 一律不断。
                return;
            }
            letters.push(lower);
        }
        if letters.len() < WORD_MIN {
            return;
        }

        // Liang 算法在词两端加 `.`，模式因此能锚定词首词尾。
        let mut dotted: Vec<u8> = Vec::with_capacity(letters.len() + 2);
        dotted.push(b'.');
        dotted.extend_from_slice(&letters);
        dotted.push(b'.');

        let mut weights = alloc::vec![0u8; dotted.len() + 1];
        let view = View::parse(DATA);
        let Some(view) = view else {
            return;
        };
        for start in 0..dotted.len() {
            let mut node = 0u16;
            for &label in &dotted[start..] {
                let Some(child) = view.child(node, label) else {
                    break;
                };
                node = child;
                let (weight_start, weight_len) = view.weights(node);
                for k in 0..weight_len as usize {
                    let index = start + k;
                    if index >= weights.len() {
                        break;
                    }
                    let Some(weight) = view.weight(weight_start as usize + k) else {
                        break;
                    };
                    if weight > weights[index] {
                        weights[index] = weight;
                    }
                }
            }
        }

        // 断点 after i letters 对应 dotted 下标 i+1。
        let mut i = LEFT_MIN;
        while i + RIGHT_MIN <= letters.len() {
            if weights[i + 1] % 2 == 1 {
                out.push(i as u16);
            }
            i += 1;
        }
    }

    fn name(&self) -> &str {
        "en-us"
    }
}

/// 二进制表的只读视图。所有越界访问都返回 `None`，坏表不会 panic。
#[derive(Clone, Copy)]
struct View<'a> {
    data: &'a [u8],
    node_count: usize,
    edge_base: usize,
    weight_count: usize,
    weight_base: usize,
}

impl<'a> View<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < HEADER_SIZE || &data[0..4] != b"HYPH" || data[4] != VERSION {
            return None;
        }
        let node_count = read_u32(data, 5)? as usize;
        let edge_count = read_u32(data, 9)? as usize;
        let weight_count = read_u32(data, 13)? as usize;
        let edge_base = HEADER_SIZE.checked_add(node_count.checked_mul(NODE_SIZE)?)?;
        let weight_base = edge_base.checked_add(edge_count.checked_mul(EDGE_SIZE)?)?;
        if weight_base > data.len() {
            return None;
        }
        if weight_count > u16::MAX as usize {
            return None;
        }
        Some(Self {
            data,
            node_count,
            edge_base,
            weight_count,
            weight_base,
        })
    }

    fn node(&self, id: u16) -> Option<(u16, u8, u8, u16)> {
        let id = id as usize;
        if id >= self.node_count {
            return None;
        }
        let at = HEADER_SIZE + id * NODE_SIZE;
        Some((
            read_u16(self.data, at)?,
            *self.data.get(at + 2)?,
            *self.data.get(at + 3)?,
            read_u16(self.data, at + 4)?,
        ))
    }

    /// 按标签找子节点。每个节点最多几十条边，线性扫描比二分更省代码。
    fn child(&self, id: u16, label: u8) -> Option<u16> {
        let (edge_start, edge_len, _, _) = self.node(id)?;
        for index in 0..edge_len as usize {
            let at = self.edge_base + edge_start as usize * EDGE_SIZE + index * EDGE_SIZE;
            if *self.data.get(at)? == label {
                return read_u16(self.data, at + 1);
            }
        }
        None
    }

    fn weight(&self, index: usize) -> Option<u8> {
        if index >= self.weight_count {
            return None;
        }
        self.data.get(self.weight_base + index).copied()
    }

    fn weights(&self, id: u16) -> (u16, u8) {
        match self.node(id) {
            Some((_, _, weight_len, weight_start))
                if weight_start as usize + weight_len as usize <= self.weight_count =>
            {
                (weight_start, weight_len)
            }
            _ => (0, 0),
        }
    }
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    let bytes = data.get(at..at + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaks(word: &str) -> Vec<u16> {
        let mut out = Vec::new();
        English::new().breaks(word, &mut out);
        out
    }

    #[test]
    fn table_header_is_valid() {
        let view = View::parse(DATA).expect("pattern table");
        assert_eq!(view.node_count, 8399);
        assert!(view.weight_base + view.weight_count <= DATA.len());
        assert!(
            view.child(0, b'.').is_some(),
            "root must have a boundary edge"
        );
    }

    #[test]
    fn hyphenates_known_words() {
        // 与 TeX ushyphmax 一致：hy-phen-ation / type-set-ting / com-put-er。
        assert_eq!(breaks("hyphenation"), vec![2, 6]);
        assert_eq!(breaks("typesetting"), vec![4, 7]);
        assert_eq!(breaks("computer"), vec![3]);
    }

    #[test]
    fn respects_hyphenmins_and_short_words() {
        assert!(breaks("the").is_empty());
        assert!(breaks("four").is_empty());
        for word in ["hyphenation", "associate", "algorithm"] {
            for at in breaks(word) {
                assert!(at as usize >= 2, "{word} break {at} violates left min");
                assert!(
                    (word.len() as u16 - at) >= 3,
                    "{word} break {at} violates right min"
                );
            }
        }
    }

    #[test]
    fn rejects_non_letters() {
        assert!(breaks("hello-world").is_empty());
        assert!(breaks("naïve").is_empty());
        assert!(breaks("").is_empty());
    }

    #[test]
    fn case_is_ignored() {
        assert_eq!(breaks("Hyphenation"), breaks("hyphenation"));
    }
}
