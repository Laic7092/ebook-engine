//! 连续滚动会话：跨章高度表、全书流坐标取瓦片、跨章命中、行/锚点 → y。
//!
//! 用 `fixtures/demo.epub`（两章，且没有字体时也有边框/插图可看）；
//! 夹具是生成的，干净 checkout 上没有就跳过。

use ebook_reader::{EdgesF, Engine, ScrollChapter, Style, Viewport};

fn epub() -> Option<Vec<u8>> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.epub"
    ))
    .ok()
}

fn txt() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.txt"
    ))
    .unwrap()
}

fn viewport() -> Viewport {
    let mut viewport = Viewport::new(400.0, 700.0);
    viewport.insets = EdgesF {
        top: 0.0,
        right: 24.0,
        bottom: 0.0,
        left: 24.0,
    };
    viewport
}

fn open_epub() -> Option<Engine> {
    let bytes = epub()?;
    Some(Engine::open(bytes, "demo.epub").unwrap())
}

#[test]
fn chapter_tops_and_total_height_are_a_table_the_host_can_use() {
    let Some(mut engine) = open_epub() else {
        eprintln!("fixtures/demo.epub 不在，跳过");
        return;
    };
    assert_eq!(engine.chapter_count(), 2);
    // 还没有会话：滚动入口明确报错，而不是给一个看起来像 0 的高度。
    assert!(engine.scroll_total_height().is_err());
    assert!(engine.scroll_spine_at(0.0).is_err());

    assert_eq!(
        engine.scroll_begin(viewport(), Style::default()).unwrap(),
        2
    );
    // 上下留白被引擎置 0：页坐标与流坐标是同一个坐标系。
    let chapters: Vec<ScrollChapter> = engine.scroll_chapters().unwrap();
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0].spine, 0);
    assert_eq!(chapters[0].top, 0.0);
    assert!(chapters[0].ok, "第 0 章排版失败");
    assert!(chapters[0].height > 0.0, "第 0 章没有高度");
    assert_eq!(chapters[1].top, chapters[0].height);
    assert!(chapters[1].height > 0.0, "第 1 章没有高度");

    let total = engine.scroll_total_height().unwrap();
    assert!((total - (chapters[0].height + chapters[1].height)).abs() < 0.01);

    // 逐章取表与一次取表一致（懒测量的结果不会被重算掉）。
    let one = engine.scroll_chapter(1).unwrap();
    assert_eq!(one.top, chapters[1].top);
    assert_eq!(one.height, chapters[1].height);

    // 全书流坐标 → 章。
    assert_eq!(engine.scroll_spine_at(0.0).unwrap(), Some((0, 0.0)));
    let (spine, top) = engine
        .scroll_spine_at(chapters[1].top + 1.0)
        .unwrap()
        .unwrap();
    assert_eq!(spine, 1);
    assert_eq!(top, chapters[1].top);
    assert_eq!(engine.scroll_spine_at(total + 100.0).unwrap(), None);

    assert!(matches!(
        engine.scroll_chapter(9),
        Err(ebook_reader::EngineError::ChapterOutOfRange(9))
    ));

    // 进度：0 在书首，1 在书尾。
    assert!(engine.scroll_progress(0.0).unwrap() < 1.0e-6);
    assert!((engine.scroll_progress(total).unwrap() - 1.0).abs() < 1.0e-6);
}

#[test]
fn a_slice_can_cross_a_chapter_seam() {
    let Some(mut engine) = open_epub() else {
        eprintln!("fixtures/demo.epub 不在，跳过");
        return;
    };
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    let chapters = engine.scroll_chapters().unwrap();
    let seam = chapters[1].top;

    // 起点取整，让期望值可以按整数行比较（缩放 1 时 y 就是行号）。
    let y = seam.floor() - 40.0;
    let slice = engine.scroll_slice(y, 120.0, 1.0, 0, 0, false).unwrap();
    assert_eq!(slice.width, 400);
    assert_eq!(slice.height, 120);

    // 期望：缝前 40 点来自第 0 章的末尾，缝后 80 点来自第 1 章的开头。
    // 两章都按同样的排版参数单独画，再按行拼起来。
    let style = Style::default();
    let seam_row = ((seam - y) * 1.0).round();
    engine.layout_scroll(0, viewport(), style).unwrap();
    let before = engine
        .bitmap_window(0, y, seam_row, 1.0, 0, 0, false)
        .unwrap();
    engine.layout_scroll(1, viewport(), style).unwrap();
    let after = engine
        .bitmap_window(0, 0.0, 120.0 - seam_row, 1.0, 0, 0, false)
        .unwrap();

    // 缝在切片里的行号：起点对齐到整数行之后，缝前/缝后各占多少行。
    let seam_row = ((seam - y) * 1.0).round() as usize;
    let after_rows = slice.height as usize - seam_row;
    let stride = 400 * 4;
    assert_eq!(before.height as usize, seam_row);
    assert_eq!(after.height as usize, after_rows);
    assert_rows_equal(
        &slice.rgba[..seam_row * stride],
        &before.rgba,
        stride,
        "缝前",
    );
    assert_rows_equal(
        &slice.rgba[seam_row * stride..slice.height as usize * stride],
        &after.rgba,
        stride,
        "缝后",
    );

    // 内容之外仍然是纸色（不是没初始化的内存）。
    let beyond = engine
        .scroll_slice(
            chapters[1].top + chapters[1].height + 500.0,
            60.0,
            1.0,
            0,
            0,
            false,
        )
        .unwrap();
    assert_eq!(beyond.height, 60);
    assert!(beyond
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| *pixel == [255, 255, 255, 255]));
}

/// 短章在滚动列里也占满一屏：章尾是纸色，下一章从整屏边界开始。
#[test]
fn short_chapters_occupy_a_whole_screen() {
    let mut engine = Engine::open(txt(), "demo.txt").unwrap();
    let viewport = viewport();
    engine.scroll_begin(viewport, Style::default()).unwrap();
    let chapters = engine.scroll_chapters().unwrap();
    assert_eq!(chapters.len(), 3, "demo.txt 有三章");
    for chapter in &chapters {
        assert!(
            chapter.ok,
            "第 {} 章排版失败: {:?}",
            chapter.spine,
            engine.scroll_chapter(chapter.spine)
        );
        assert_eq!(
            chapter.height, viewport.height,
            "第 {} 章不足一屏，高度应向上取整",
            chapter.spine
        );
    }
    assert_eq!(engine.scroll_total_height().unwrap(), viewport.height * 3.0);
}

#[test]
fn hits_and_anchors_are_in_book_flow_coordinates() {
    let Some(mut engine) = open_epub() else {
        eprintln!("fixtures/demo.epub 不在，跳过");
        return;
    };
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    let chapters = engine.scroll_chapters().unwrap();

    // 第 1 章中间点一下：命中要带上 spine，y 换成全书流坐标。
    let y = chapters[1].top + 30.0;
    let hit = engine
        .scroll_hit(60.0, y)
        .unwrap()
        .expect("第 1 章应该有行");
    assert_eq!(hit.spine, 1);
    assert!(hit.y >= chapters[1].top);
    assert!(hit.y < chapters[1].top + chapters[1].height);
    // 命中行的 y 与 `scroll_line_y` 一致（同一个坐标系，不差一个章起点）。
    let line_y = engine.scroll_line_y(1, hit.line).unwrap().unwrap();
    assert!((line_y - hit.y).abs() < 0.01, "{line_y} vs {}", hit.y);

    // 第 0 章的锚点：`#note` 是带边框的段落，位置落在第 0 章里。
    let anchor = engine
        .scroll_anchor_y(0, "note")
        .unwrap()
        .expect("note 锚点");
    assert!(anchor >= 0.0 && anchor < chapters[0].height);
    assert_eq!(engine.scroll_anchor_y(0, "not-a-real-id").unwrap(), None);

    // 越界的章号是错误，不是 None。
    assert!(engine.scroll_anchor_y(9, "note").is_err());
}

#[test]
fn a_text_book_scrolls_too() {
    let mut engine = Engine::open(txt(), "demo.txt").unwrap();
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    assert!(engine.chapter_count() >= 1);
    let total = engine.scroll_total_height().unwrap();
    assert!(total > 0.0);
    let tile = engine
        .scroll_slice(total / 3.0, 240.0, 2.0, 0, 0, false)
        .unwrap();
    assert_eq!(tile.width, 800);
    assert_eq!(tile.height, 480);
    assert_eq!(tile.rgba.len(), 800 * 480 * 4);
}

#[test]
fn a_slice_is_deterministic_and_snapped_to_device_pixels() {
    let mut engine = Engine::open(txt(), "demo.txt").unwrap();
    engine.scroll_begin(viewport(), Style::default()).unwrap();
    let first = engine.scroll_slice(123.0, 200.0, 2.0, 0, 0, false).unwrap();
    let again = engine.scroll_slice(123.0, 200.0, 2.0, 0, 0, false).unwrap();
    assert_eq!(first.rgba, again.rgba);
    // 起点对齐到 0.5pt（2x 的一像素）之后，同一个位置与瓦片对齐的位置逐字节相同。
    let aligned = engine.scroll_slice(123.5, 200.0, 2.0, 0, 0, false).unwrap();
    let aligned_again = engine.scroll_slice(123.5, 200.0, 2.0, 0, 0, false).unwrap();
    assert_eq!(aligned.rgba, aligned_again.rgba);
}

fn assert_rows_equal(actual: &[u8], expected: &[u8], stride: usize, label: &str) {
    assert!(
        actual.len() >= expected.len(),
        "{label}: 少了 {} 字节",
        expected.len() - actual.len()
    );
    for row in 0..expected.len() / stride {
        let a = &actual[row * stride..(row + 1) * stride];
        let b = &expected[row * stride..(row + 1) * stride];
        if a != b {
            let column = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(0);
            panic!("{label}: 第 {row} 行（字节 {column}）不一致");
        }
    }
}
