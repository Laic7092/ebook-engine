use ebook_ir::{LayoutResult, Op, PointF, RenderError, Resources, Surface};

pub fn paint(
    ir: &LayoutResult,
    page: u32,
    surface: &mut dyn Surface,
    res: &mut dyn Resources,
) -> Result<(), RenderError> {
    let page = ir.page(page).ok_or(RenderError::NoSuchPage(page))?;
    let origin = PointF::new(page.origin.x - page.inset.left, page.origin.y - page.inset.top);
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
        }
    }
}
