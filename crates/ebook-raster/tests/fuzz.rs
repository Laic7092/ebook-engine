//! 光栅 fuzz：随机的 rect / glyph run / image / 嵌套 clip，断言不 panic、
//! 不画到画布外、结果确定。
//!
//! 光栅层是纯函数：同一份 IR + 同一份资源，两次渲染必须逐字节相同。
//! 这里用固定种子生成 1000 页随机 op，坐标刻意包含负数、NaN 之外的非有限值
//! 不进 IR（排版层不会产出），只放极端但有限的坐标，让裁剪与取整逻辑全走一遍。

use ebook_ir::{
    Border, BorderBox, Dir, DisplayList, EdgesF, Fit, FontId, Glyph, GlyphBitmap, GlyphRun,
    ImageData, ImageId, IrVersion, LayoutResult, Op, Page, PointF, RectF, RenderError, Resources,
    Rgba8, Rotate, RunMap, SizeF, Span, Unresolved, UnresolvedReason, IR_VERSION,
};
use ebook_raster::RgbaSurface;

struct Res {
    glyph: GlyphBitmap,
    image: ImageData,
}

impl Default for Res {
    fn default() -> Self {
        Self {
            glyph: GlyphBitmap {
                w: 4,
                h: 4,
                left: -1,
                top: -4,
                alpha: vec![
                    0, 64, 128, 255, 32, 96, 160, 224, 16, 48, 80, 112, 8, 24, 40, 56,
                ],
            },
            image: ImageData {
                w: 2,
                h: 2,
                rgba: vec![
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
                ],
            },
        }
    }
}

impl Resources for Res {
    fn glyph_alpha(&mut self, _font: FontId, _gid: u16, _px: f32) -> Option<GlyphBitmap> {
        Some(self.glyph.clone())
    }

    fn image(&mut self, _id: ImageId) -> Option<ImageData> {
        Some(self.image.clone())
    }
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(0x5851_f42d_4c95_7f2d)
            .wrapping_add(0x1405_7b7e_f767_814f);
        self.0 >> 17
    }

    fn below(&mut self, n: u32) -> u32 {
        assert!(n > 0);
        (self.next() % n as u64) as u32
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 10_007) as f32 / 10_007.0
    }

    fn coord(&mut self, span: f32) -> f32 {
        // 大半落在画布附近，少数远在画布外（含负数），让裁剪与取整都被压测。
        if self.below(8) == 0 {
            -span + self.unit() * span * 4.0
        } else {
            self.unit() * span
        }
    }

    fn rect(&mut self, span: f32) -> RectF {
        let x = self.coord(span);
        let y = self.coord(span);
        let w = if self.below(10) == 0 {
            0.0
        } else {
            self.unit() * span
        };
        let h = if self.below(10) == 0 {
            0.0
        } else {
            self.unit() * span
        };
        RectF::new(x, y, w, h)
    }

    fn color(&mut self) -> Rgba8 {
        Rgba8::new(
            (self.next() & 0xff) as u8,
            (self.next() & 0xff) as u8,
            (self.next() & 0xff) as u8,
            (self.next() & 0xff) as u8,
        )
    }
}

fn page_ir(width: f32, height: f32, ops: Vec<Op>, glyphs: Vec<Glyph>) -> LayoutResult {
    let count = ops.len() as u32;
    LayoutResult {
        version: IR_VERSION,
        spine: 0,
        pages: vec![Page {
            index: 0,
            origin: PointF::new(0.0, 0.0),
            size: SizeF::new(width, height),
            inset: EdgesF::ZERO,
            ops: Span::new(0, count),
            lines: Span::EMPTY,
            anchor: None,
        }],
        display: DisplayList { ops, glyphs },
        runs: RunMap::default(),
        dirty: Vec::new(),
        issues: Vec::new(),
        truncated: false,
    }
}

/// 生成一段随机的 op 列表；`depth` 控制 `Clip` 的嵌套层数。
fn ops(rng: &mut Rng, span: f32, glyphs: &mut Vec<Glyph>, depth: u32) -> Vec<Op> {
    let mut out = Vec::new();
    let count = 1 + rng.below(4);
    for _ in 0..count {
        match rng.below(8) {
            0 => out.push(Op::Rect {
                rect: rng.rect(span),
                color: rng.color(),
            }),
            1 => out.push(Op::Border(BorderBox {
                rect: rng.rect(span),
                width: ebook_ir::SidesF::all(rng.unit() * 3.0),
                color: rng.color(),
            })),
            2 => {
                let start = glyphs.len() as u32;
                let n = rng.below(4);
                for _ in 0..n {
                    glyphs.push(Glyph {
                        gid: rng.below(64) as u16,
                        dx: rng.unit() * 6.0 - 1.0,
                        dy: rng.unit() * 4.0 - 2.0,
                        ch: rng.below(16),
                    });
                }
                out.push(Op::GlyphRun(GlyphRun {
                    font: FontId(0),
                    size: 6.0 + rng.unit() * 10.0,
                    color: rng.color(),
                    origin: PointF::new(rng.coord(span), rng.coord(span)),
                    glyphs: Span::new(start, n),
                    text: String::from("ab"),
                    dir: if rng.below(2) == 0 {
                        Dir::Ltr
                    } else {
                        Dir::Rtl
                    },
                    rotate: Rotate::None,
                }));
            }
            3 => out.push(Op::Image {
                id: rng.below(4) as ImageId,
                rect: rng.rect(span),
                fit: match rng.below(3) {
                    0 => Fit::Contain,
                    1 => Fit::Cover,
                    _ => Fit::Fill,
                },
            }),
            4 => out.push(Op::Unresolved(Unresolved {
                node: 0,
                rect: rng.rect(span),
                reason: UnresolvedReason::ImagePending,
            })),
            5 => out.push(Op::Link {
                rect: rng.rect(span),
                href: String::from("https://example.com/x"),
            }),
            6 if depth < 2 => {
                let inner = ops(rng, span, glyphs, depth + 1);
                out.push(Op::Clip {
                    rect: rng.rect(span),
                    ops: inner,
                });
            }
            _ => out.push(Op::Rect {
                rect: RectF::new(-span, -span, span * 2.0, span * 2.0),
                color: rng.color(),
            }),
        }
    }
    out
}

fn render(ir: &LayoutResult) -> RgbaSurface {
    let page = ir.page(0).expect("page");
    let mut surface = RgbaSurface::from_page(page);
    let mut res = Res::default();
    surface.render_page(ir, 0, &mut res).expect("render");
    surface
}

#[test]
fn thousand_random_pages_render_deterministically() {
    for seed in 1..=1000u64 {
        let mut rng = Rng::new(seed);
        let width = 1.0 + rng.unit() * 72.0;
        let height = 1.0 + rng.unit() * 72.0;
        let span = width.max(height);
        let mut glyphs = Vec::new();
        let list = ops(&mut rng, span, &mut glyphs, 0);
        let ir = page_ir(width, height, list, glyphs);

        let first = render(&ir);
        let second = render(&ir);

        assert_eq!(first.width(), second.width(), "seed {seed}: 画布宽变了");
        assert_eq!(first.height(), second.height(), "seed {seed}: 画布高变了");
        assert_eq!(
            first.pixels(),
            second.pixels(),
            "seed {seed}: 两次渲染不是逐像素相同"
        );
        assert_eq!(
            first.pixels().len(),
            first.width() as usize * first.height() as usize,
            "seed {seed}: 像素缓冲区大小与画布不一致"
        );
        // 任何混合都不应把不透明画布弄成半透明；透明像素会让 PPM/像素基线失真。
        for (index, pixel) in first.pixels().iter().enumerate() {
            assert_eq!(pixel.a, 255, "seed {seed}: 像素 {index} 的 alpha 变了");
        }
        // 越界查询必须安全返回 None，而不是读到相邻行。
        assert!(first.pixel(first.width(), 0).is_none());
        assert!(first.pixel(0, first.height()).is_none());
    }
}

#[test]
fn off_canvas_ops_leave_the_surface_untouched() {
    let glyphs = vec![Glyph {
        gid: 1,
        dx: 2.0,
        dy: 0.0,
        ch: 0,
    }];
    let off = RectF::new(-1000.0, -1000.0, 10.0, 10.0);
    let far = RectF::new(1000.0, 1000.0, 10.0, 10.0);
    let list = vec![
        Op::Rect {
            rect: off,
            color: Rgba8::new(255, 0, 0, 255),
        },
        Op::Rect {
            rect: far,
            color: Rgba8::new(0, 255, 0, 255),
        },
        Op::Border(BorderBox {
            rect: far,
            width: ebook_ir::SidesF::all(2.0),
            color: Rgba8::new(0, 0, 255, 255),
        }),
        Op::GlyphRun(GlyphRun {
            font: FontId(0),
            size: 12.0,
            color: Rgba8::BLACK,
            origin: PointF::new(-500.0, -500.0),
            glyphs: Span::new(0, 1),
            text: String::from("a"),
            dir: Dir::Ltr,
            rotate: Rotate::None,
        }),
        Op::Image {
            id: 0,
            rect: off,
            fit: Fit::Contain,
        },
        Op::Unresolved(Unresolved {
            node: 0,
            rect: far,
            reason: UnresolvedReason::SizeFromContent,
        }),
        // 裁剪框在画布外，内部却有一个覆盖整屏的矩形：交集为空，一个像素都不该画。
        Op::Clip {
            rect: RectF::new(500.0, 500.0, 10.0, 10.0),
            ops: vec![Op::Rect {
                rect: RectF::new(0.0, 0.0, 8.0, 8.0),
                color: Rgba8::new(255, 0, 255, 255),
            }],
        },
    ];
    let ir = page_ir(8.0, 8.0, list, glyphs);
    let surface = render(&ir);
    for pixel in surface.pixels() {
        assert_eq!(*pixel, Rgba8::WHITE, "画布外的 op 改了像素");
    }
}

#[test]
fn clip_bounds_every_write() {
    // 整屏红底，再在 (5..15)² 里用裁剪画整屏蓝：裁剪外保持红，裁剪内变蓝。
    let red = Rgba8::new(255, 0, 0, 255);
    let blue = Rgba8::new(0, 0, 255, 255);
    let list = vec![
        Op::Rect {
            rect: RectF::new(0.0, 0.0, 20.0, 20.0),
            color: red,
        },
        Op::Clip {
            rect: RectF::new(5.0, 5.0, 10.0, 10.0),
            ops: vec![Op::Rect {
                rect: RectF::new(0.0, 0.0, 20.0, 20.0),
                color: blue,
            }],
        },
    ];
    let ir = page_ir(20.0, 20.0, list, Vec::new());
    let surface = render(&ir);
    for y in 0..20u32 {
        for x in 0..20u32 {
            let want = if (5..15).contains(&x) && (5..15).contains(&y) {
                blue
            } else {
                red
            };
            assert_eq!(surface.pixel(x, y), Some(want), "像素 ({x}, {y})");
        }
    }
}

#[test]
fn missing_page_is_an_error_not_a_panic() {
    let ir = page_ir(4.0, 4.0, Vec::new(), Vec::new());
    let mut surface = RgbaSurface::new(4, 4);
    let mut res = Res::default();
    assert_eq!(
        surface.render_page(&ir, 7, &mut res),
        Err(RenderError::NoSuchPage(7))
    );
    // 失败不写半页：清屏后的画布仍是白色。
    for pixel in surface.pixels() {
        assert_eq!(*pixel, Rgba8::WHITE);
    }
    let _ = (Border::default(), IrVersion { major: 1, minor: 3 });
}
