use ebook_format::xhtml::{build, NoAssets};
use ebook_ir::{
    BorderBox, Dir, EdgesF, Fit, FontId, Glyph, GlyphBitmap, GlyphRun, ImageData, ImageId,
    LayoutResult, Op, PointF, RectF, Renderer, Resources, Rgba8, Rotate, SidesF, Span, StyledDoc,
    Unresolved, UnresolvedReason,
};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
use ebook_raster::RgbaSurface;

struct FixedResources {
    glyph: GlyphBitmap,
    image: ImageData,
}

impl FixedResources {
    fn new() -> Self {
        Self {
            glyph: GlyphBitmap {
                w: 8,
                h: 8,
                left: 0,
                top: -8,
                alpha: vec![128; 64],
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

impl Resources for FixedResources {
    fn glyph_alpha(&mut self, _font: FontId, _gid: u16, _px: f32) -> Option<GlyphBitmap> {
        Some(self.glyph.clone())
    }

    fn image(&mut self, _id: ImageId) -> Option<ImageData> {
        Some(self.image.clone())
    }
}

fn document_ir(text: &str) -> LayoutResult {
    let html = format!(
        "<html><head><style>body{{margin:0}}p{{margin:0;font-size:16px;line-height:24px}}</style></head><body><p>{text}</p></body></html>"
    );
    let doc: StyledDoc = build(&html, 0, &mut NoAssets).expect("xhtml");
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(200.0, 200.0).with_margin(EdgesF::all(0.0));
    layout(&doc, &env, &constraint)
}

fn append_op(ir: &mut LayoutResult, op: Op) {
    ir.display.ops.push(op);
    if let Some(page) = ir.pages.last_mut() {
        page.ops.len += 1;
    }
}

fn first_run(ir: &LayoutResult) -> GlyphRun {
    ir.display
        .ops
        .iter()
        .find_map(|op| match op {
            Op::GlyphRun(run) => Some(run.clone()),
            _ => None,
        })
        .expect("glyph run")
}

fn render(ir: &LayoutResult, surface: &mut RgbaSurface) {
    let mut res = FixedResources::new();
    surface.render_page(ir, 0, &mut res).expect("render");
}

#[test]
fn renderer_fills_target_white() {
    let ir = document_ir("A");
    let mut target = RgbaSurface::new(200, 200);
    target.clear(Rgba8::BLACK);
    let mut renderer = RgbaSurface::new(200, 200);
    let mut res = FixedResources::new();
    renderer
        .render(&ir, 0, &mut target, &mut res)
        .expect("render");
    assert_eq!(target.pixel(199, 199), Some(Rgba8::WHITE));
    assert!(target.pixels().iter().any(|pixel| *pixel != Rgba8::WHITE));
}

#[test]
fn rect_color_is_exact_and_render_is_deterministic() {
    let mut ir = document_ir("A");
    let color = Rgba8::new(17, 34, 51, 255);
    append_op(
        &mut ir,
        Op::Rect {
            rect: RectF::new(20.0, 40.0, 30.0, 20.0),
            color,
        },
    );

    let mut first = RgbaSurface::new(200, 200);
    render(&ir, &mut first);
    assert_eq!(first.pixel(20, 40), Some(color));
    assert_eq!(first.pixel(35, 50), Some(color));
    assert_eq!(first.pixel(49, 59), Some(color));
    assert_eq!(first.pixel(19, 40), Some(Rgba8::WHITE));
    assert_eq!(first.pixel(50, 59), Some(Rgba8::WHITE));

    let mut second = RgbaSurface::new(200, 200);
    render(&ir, &mut second);
    assert_eq!(first.to_ppm(), second.to_ppm());
    assert_eq!(first.pixels(), second.pixels());
}

#[test]
fn glyph_run_draws_ink_from_resources() {
    let ir = document_ir("AB");
    let run = first_run(&ir);
    let mut surface = RgbaSurface::new(200, 200);
    render(&ir, &mut surface);

    let ink = surface
        .pixels()
        .iter()
        .filter(|pixel| **pixel != Rgba8::WHITE)
        .count();
    assert!(ink > 0);

    let x = run.origin.x.floor() as u32 + 4;
    let y = (run.origin.y + -8.0).floor() as u32 + 4;
    assert_eq!(surface.pixel(x, y), Some(Rgba8::new(127, 127, 127, 255)));
}

#[test]
fn border_and_image_are_painted() {
    let mut ir = document_ir("A");
    let border_color = Rgba8::new(200, 10, 20, 255);
    append_op(
        &mut ir,
        Op::Border(BorderBox {
            rect: RectF::new(20.0, 40.0, 20.0, 20.0),
            width: SidesF::all(3.0),
            color: border_color,
        }),
    );
    append_op(
        &mut ir,
        Op::Image {
            id: 0,
            rect: RectF::new(60.0, 40.0, 8.0, 8.0),
            fit: Fit::Fill,
        },
    );

    let mut surface = RgbaSurface::new(200, 200);
    render(&ir, &mut surface);

    assert_eq!(surface.pixel(20, 40), Some(border_color));
    assert_eq!(surface.pixel(22, 42), Some(border_color));
    assert_eq!(surface.pixel(23, 43), Some(Rgba8::WHITE));
    // 图片走双线性采样：2x2 放大到 8x8 时，(64,40) 落在红绿之间，
    // 不是最近邻的纯绿。权重由目标像素中心到 texel 中心的距离决定。
    assert_eq!(surface.pixel(60, 40), Some(Rgba8::new(255, 0, 0, 255)));
    assert_eq!(surface.pixel(64, 40), Some(Rgba8::new(96, 159, 0, 255)));
    assert_eq!(surface.pixel(60, 44), Some(Rgba8::new(96, 0, 159, 255)));
    assert_eq!(surface.pixel(64, 44), Some(Rgba8::new(135, 159, 159, 255)));
}

#[test]
fn offscreen_ops_leave_the_canvas_white() {
    let mut ir = document_ir("A");
    ir.display.ops.clear();
    ir.display.glyphs.clear();
    ir.runs.lines.clear();
    ir.pages.truncate(1);
    ir.pages[0].ops = ebook_ir::Span::EMPTY;
    ir.pages[0].lines = ebook_ir::Span::EMPTY;

    let ops = [
        Op::Rect {
            rect: RectF::new(-100.0, -100.0, 10.0, 10.0),
            color: Rgba8::new(255, 0, 0, 255),
        },
        Op::Rect {
            rect: RectF::new(100.0, 100.0, 10.0, 10.0),
            color: Rgba8::new(0, 255, 0, 255),
        },
        Op::Border(BorderBox {
            rect: RectF::new(-50.0, -50.0, 20.0, 20.0),
            width: SidesF::all(2.0),
            color: Rgba8::new(0, 0, 255, 255),
        }),
        Op::Image {
            id: 0,
            rect: RectF::new(100.0, 100.0, 5.0, 5.0),
            fit: Fit::Fill,
        },
        Op::Unresolved(Unresolved {
            node: 0,
            rect: RectF::new(-100.0, -100.0, 50.0, 50.0),
            reason: UnresolvedReason::ImagePending,
        }),
    ];
    for op in ops {
        append_op(&mut ir, op);
    }

    let mut surface = RgbaSurface::new(32, 32);
    render(&ir, &mut surface);
    assert!(surface.pixels().iter().all(|pixel| *pixel == Rgba8::WHITE));
}

#[test]
fn ops_are_clipped_to_the_canvas() {
    let mut ir = document_ir("A");
    append_op(
        &mut ir,
        Op::Rect {
            rect: RectF::new(-100.0, -100.0, 300.0, 300.0),
            color: Rgba8::new(1, 2, 3, 255),
        },
    );
    append_op(
        &mut ir,
        Op::Border(BorderBox {
            rect: RectF::new(28.0, 28.0, 20.0, 20.0),
            width: SidesF::all(4.0),
            color: Rgba8::new(9, 8, 7, 255),
        }),
    );
    append_op(
        &mut ir,
        Op::Image {
            id: 0,
            rect: RectF::new(-5.0, -5.0, 20.0, 20.0),
            fit: Fit::Cover,
        },
    );
    append_op(
        &mut ir,
        Op::Unresolved(Unresolved {
            node: 0,
            rect: RectF::new(-10.0, -10.0, 30.0, 30.0),
            reason: UnresolvedReason::ImagePending,
        }),
    );

    let mut surface = RgbaSurface::new(32, 32);
    render(&ir, &mut surface);

    assert_eq!(surface.pixels().len(), 32 * 32);
    assert!(surface.pixels().iter().all(|pixel| pixel.a == 255));
    let ppm = surface.to_ppm();
    assert_eq!(ppm.len(), "P6\n32 32\n255\n".len() + 32 * 32 * 3);
}

fn blank_ir() -> LayoutResult {
    let mut ir = document_ir("A");
    ir.display.ops.clear();
    ir.display.glyphs.clear();
    ir.runs.lines.clear();
    ir.pages.truncate(1);
    ir.pages[0].ops = Span::EMPTY;
    ir.pages[0].lines = Span::EMPTY;
    ir
}

#[test]
fn nested_clips_intersect() {
    let mut ir = blank_ir();
    let rect = Op::Rect {
        rect: RectF::new(0.0, 0.0, 200.0, 200.0),
        color: Rgba8::BLACK,
    };
    let inner = Op::Clip {
        rect: RectF::new(20.0, 20.0, 30.0, 30.0),
        ops: vec![rect],
    };
    let outer = Op::Clip {
        rect: RectF::new(10.0, 10.0, 50.0, 50.0),
        ops: vec![inner],
    };
    append_op(&mut ir, outer);

    let mut surface = RgbaSurface::new(200, 200);
    render(&ir, &mut surface);

    // 两层裁剪的交集：x/y 都在 [20, 50)。
    assert_eq!(surface.pixel(19, 25), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(20, 25), Some(Rgba8::BLACK));
    assert_eq!(surface.pixel(49, 49), Some(Rgba8::BLACK));
    assert_eq!(surface.pixel(50, 49), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(30, 19), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(30, 20), Some(Rgba8::BLACK));
}

#[test]
fn clip_cuts_a_glyph_in_half() {
    let mut ir = blank_ir();
    ir.display.glyphs.push(Glyph {
        gid: 1,
        dx: 0.0,
        dy: 0.0,
    });
    let run = GlyphRun {
        font: FontId(0),
        size: 16.0,
        color: Rgba8::BLACK,
        origin: PointF::new(20.0, 30.0),
        glyphs: Span::new(0, 1),
        text: String::from("A"),
        dir: Dir::Ltr,
        rotate: Rotate::None,
    };
    append_op(
        &mut ir,
        Op::Clip {
            rect: RectF::new(24.0, 0.0, 100.0, 200.0),
            ops: vec![Op::GlyphRun(run)],
        },
    );

    let mut surface = RgbaSurface::new(200, 200);
    render(&ir, &mut surface);

    // 字形覆盖 x 20..28；裁剪从 x=24 开始，左半变白，右半仍有墨。
    assert_eq!(surface.pixel(23, 22), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(24, 22), Some(Rgba8::new(127, 127, 127, 255)));
    assert_eq!(surface.pixel(27, 29), Some(Rgba8::new(127, 127, 127, 255)));
    assert_eq!(surface.pixel(28, 22), Some(Rgba8::WHITE));
}

#[test]
fn border_sides_have_independent_widths() {
    let mut ir = blank_ir();
    let color = Rgba8::new(10, 20, 30, 255);
    append_op(
        &mut ir,
        Op::Border(BorderBox {
            rect: RectF::new(20.0, 40.0, 40.0, 30.0),
            width: SidesF {
                top: 2.0,
                right: 4.0,
                bottom: 6.0,
                left: 8.0,
            },
            color,
        }),
    );

    let mut surface = RgbaSurface::new(100, 100);
    render(&ir, &mut surface);

    // 上 2、下 6、左 8、右 4，四条边在拐角不重叠。
    assert_eq!(surface.pixel(20, 40), Some(color));
    assert_eq!(surface.pixel(59, 41), Some(color));
    assert_eq!(surface.pixel(20, 44), Some(color));
    assert_eq!(surface.pixel(55, 44), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(56, 44), Some(color));
    assert_eq!(surface.pixel(30, 64), Some(color));
    assert_eq!(surface.pixel(30, 62), Some(Rgba8::WHITE));
    assert_eq!(surface.pixel(30, 70), Some(Rgba8::WHITE));
}

#[test]
fn translucent_border_corners_blend_once() {
    let mut ir = blank_ir();
    // 50% 黑。拐角若被上/左两边各画一次会变成 ~25% 白，这里要求只混合一次。
    append_op(
        &mut ir,
        Op::Border(BorderBox {
            rect: RectF::new(20.0, 40.0, 40.0, 30.0),
            width: SidesF {
                top: 4.0,
                right: 4.0,
                bottom: 4.0,
                left: 4.0,
            },
            color: Rgba8::new(0, 0, 0, 128),
        }),
    );

    let mut surface = RgbaSurface::new(100, 100);
    render(&ir, &mut surface);

    let corner = surface.pixel(20, 40).expect("corner");
    assert_eq!(corner, Rgba8::new(127, 127, 127, 255));
    assert_eq!(corner, surface.pixel(23, 43).expect("corner"));
}

#[test]
fn to_ppm_is_byte_identical_across_runs_and_op_order() {
    let mut first = blank_ir();
    append_op(
        &mut first,
        Op::Rect {
            rect: RectF::new(10.0, 10.0, 20.0, 20.0),
            color: Rgba8::new(200, 10, 10, 255),
        },
    );
    append_op(
        &mut first,
        Op::Rect {
            rect: RectF::new(50.0, 50.0, 20.0, 20.0),
            color: Rgba8::new(10, 10, 200, 255),
        },
    );

    // 互不重叠的两个 op 交换顺序，字节流必须相同。
    let mut second = blank_ir();
    append_op(
        &mut second,
        Op::Rect {
            rect: RectF::new(50.0, 50.0, 20.0, 20.0),
            color: Rgba8::new(10, 10, 200, 255),
        },
    );
    append_op(
        &mut second,
        Op::Rect {
            rect: RectF::new(10.0, 10.0, 20.0, 20.0),
            color: Rgba8::new(200, 10, 10, 255),
        },
    );

    let mut a = RgbaSurface::new(200, 200);
    render(&first, &mut a);
    let mut b = RgbaSurface::new(200, 200);
    render(&first, &mut b);
    let mut c = RgbaSurface::new(200, 200);
    render(&second, &mut c);
    assert_eq!(a.to_ppm(), b.to_ppm());
    assert_eq!(a.to_ppm(), c.to_ppm());
}
