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
    paint_ops(ir, ir.display.op_slice(page.ops), surface, res, origin);
    Ok(())
}

/// 把排版流里从 `top` 开始的一段画到 surface：流坐标 `top` 落在 surface 的 y=0。
///
/// 滚动模式用：surface 是一个瓦片（瓦片高度由宿主决定），`top` 是瓦片在
/// 本章流坐标里的起点。左右留白仍画进 surface（和分页模式一致）；上下留白
/// 不参与平移——宿主在滚动容器上自己留，瓦片之间才没有空白带。
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
    paint_ops(ir, ir.display.op_slice(page.ops), surface, res, origin);
    Ok(())
}

fn paint_ops(
    ir: &LayoutResult,
    ops: &[Op],
    surface: &mut dyn Surface,
    res: &mut dyn Resources,
    origin: PointF,
) {
    for op in ops {
        match op {
            Op::GlyphRun(run) => {
                let glyphs = ir.display.glyph_slice(run.glyphs);
                let mut moved = run.clone();
                moved.origin = run.origin.shifted(-origin.x, -origin.y);
                surface.glyph_run(&moved, glyphs, res);
            }
            Op::Rect { rect, color } => surface.rect(rect.shifted(-origin.x, -origin.y), *color),
            Op::Border(border) => {
                let mut moved = border.clone();
                moved.rect = border.rect.shifted(-origin.x, -origin.y);
                surface.border(&moved);
            }
            Op::Image { id, rect, fit } => {
                surface.image(*id, rect.shifted(-origin.x, -origin.y), *fit, res)
            }
            Op::Unresolved(u) => {
                let mut moved = u.clone();
                moved.rect = u.rect.shifted(-origin.x, -origin.y);
                surface.unresolved(&moved);
            }
            Op::Clip { rect, ops } => {
                surface.push_clip(rect.shifted(-origin.x, -origin.y));
                paint_ops(ir, ops, surface, res, origin);
                surface.pop_clip();
            }
            Op::Link { rect, href } => {
                surface.link(rect.shifted(-origin.x, -origin.y), href);
            }
        }
    }
}
