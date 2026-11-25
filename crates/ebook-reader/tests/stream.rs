//! 流式入口：高度索引、惰性实测、章节 append/replace、索引导出/导入。

use ebook_reader::{Engine, Style, Viewport};

fn viewport() -> Viewport {
    Viewport::new(400.0, 700.0).with_margin(24.0)
}

fn paragraph(index: usize) -> String {
    (0..8)
        .map(|line| format!("第 {index} 章第 {line} 段，这是一行用来测量的正文。"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn stream_book(chapters: usize) -> Engine {
    let mut engine = Engine::new_book("流式测试");
    for index in 0..chapters {
        engine
            .append_chapter(&format!("第 {index} 章"), &paragraph(index))
            .unwrap();
    }
    engine
}

#[test]
fn begin_builds_estimates_without_laying_out_any_chapter() {
    let mut engine = stream_book(40);
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    let metrics = engine.scroll_metrics().unwrap();
    assert_eq!(metrics.count, 40);
    assert_eq!(metrics.measured, 0, "scroll_begin 不应该排版任何章");
    assert!(metrics.total > 0.0);

    let table = engine.scroll_chapters_estimated().unwrap();
    assert_eq!(table.len(), 40);
    assert!(table.iter().all(|chapter| chapter.estimated && chapter.ok));
    assert_eq!(table[1].top, table[0].height);
}

#[test]
fn reading_one_chapter_only_measures_that_chapter() {
    let mut engine = stream_book(40);
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    let total_before = engine.scroll_total_height().unwrap();

    let chapter = engine.scroll_chapter(20).unwrap();
    assert!(!chapter.estimated);
    assert!(chapter.height > 0.0);
    let metrics = engine.scroll_metrics().unwrap();
    assert_eq!(metrics.measured, 1, "只应该实测第 20 章");

    // 前面的章仍是估计值，但起点表已经变了。
    let table = engine.scroll_chapters_estimated().unwrap();
    assert!(table[0].estimated);
    assert!(!table[20].estimated);
    assert!(engine.scroll_total_height().unwrap() > 0.0);
    let _ = total_before;
}

#[test]
fn a_band_only_touches_the_chapters_it_covers() {
    let mut engine = stream_book(60);
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    // 先实测第 0 章，让绝对坐标至少在这一章里是真值。
    let first = engine.scroll_chapter(0).unwrap();
    let band = engine
        .scroll_slice(first.top + 10.0, 300.0, 1.0, 0, 0, false)
        .unwrap();
    assert_eq!(band.width, 400);
    assert_eq!(band.height, 300);
    let metrics = engine.scroll_metrics().unwrap();
    assert!(
        metrics.measured <= 3,
        "一枚瓦片不该量到 {} 章",
        metrics.measured
    );
}

#[test]
fn append_and_replace_change_only_the_chapter_rev() {
    let mut engine = stream_book(3);
    let revs: Vec<u64> = (0..3).map(|i| engine.chapter_rev(i).unwrap()).collect();
    assert!(revs.iter().all(|rev| *rev > 0));

    engine.append_chapter("新章", "新的一行\n又一行").unwrap();
    assert_eq!(engine.chapter_count(), 4);
    assert!(engine.chapter_rev(3).unwrap() > 0);
    assert_eq!(engine.chapter_rev(0), Some(revs[0]));

    engine
        .replace_chapter(1, "改过的章", "替换后的正文\n第二段")
        .unwrap();
    assert_eq!(engine.chapter_count(), 4);
    let replaced = engine.chapter_rev(1).unwrap();
    assert_ne!(replaced, revs[1]);
    assert_eq!(engine.chapter_rev(2), Some(revs[2]), "其它章版本不变");
    assert!(engine.chapter_rev(9).is_none());
    assert!(engine.replace_chapter(9, "越界", "正文").is_err());
    assert!(engine.append_chapter("空章", "").is_err());
}

#[test]
fn height_index_round_trips_through_a_new_session() {
    let mut engine = stream_book(12);
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    let measured = engine.scroll_chapter(4).unwrap();
    let bytes = engine.scroll_index_export().unwrap();

    // 同一本书重新打开：指纹、视口、样式一致，导入后精度回来。
    let mut again = stream_book(12);
    again.scroll_begin(viewport(), Style::default()).unwrap();
    assert!(again.scroll_index_import(&bytes).unwrap());
    let restored = again.scroll_chapter(4).unwrap();
    assert_eq!(restored.height, measured.height);
    assert_eq!(again.scroll_metrics().unwrap().measured, 1);

    // 视口不同 => 键不同 => 拒绝导入，不串高度。
    let mut other = stream_book(12);
    other
        .scroll_begin(
            Viewport::new(500.0, 700.0).with_margin(24.0),
            Style::default(),
        )
        .unwrap();
    assert!(!other.scroll_index_import(&bytes).unwrap());
}

#[test]
fn replace_invalidates_the_measured_height_but_keeps_the_session() {
    let mut engine = stream_book(6);
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    engine.scroll_chapter(2).unwrap();
    assert_eq!(engine.scroll_metrics().unwrap().measured, 1);

    engine.replace_chapter(2, "改过", "短").unwrap();
    let metrics = engine.scroll_metrics().unwrap();
    assert_eq!(metrics.measured, 0, "替换后该章高度退回估计");
    let chapter = engine.scroll_chapters_estimated().unwrap()[2];
    assert!(chapter.estimated);
}

#[test]
fn a_cleared_stream_book_still_answers_without_panicking() {
    let mut engine = stream_book(2);
    engine.clear_chapters().unwrap();
    assert_eq!(engine.chapter_count(), 0);
    assert!(engine.scroll_begin(viewport(), Style::default()).is_err());
    assert!(engine.chapter_rev(0).is_none());
}
