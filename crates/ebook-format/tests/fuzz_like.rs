//! 类 fuzz 回归：对真实样书做确定性的截断与翻字节，断言只可能 `Ok` 或 `Err`，
//! 不会 panic，也不会卡死。

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

fn run(bytes: Vec<u8>) {
    let mut src = ebook_format::MemorySource::new();
    src.insert("fuzz.epub", bytes);
    if let Ok(opened) = ebook_format::open(&src) {
        for index in 0..opened.len().min(8) {
            let _ = opened.chapter(&src, index);
        }
    }
}

#[test]
fn two_hundred_mutations_never_panic() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/en.epub");
    let base = std::fs::read(path).expect("fixtures/en.epub");
    assert!(base.len() > 64);
    let mut state = 0x9e37_79b9_7f4a_7c15u64;

    for iteration in 0..200 {
        let mut bytes = base.clone();
        match next(&mut state) % 4 {
            0 => {
                let cut = (next(&mut state) as usize) % bytes.len();
                bytes.truncate(cut);
            }
            1 => {
                let at = (next(&mut state) as usize) % bytes.len();
                bytes[at] ^= 0xff;
            }
            2 => {
                let at = (next(&mut state) as usize) % bytes.len();
                bytes[at] = (next(&mut state) & 0xff) as u8;
            }
            _ => {
                // 头部与尾部各切一刀，留下一个结构更坏的中间段。
                let cut = (next(&mut state) as usize) % (bytes.len() / 2).max(1);
                bytes.drain(..cut);
                let keep = (next(&mut state) as usize) % bytes.len().max(1);
                bytes.truncate(keep);
            }
        }

        let result = catch_unwind(AssertUnwindSafe(|| run(bytes)));
        assert!(result.is_ok(), "mutation {iteration} panicked");
    }
}
