//! 设备像素缩放：把页内坐标从 CSS px 换成设备像素再交给后端。
//!
//! DPR 是光栅设备的概念，不属于排版：IR 永远是 CSS px。这里做的是
//! “画之前乘一次”，而不是“画完再放大”：
//!
//! - 坐标、字号、边框宽度、字形 advance 全部乘 `scale`，所以字形按设备像素
//!   `size * scale` 光栅化，边缘不会因为插值而发糊。
//! - 子像素相位（小数部分）跟着一起缩放，落点仍由后端的 `floor` 决定，
//!   跨平台逐字节一致的既有性质不变。
//!
//! 只影响绘制，不改变 `IR`，也不影响命中测试（那仍然在 CSS px 上做）。

use alloc::vec::Vec;

use ebook_ir::{
    BorderBox, Fit, Glyph, GlyphRun, ImageId, PointF, RectF, Resources, Rgba8, SidesF, SizeF,
    Surface, Unresolved,
};

/// 装饰另一个 [`Surface`]，所有几何乘一个常数。
pub struct ScaledSurface<'a> {
    inner: &'a mut dyn Surface,
    scale: f32,
}

impl<'a> ScaledSurface<'a> {
    /// `scale` 非有限或 <= 0 时按 1.0 处理，避免 NaN 传染到坐标。
    pub fn new(inner: &'a mut dyn Surface, scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        Self { inner, scale }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    fn scaled_len(&self, value: f32) -> f32 {
        value * self.scale
    }

    fn scaled_rect(&self, rect: RectF) -> RectF {
        RectF::new(
            self.scaled_len(rect.x),
            self.scaled_len(rect.y),
            self.scaled_len(rect.w),
            self.scaled_len(rect.h),
        )
    }

    fn scaled_sides(&self, sides: SidesF) -> SidesF {
        SidesF {
            top: self.scaled_len(sides.top),
            right: self.scaled_len(sides.right),
            bottom: self.scaled_len(sides.bottom),
            left: self.scaled_len(sides.left),
        }
    }
}

impl Surface for ScaledSurface<'_> {
    fn size(&self) -> SizeF {
        let size = self.inner.size();
        SizeF::new(self.scaled_len(size.w), self.scaled_len(size.h))
    }

    fn glyph_run(&mut self, run: &GlyphRun, glyphs: &[Glyph], res: &mut dyn Resources) {
        self.glyph_run_shifted(run, glyphs, PointF::new(0.0, 0.0), res);
    }

    fn glyph_run_shifted(
        &mut self,
        run: &GlyphRun,
        glyphs: &[Glyph],
        shift: PointF,
        res: &mut dyn Resources,
    ) {
        // 字号是光栅化入参，必须按设备像素给；advance 与偏移同步缩放，
        // 否则字形会在缩放后重叠或拉开。平移与缩放一起算，内层后端
        // 就不需要再为每个 run 复制一次 `GlyphRun`。
        let mut moved = run.clone();
        moved.origin = PointF::new(
            self.scaled_len(run.origin.x + shift.x),
            self.scaled_len(run.origin.y + shift.y),
        );
        moved.size = self.scaled_len(run.size);
        let scaled: Vec<Glyph> = glyphs
            .iter()
            .map(|glyph| Glyph {
                dx: self.scaled_len(glyph.dx),
                dy: self.scaled_len(glyph.dy),
                ..*glyph
            })
            .collect();
        self.inner.glyph_run(&moved, &scaled, res);
    }

    fn rect(&mut self, rect: RectF, color: Rgba8) {
        self.inner.rect(self.scaled_rect(rect), color);
    }

    fn border(&mut self, border: &BorderBox) {
        let mut moved = border.clone();
        moved.rect = self.scaled_rect(border.rect);
        moved.width = self.scaled_sides(border.width);
        self.inner.border(&moved);
    }

    fn image(&mut self, id: ImageId, rect: RectF, fit: Fit, res: &mut dyn Resources) {
        self.inner.image(id, self.scaled_rect(rect), fit, res);
    }

    fn unresolved(&mut self, unresolved: &Unresolved) {
        let mut moved = unresolved.clone();
        moved.rect = self.scaled_rect(unresolved.rect);
        self.inner.unresolved(&moved);
    }

    fn link(&mut self, rect: RectF, href: &str) {
        self.inner.link(self.scaled_rect(rect), href);
    }

    fn push_clip(&mut self, rect: RectF) {
        self.inner.push_clip(self.scaled_rect(rect));
    }

    fn pop_clip(&mut self) {
        self.inner.pop_clip();
    }
}
