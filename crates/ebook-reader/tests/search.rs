//! 全文搜索：索引、命中上限、跳转定位、流式书失效。

use ebook_reader::{Engine, Location, Style, Viewport};

fn demo() -> Engine {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.txt"
    ))
    .unwrap();
    Engine::open(bytes, "demo.txt").unwrap()
}

fn viewport() -> Viewport {
    Viewport::new(400.0, 700.0).with_margin(24.0)
}

#[test]
fn finds_across_the_whole_book() {
    let mut engine = demo();
    let results = engine.search("雪", 50);
    assert!(!results.hits.is_empty(), "demo.txt 里应该有“雪”");
    assert!(results.hits.len() <= 50);
    assert!(results.scanned > 0);
    assert_eq!(results.skipped, 0);

    // 命中位置要能落到排版后的页/行，而且就是命中那一行：段落会折行，
    // 落进段落的最后一行不算定位到了。
    let hit = &results.hits[0];
    engine
        .layout(hit.spine, viewport(), Style::default())
        .unwrap();
    let location = engine.locate_locator(&hit.range.start);
    assert!(location.is_some(), "搜索命中必须能定位: {hit:?}");
    let location = location.unwrap();
    assert!(location.page < engine.page_count());
    let line_start = Location {
        page: location.page,
        line: location.line,
        byte: 0,
    };
    let line_end = Location {
        page: location.page,
        line: location.line,
        byte: u32::MAX,
    };
    let text = engine.selection(line_start, line_end).unwrap().text;
    assert!(
        text.contains(hit.text.as_str()),
        "命中应落在这一行: 行是 {text:?}，命中是 {:?}",
        hit.text
    );
}

#[test]
fn matches_phrase_inside_a_paragraph() {
    let mut engine = demo();
    let results = engine.search("沈砚背着书箱", 10);
    assert_eq!(results.hits.len(), 1, "{:?}", results.hits);
    assert!(results.hits[0].text.contains("沈砚背着书箱"));
    assert_eq!(results.hits[0].range.start.spine, 0);
}

#[test]
fn caps_hits_and_reports_truncation() {
    let mut engine = demo();
    let results = engine.search("的", 3);
    assert_eq!(results.hits.len(), 3);
    assert!(results.truncated);
}

#[test]
fn empty_query_returns_nothing() {
    let mut engine = demo();
    let results = engine.search("   ", 10);
    assert!(results.hits.is_empty());
    assert_eq!(results.scanned, 0);
    assert!(!results.truncated);
}

#[test]
fn stream_book_search_sees_replacements() {
    let mut engine = Engine::new_book("stories");
    engine.append_chapter("一", "alpha beta").unwrap();
    engine.append_chapter("二", "gamma alpha").unwrap();
    assert_eq!(engine.search("alpha", 10).hits.len(), 2);

    engine.replace_chapter(0, "一", "delta").unwrap();
    let results = engine.search("alpha", 10);
    assert_eq!(results.hits.len(), 1);
    assert_eq!(results.hits[0].spine, 1);
    assert_eq!(engine.search("delta", 10).hits.len(), 1);
}

#[test]
fn cache_is_reused_without_changing_results() {
    let mut engine = demo();
    let first = engine.search("灯", 20);
    let second = engine.search("灯", 20);
    assert_eq!(first, second);
    assert!(engine.memory_usage().search_bytes > 0);
    engine.trim_memory();
    assert_eq!(engine.memory_usage().search_bytes, 0);
}
