//! 前段缓存接进 `Engine` 之后的两条硬约束：
//!
//! 1. 同宽不同高（地址栏收起、旋屏）只重跑分页，IR 仍与「新引擎从头排」逐字节相同；
//! 2. 换字号、换书、换字体必须失效——缓存命中错一次就是整章错行。

use ebook_ir::codec;
use ebook_reader::{Engine, Style, Viewport};

fn demo() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.txt"
    ))
    .unwrap()
}

fn viewport(height: f32) -> Viewport {
    Viewport::new(400.0, height).with_margin(24.0)
}

fn bytes(engine: &Engine) -> Vec<u8> {
    codec::to_bytes(engine.ir().expect("ir")).expect("encode")
}

#[test]
fn height_only_change_hits_the_flow_cache() {
    let mut engine = Engine::open(demo(), "demo.txt").unwrap();
    engine.layout(0, viewport(700.0), Style::default()).unwrap();
    let (hits, misses) = engine.flow_cache_stats();
    assert_eq!((hits, misses), (0, 1), "第一次必然重排前段");

    // 同宽、只换页高：断行结果完全一样，只有分页要重算。
    engine.layout(0, viewport(500.0), Style::default()).unwrap();
    let (hits, misses) = engine.flow_cache_stats();
    assert_eq!((hits, misses), (1, 1), "换页高必须命中前段缓存");

    // 与「新建一个引擎直接排同样的输入」逐字节对比。
    let mut fresh = Engine::open(demo(), "demo.txt").unwrap();
    fresh.layout(0, viewport(500.0), Style::default()).unwrap();
    assert_eq!(
        bytes(&engine),
        bytes(&fresh),
        "命中缓存的 IR 必须逐字节相同"
    );
}

#[test]
fn width_change_misses() {
    let mut engine = Engine::open(demo(), "demo.txt").unwrap();
    engine.layout(0, viewport(700.0), Style::default()).unwrap();
    engine
        .layout(
            0,
            Viewport::new(320.0, 700.0).with_margin(24.0),
            Style::default(),
        )
        .unwrap();
    let (hits, misses) = engine.flow_cache_stats();
    assert_eq!((hits, misses), (0, 2), "换宽度必须重排前段");
}

#[test]
fn style_change_misses() {
    let mut engine = Engine::open(demo(), "demo.txt").unwrap();
    engine.layout(0, viewport(700.0), Style::default()).unwrap();
    let bigger = Style {
        font_scale: 1.4,
        ..Style::default()
    };
    engine.layout(0, viewport(700.0), bigger).unwrap();
    let (hits, misses) = engine.flow_cache_stats();
    assert_eq!((hits, misses), (0, 2), "换字号必须重排前段");

    // 同样字号再换页高：仍然只重跑分页。
    engine.layout(0, viewport(500.0), bigger).unwrap();
    let (hits, misses) = engine.flow_cache_stats();
    assert_eq!((hits, misses), (1, 2));
}

#[test]
fn page_and_scroll_modes_share_the_flow() {
    let mut engine = Engine::open(demo(), "demo.txt").unwrap();
    engine.layout(0, viewport(700.0), Style::default()).unwrap();
    engine
        .layout_scroll(0, viewport(700.0), Style::default())
        .unwrap();
    let (hits, misses) = engine.flow_cache_stats();
    assert_eq!((hits, misses), (1, 1), "分页与滚动共用同一份断行");

    let mut fresh = Engine::open(demo(), "demo.txt").unwrap();
    fresh
        .layout_scroll(0, viewport(700.0), Style::default())
        .unwrap();
    assert_eq!(bytes(&engine), bytes(&fresh), "滚动模式的 IR 仍逐字节相同");
}
