use ebook_ir::{EdgesF, Len, LineHeight, Sides};

pub(crate) fn margins(sides: &Sides, base: f32, em: f32) -> EdgesF {
    EdgesF {
        top: resolve_signed(sides.top, base, em),
        right: resolve_signed(sides.right, base, em),
        bottom: resolve_signed(sides.bottom, base, em),
        left: resolve_signed(sides.left, base, em),
    }
}

pub(crate) fn sides(sides: &Sides, base: f32, em: f32) -> EdgesF {
    EdgesF {
        top: resolve_or_zero(sides.top, base, em),
        right: resolve_or_zero(sides.right, base, em),
        bottom: resolve_or_zero(sides.bottom, base, em),
        left: resolve_or_zero(sides.left, base, em),
    }
}

pub(crate) fn resolve_signed(len: Len, base: f32, em: f32) -> f32 {
    len.resolve(base, em).unwrap_or(0.0)
}

pub(crate) fn resolve_or_zero(len: Len, base: f32, em: f32) -> f32 {
    len.resolve(base, em).unwrap_or(0.0).max(0.0)
}

pub(crate) fn line_px(line_height: LineHeight, size: f32) -> f32 {
    match line_height {
        LineHeight::Normal => size * 1.5,
        LineHeight::Px(v) => v.max(0.0),
        LineHeight::Mul(v) => size * v.max(0.0),
    }
}
