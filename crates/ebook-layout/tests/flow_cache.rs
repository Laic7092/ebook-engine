//! 前段缓存（[`FlowCache`]）必须只省时间、不改输出。
//!
//! 这一层的正确性只有一条判据：**同一份输入，走缓存与不走缓存的 IR 必须逐字节
//! 相同**。分页依赖页高、断行只依赖宽度，所以「换页高」应当命中，「换宽度」
//! 必须不命中——后者失效会直接把另一条行宽的断行结果当成这一条的。

use ebook_format::{open, MemorySource};
use ebook_ir::{EdgesF, LayoutResult, StyledDoc};
use ebook_layout::{
    layout, FlowCache, Layout, LayoutEnv, LayoutOptions, NoImageSizes, PageConstraint, StubMeasurer,
};

const REV: u64 = 7;

fn chapter() -> StyledDoc {
    let mut src = MemorySource::new();
    src.insert(
        "demo.epub",
        include_bytes!("../../../fixtures/demo.epub").to_vec(),
    );
    let opened = open(&src).expect("open");
    opened.chapter(&src, 0).expect("chapter")
}

fn constraint(width: f32, height: f32) -> PageConstraint {
    PageConstraint::new(width, height).with_margin(EdgesF::all(24.0))
}

/// 不带缓存的同一组输入，用来做逐字节对照。
fn plain(doc: &StyledDoc, c: &PageConstraint, opts: &LayoutOptions) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    Layout::new().layout_with(doc, &env, c, opts)
}

fn cached(
    doc: &StyledDoc,
    c: &PageConstraint,
    opts: &LayoutOptions,
    cache: &mut FlowCache,
    rev: u64,
) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    Layout::new().layout_cached(doc, &env, c, opts, cache, rev)
}

fn assert_bytes_equal(a: &LayoutResult, b: &LayoutResult, what: &str) {
    let left = ebook_ir::codec::to_bytes(a).expect("encode a");
    let right = ebook_ir::codec::to_bytes(b).expect("encode b");
    assert_eq!(
        left, right,
        "{what}: 缓存路径的输出必须与非缓存路径逐字节相同"
    );
}

#[test]
fn cached_output_matches_uncached() {
    let doc = chapter();
    let opts = LayoutOptions::default();
    let c = constraint(390.0, 760.0);
    let mut cache = FlowCache::new();

    let first = cached(&doc, &c, &opts, &mut cache, REV);
    assert_eq!(cache.misses(), 1, "第一次必然未命中");
    assert_bytes_equal(&first, &plain(&doc, &c, &opts), "首次排版");

    // 同宽不同高：分页换了，flow 没换。
    let taller = constraint(390.0, 900.0);
    let second = cached(&doc, &taller, &opts, &mut cache, REV);
    assert_eq!(cache.hits(), 1, "同宽换页高必须命中");
    assert_bytes_equal(&second, &plain(&doc, &taller, &opts), "换页高");
}

#[test]
fn width_change_misses() {
    let doc = chapter();
    let opts = LayoutOptions::default();
    let mut cache = FlowCache::new();

    cached(&doc, &constraint(390.0, 760.0), &opts, &mut cache, REV);
    let wide = constraint(420.0, 760.0);
    let result = cached(&doc, &wide, &opts, &mut cache, REV);
    assert_eq!(cache.misses(), 2, "宽度变了必须重排前段");
    assert_bytes_equal(&result, &plain(&doc, &wide, &opts), "换宽度");
}

#[test]
fn rev_change_misses() {
    let doc = chapter();
    let opts = LayoutOptions::default();
    let c = constraint(390.0, 760.0);
    let mut cache = FlowCache::new();

    cached(&doc, &c, &opts, &mut cache, REV);
    let result = cached(&doc, &c, &opts, &mut cache, REV + 1);
    assert_eq!(cache.misses(), 2, "版本号变了必须重排前段");
    assert_bytes_equal(&result, &plain(&doc, &c, &opts), "换 rev");
}

#[test]
fn line_budget_change_misses() {
    let doc = chapter();
    let c = constraint(390.0, 760.0);
    let mut cache = FlowCache::new();

    let opts = LayoutOptions::default();
    cached(&doc, &c, &opts, &mut cache, REV);

    // 行数预算参与断行（超限时提前停），所以必须进键。
    let tight = LayoutOptions {
        limits: ebook_ir::Limits {
            max_lines: 2,
            ..ebook_ir::Limits::default()
        },
        ..LayoutOptions::default()
    };
    let result = cached(&doc, &c, &tight, &mut cache, REV);
    assert_eq!(cache.misses(), 2, "行数预算变了必须重排前段");
    assert_bytes_equal(&result, &plain(&doc, &c, &tight), "改行数预算");
}

#[test]
fn mode_switch_reuses_the_flow() {
    let doc = chapter();
    let c = constraint(390.0, 760.0);
    let mut cache = FlowCache::new();

    let paged = LayoutOptions::default();
    let first = cached(&doc, &c, &paged, &mut cache, REV);

    // 分页 ↔ 滚动只换分页策略，断行结果完全一样。
    let scroll = LayoutOptions::scroll();
    let second = cached(&doc, &c, &scroll, &mut cache, REV);
    assert_eq!(cache.hits(), 1, "换模式必须命中");
    assert_bytes_equal(&second, &plain(&doc, &c, &scroll), "换模式");
    assert_ne!(
        ebook_ir::codec::to_bytes(&first).unwrap(),
        ebook_ir::codec::to_bytes(&second).unwrap(),
        "两种模式的页确实不同，命中的只是前段"
    );
}

#[test]
fn clear_forces_a_rerun() {
    let doc = chapter();
    let opts = LayoutOptions::default();
    let c = constraint(390.0, 760.0);
    let mut cache = FlowCache::new();

    cached(&doc, &c, &opts, &mut cache, REV);
    cache.clear();
    let result = cached(&doc, &c, &opts, &mut cache, REV);
    assert_eq!(cache.misses(), 2);
    assert_bytes_equal(&result, &plain(&doc, &c, &opts), "清空后");
}

#[test]
fn layout_function_is_still_available() {
    // `layout` 是自由函数，没有缓存入口：它必须继续存在且不带状态。
    let doc = chapter();
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let c = constraint(390.0, 760.0);
    let a = layout(&doc, &env, &c);
    let b = layout(&doc, &env, &c);
    assert_bytes_equal(&a, &b, "无缓存的自由函数");
}
