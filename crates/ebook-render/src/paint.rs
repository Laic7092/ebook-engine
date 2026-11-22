use ebook_ir::{LayoutResult, Op, PointF, RenderError, Resources, Surface};

pub fn paint(
    ir: &LayoutResult,
    page: u32,
    surface: &mut dyn Surface,
    res: &mut dyn Resources,
) -> Result<(), RenderError> {
    let page = ir.page(page).ok_or(RenderError::NoSuchPage(page))?;
    let origin = PointF::new(
        page.origin.x - page.inset.left,
        page.origin.y - page.inset.top,
    );
    paint_ops(
        ir,
        ir.display.op_slice(page.ops),
        surface,
        res,
        origin,
        None,
    );
    Ok(())
}

/// 把排版流里从 `top` 开始的一段画到 surface：流坐标 `top` 落在 surface 的 y=0。
///
/// 滚动模式用：surface 是一个瓦片（瓦片高度由宿主决定），`top` 是瓦片在
/// 本章流坐标里的起点。左右留白仍画进 surface（和分页模式一致）；上下留白
/// 不参与平移——宿主在滚动容器上自己留，瓦片之间才没有空白带。
///
/// **只画落在窗口里的 op**：窗口外的字形连光栅查询都不做，所以代价只跟
/// 瓦片高有关，不跟章高有关。窗口高由 `surface.size().h / surface.scale()`
/// 得出（见 [`Surface::scale`]）。
///
/// surface 不会被清空：调用方先清成纸色（`Engine::bitmap_window` 已经做了）。
///
/// `top` 是流坐标。分页模式下要原样画一页，传 `page.origin.y - page.inset.top`；
/// 滚动模式下 `page.origin.y` 就是 0，传瓦片的内容 y 即可。
pub fn paint_window(
    ir: &LayoutResult,
    page: u32,
    top: f32,
    surface: &mut dyn Surface,
    res: &mut dyn Resources,
) -> Result<(), RenderError> {
    let page = ir.page(page).ok_or(RenderError::NoSuchPage(page))?;
    let origin = PointF::new(page.origin.x - page.inset.left, top);
    let scale = surface.scale();
    let height = if scale.is_finite() && scale > 0.0 {
        surface.size().h / scale
    } else {
        surface.size().h
    };
    let band = if top.is_finite() && height.is_finite() && height > 0.0 {
        Some(Band {
            top,
            bottom: top + height,
        })
    } else {
        None
    };
    paint_ops(
        ir,
        ir.display.op_slice(page.ops),
        surface,
        res,
        origin,
        band,
    );
    Ok(())
}

/// 排版流坐标里可见的一段 `[top, bottom)`。`None` 表示整页都要画。
#[derive(Clone, Copy, Debug)]
struct Band {
    top: f32,
    bottom: f32,
}

impl Band {
    /// 保守判断：与 `[top, bottom)` 有交集才画。
    fn intersects(self, top: f32, bottom: f32) -> bool {
        bottom > self.top && top < self.bottom
    }

    fn intersect(self, rect: ebook_ir::RectF) -> Option<Self> {
        let top = self.top.max(rect.y);
        let bottom = self.bottom.min(rect.bottom());
        (bottom > top).then_some(Self { top, bottom })
    }
}

/// 一个 run 在流坐标里的纵向范围。`±2em` 足够覆盖升部、降部、下划线与
/// 合成斜体的切变，宁可比实际大一点：漏画是不可逆的，多画一个 run 只是
/// 让后端按 surface 边界裁像素（老行为）。
fn run_band(run: &ebook_ir::GlyphRun) -> (f32, f32) {
    let reach = run.size.abs().max(1.0) * 2.0;
    (run.origin.y - reach, run.origin.y + reach)
}

fn paint_ops(
    ir: &LayoutResult,
    ops: &[Op],
    surface: &mut dyn Surface,
    res: &mut dyn Resources,
    origin: PointF,
    band: Option<Band>,
) {
    for op in ops {
        match op {
            Op::GlyphRun(run) => {
                if let Some(band) = band {
                    let (top, bottom) = run_band(run);
                    if !band.intersects(top, bottom) {
                        continue;
                    }
                }
                let glyphs = ir.display.glyph_slice(run.glyphs);
                // 平移交给后端，不再 clone 整个 `GlyphRun`（里面有一份 `text`）。
                surface.glyph_run_shifted(run, glyphs, PointF::new(-origin.x, -origin.y), res);
            }
            Op::Rect { rect, color } => {
                if let Some(band) = band {
                    if !band.intersects(rect.y, rect.bottom()) {
                        continue;
                    }
                }
                surface.rect(rect.shifted(-origin.x, -origin.y), *color);
            }
            Op::Border(border) => {
                if let Some(band) = band {
                    let rect = border.rect;
                    if !band.intersects(rect.y, rect.bottom()) {
                        continue;
                    }
                }
                let mut moved = border.clone();
                moved.rect = border.rect.shifted(-origin.x, -origin.y);
                surface.border(&moved);
            }
            Op::Image { id, rect, fit } => {
                if let Some(band) = band {
                    if !band.intersects(rect.y, rect.bottom()) {
                        continue;
                    }
                }
                surface.image(*id, rect.shifted(-origin.x, -origin.y), *fit, res)
            }
            Op::Unresolved(u) => {
                if let Some(band) = band {
                    if !band.intersects(u.rect.y, u.rect.bottom()) {
                        continue;
                    }
                }
                let mut moved = u.clone();
                moved.rect = u.rect.shifted(-origin.x, -origin.y);
                surface.unresolved(&moved);
            }
            Op::Clip { rect, ops } => {
                let inner = match band {
                    Some(band) => match band.intersect(*rect) {
                        Some(narrowed) => Some(narrowed),
                        // 整个裁剪块都在窗口外：连 push/pop 都不必做。
                        None => continue,
                    },
                    None => None,
                };
                surface.push_clip(rect.shifted(-origin.x, -origin.y));
                paint_ops(ir, ops, surface, res, origin, inner);
                surface.pop_clip();
            }
            Op::Link { rect, href } => {
                if let Some(band) = band {
                    if !band.intersects(rect.y, rect.bottom()) {
                        continue;
                    }
                }
                surface.link(rect.shifted(-origin.x, -origin.y), href);
            }
        }
    }
}
