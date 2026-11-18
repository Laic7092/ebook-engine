//! 类 fuzz 回归：对真实样书做确定性的截断、翻字节、搬块、插字节与抹零，断言
//! 格式层只可能 `Ok` 或 `Err`，不会 panic，也不会卡死。
//!
//! 变异器是 xorshift64*，种子固定；失败时打印的 iteration 可以单独复现。
//! 覆盖的入口：`open`（探针 + 容器解析）与 `chapter`（XHTML/CSS 级联）。

use std::panic::{catch_unwind, AssertUnwindSafe};

fn next(state: &mut u64) -> u64 {
    // xorshift64*，确定性，失败可复现。
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

/// 跑一遍 `open` + 前 8 章；返回输入是否被接受。
fn run(bytes: Vec<u8>) -> bool {
    let mut src = ebook_format::MemorySource::new();
    src.insert("fuzz.epub", bytes);
    let Ok(opened) = ebook_format::open(&src) else {
        return false;
    };
    for index in 0..opened.len().min(8) {
        let _ = opened.chapter(&src, index);
    }
    true
}

fn mutate(state: &mut u64, base: &[u8]) -> Vec<u8> {
    let mut bytes = base.to_vec();
    match next(state) % 7 {
        0 => {
            let cut = (next(state) as usize) % bytes.len();
            bytes.truncate(cut);
        }
        1 => {
            let at = (next(state) as usize) % bytes.len();
            bytes[at] ^= 0xff;
        }
        2 => {
            let at = (next(state) as usize) % bytes.len();
            bytes[at] = (next(state) & 0xff) as u8;
        }
        3 => {
            // 头部与尾部各切一刀，留下一个结构更坏的中间段。
            let cut = (next(state) as usize) % (bytes.len() / 2).max(1);
            bytes.drain(..cut);
            let keep = (next(state) as usize) % bytes.len().max(1);
            bytes.truncate(keep);
        }
        4 => {
            // 把一段字节搬到另一处：重复 ZIP 目录结构的常见来源。
            let len = 1 + (next(state) as usize) % (bytes.len() / 8).max(1);
            let from = (next(state) as usize) % (bytes.len() - len);
            let to = (next(state) as usize) % (bytes.len() - len);
            let chunk: Vec<u8> = bytes[from..from + len].to_vec();
            bytes[to..to + len].copy_from_slice(&chunk);
        }
        5 => {
            // 插入一小段垃圾，让长度与偏移同时失真。
            let len = 1 + (next(state) as usize) % 16;
            let at = (next(state) as usize) % bytes.len();
            let junk: Vec<u8> = (0..len).map(|_| (next(state) & 0xff) as u8).collect();
            bytes.splice(at..at, junk);
        }
        _ => {
            // 抹掉一段：连续 0 是 ZIP / XML 解析器最讨厌的输入之一。
            let len = 1 + (next(state) as usize) % (bytes.len() / 4).max(1);
            let at = (next(state) as usize) % (bytes.len() - len);
            bytes[at..at + len].fill(0);
        }
    }
    bytes
}

#[test]
fn thousand_mutations_never_panic() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/en.epub");
    let base = std::fs::read(path).expect("fixtures/en.epub");
    assert!(base.len() > 64);

    // 原样输入必须先打开成功，否则下面的变异没有任何意义。
    let mut pristine = ebook_format::MemorySource::new();
    pristine.insert("fuzz.epub", base.clone());
    let opened = ebook_format::open(&pristine).expect("fixtures/en.epub 应当能打开");
    assert!(!opened.is_empty());

    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut accepted = 0usize;
    for iteration in 0..1000 {
        let bytes = mutate(&mut state, &base);
        match catch_unwind(AssertUnwindSafe(|| run(bytes))) {
            Ok(true) => accepted += 1,
            Ok(false) => {}
            Err(_) => panic!("mutation {iteration} panicked"),
        }
    }
    // 大部分变异应当被拒绝，但也不能把整个语料都拒之门外；这条松散的
    // 覆盖率断言用来抓“变异器其实没改到字节”之类的退化。
    assert!(
        accepted > 10,
        "变异器太温和：只有 {accepted}/1000 个输入被接受"
    );
}
