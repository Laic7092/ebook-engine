//! 端到端冒烟测试：打开 → 排版 → 光栅化 → 交互查询。
//!
//! 用 `fixtures/demo.txt`：文本格式和 EPUB 走同一套 spine/TOC/IR 路径，
//! 但没有字体也能跑（字形缺失时 `paint` 画空白页，几何仍然完整）。

use ebook_reader::{EdgesF, Engine, Location, Style, Viewport};

fn demo() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/demo.txt"
    ))
    .unwrap()
}

fn open() -> Engine {
    let mut engine = Engine::open(demo(), "demo.txt").unwrap();
    engine
        .layout(
            0,
            Viewport::new(400.0, 700.0).with_margin(24.0),
            Style::default(),
        )
        .unwrap();
    engine
}

#[test]
fn opens_text_and_lays_out() {
    let engine = open();
    assert!(engine.chapter_count() > 0);
    assert!(!engine.pages().is_empty());
    assert_eq!(engine.spine(), Some(0));
    assert_eq!(engine.page_count() as usize, engine.pages().len());
}

#[test]
fn page_boxes_cover_the_viewport() {
    let engine = open();
    for page in engine.pages() {
        // `PageBox` 是**内容盒**（视口减掉内边距），`inset` 单独给出四边。
        assert!((page.width - (400.0 - 48.0)).abs() < 0.5, "{:?}", page);
        assert!((page.height - (700.0 - 48.0)).abs() < 0.5, "{:?}", page);
        assert_eq!(page.inset.left, 24.0);
        assert!(page.line_len > 0);
        assert!((0.0..=1.0).contains(&page.progress));
    }
    // 本章进度单调不减。
    let pages = engine.pages();
    for pair in pages.windows(2) {
        assert!(pair[1].progress >= pair[0].progress);
    }
}

#[test]
fn bitmap_matches_requested_scale() {
    let mut engine = open();
    let bitmap = engine.bitmap(0, 2.0, 0, 0, false).unwrap();
    assert_eq!(bitmap.width, 800);
    assert_eq!(bitmap.height, 1400);
    assert_eq!(bitmap.rgba.len(), 800 * 1400 * 4);
    // 越界页号是错误，不是空位图：宿主能区分"没这页"和"画失败"。
    assert!(engine.bitmap(9999, 1.0, 0, 0, false).is_err());
}

#[test]
fn hit_and_progress_agree() {
    let engine = open();
    let hit = engine.hit(0, 60.0, 40.0).expect("页面左上角应该在正文里");
    assert_eq!(hit.page, 0);
    let progress = engine.progress(hit).unwrap();
    assert!((0.0..=1.0).contains(&progress));
}

#[test]
fn selection_normalizes() {
    let engine = open();
    let a = Location {
        page: 0,
        line: 0,
        byte: 0,
    };
    let b = engine
        .hit(0, 200.0, 120.0)
        .map(|hit| Location {
            line: hit.line,
            byte: hit.byte + 3,
            ..hit
        })
        .unwrap();
    let sel = engine.selection(b, a).unwrap();
    assert!(sel.start.line <= sel.end.line);
    assert!(!sel.text.is_empty());
    assert!(!sel.rects.is_empty());
}

#[test]
fn style_changes_repaginate_without_touching_geometry() {
    let mut engine = open();
    let before = engine.page_count();
    engine
        .layout(
            0,
            Viewport::new(400.0, 700.0).with_margin(24.0),
            Style {
                font_scale: 1.6,
                line_scale: 1.5,
                ..Style::default()
            },
        )
        .unwrap();
    let after = engine.page_count();
    assert!(after >= before, "{before} -> {after}");
    for page in engine.pages() {
        assert!((page.width - (400.0 - 48.0)).abs() < 0.5);
        assert_eq!(page.inset.left, 24.0);
    }
}

#[test]
fn scroll_mode_is_one_tall_page() {
    let mut engine = Engine::open(demo(), "demo.txt").unwrap();
    let mut viewport = Viewport::new(400.0, 700.0);
    // 滚动模式：上下留白交给宿主，左右留白留给页。
    viewport.insets = EdgesF {
        top: 0.0,
        right: 24.0,
        bottom: 0.0,
        left: 24.0,
    };
    engine.layout_scroll(0, viewport, Style::default()).unwrap();
    assert!(engine.is_scroll());
    assert_eq!(engine.page_count(), 1);
    let height = engine.content_height();
    assert!(height > 0.0, "content height {height}");

    let top = engine
        .bitmap_window(0, 0.0, 120.0, 1.0, 0, 0, false)
        .unwrap();
    assert_eq!(top.width, 400);
    assert_eq!(top.height, 120);
    assert_eq!(top.rgba.len(), 400 * 120 * 4);
    let bottom = engine
        .bitmap_window(0, (height - 120.0).max(0.0), 120.0, 1.0, 0, 0, false)
        .unwrap();
    assert_eq!(bottom.width, 400);
    assert_eq!(bottom.height, 120);

    // 窗口必须是正的；越界页号仍然是错误。
    assert!(engine.bitmap_window(0, 0.0, 0.0, 1.0, 0, 0, false).is_err());
    assert!(engine
        .bitmap_window(9, 0.0, 100.0, 1.0, 0, 0, false)
        .is_err());

    // 命中仍是页内坐标：滚动页的 inset.top 是 0，所以 y 就是流坐标。
    let hit = engine.hit(0, 60.0, 40.0).expect("第一行");
    assert_eq!(hit.page, 0);
    let progress = engine.progress(hit).unwrap();
    assert!((0.0..=1.0).contains(&progress));
    let later = engine.hit(0, 60.0, height / 2.0).expect("中间一行");
    assert!(engine.progress(later).unwrap() >= progress);
}

#[test]
fn scroll_tiles_render_on_independent_engines() {
    // 一个 `Engine` 是 `&mut`，但书和排版没有全局状态：多个引擎可以同时
    // 各自排一章、各自取瓦片。
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                let mut engine = Engine::open(demo(), "demo.txt").unwrap();
                let mut viewport = Viewport::new(400.0, 700.0);
                viewport.insets = EdgesF {
                    top: 0.0,
                    right: 24.0,
                    bottom: 0.0,
                    left: 24.0,
                };
                engine.layout_scroll(0, viewport, Style::default()).unwrap();
                let height = engine.content_height();
                let tile = engine
                    .bitmap_window(0, height / 3.0, 240.0, 1.0, 0, 0, false)
                    .unwrap();
                (height, tile.width, tile.height, tile.rgba.len())
            })
        })
        .collect();
    for handle in handles {
        let (height, width, tile_height, bytes) = handle.join().unwrap();
        assert!(height > 0.0);
        assert_eq!(width, 400);
        assert_eq!(tile_height, 240);
        assert_eq!(bytes, 400 * 240 * 4);
    }
}

#[test]
fn engines_are_independent() {
    // 没有全局单例：多本书可以在多个线程里同时打开、排版。
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                let engine = open();
                assert!(engine.page_count() > 0);
                engine.page_count()
            })
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap() > 0);
    }
}

/// 换书：旧引擎 drop 之后，字节由它自己释放，不再泄漏。
#[test]
fn reopening_does_not_accumulate() {
    for _ in 0..8 {
        let engine = open();
        assert!(!engine.pages().is_empty());
    }
}

#[test]
fn memory_usage_drops_reclaimable_caches_on_trim() {
    let mut engine = open();
    engine.bitmap(0, 1.0, 0, 0, false).unwrap();
    let before = engine.memory_usage();
    assert!(before.total_bytes > 0, "{before:?}");
    engine.trim_memory();
    let after = engine.memory_usage();
    assert!(
        after.total_bytes <= before.total_bytes,
        "{before:?} -> {after:?}"
    );
    // 当前页 IR 还在，trim 之后仍能继续画。
    assert!(engine.bitmap(0, 1.0, 0, 0, false).is_ok());
}
