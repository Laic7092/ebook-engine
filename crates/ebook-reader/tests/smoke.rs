//! 端到端冒烟测试：打开 → 排版 → 光栅化 → 交互查询。
//!
//! 用 `fixtures/demo.txt`：文本格式和 EPUB 走同一套 spine/TOC/IR 路径，
//! 但没有字体也能跑（字形缺失时 `paint` 画空白页，几何仍然完整）。

use ebook_reader::{Engine, Location, Style, Viewport};

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
