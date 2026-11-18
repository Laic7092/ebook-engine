#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod flow;
mod linebreak;
mod linebreak_table;
mod measure;
mod paginate;
mod resolve;

use alloc::vec::Vec;

use ebook_ir::{
    DisplayList, EdgesF, FontId, FontSpec, GlyphRun, ImageId, LayoutResult, LineBox, Op, RectF,
    RunMap, SizeF, StyledDoc, WritingMode, IR_VERSION,
};

pub use measure::{StubMeasurer, TextMeasurer};

pub trait ResourceView {
    fn image_size(&self, id: ImageId) -> Option<SizeF>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoImageSizes;

impl ResourceView for NoImageSizes {
    fn image_size(&self, _id: ImageId) -> Option<SizeF> {
        None
    }
}

#[derive(Clone, Copy)]
pub struct LayoutEnv<'a> {
    pub measurer: &'a dyn TextMeasurer,
    pub resources: &'a dyn ResourceView,
}

impl LayoutEnv<'_> {
    pub fn select_font(&self, spec: &FontSpec) -> FontId {
        self.measurer.select(spec)
    }

    pub fn font_for(&self, spec: &FontSpec, ch: char) -> FontId {
        self.measurer.select_char(spec, ch)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PageConstraint {
    pub page: SizeF,
    pub margin: EdgesF,
    pub writing_mode: WritingMode,
}

impl PageConstraint {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            page: SizeF::new(width, height),
            margin: EdgesF::ZERO,
            writing_mode: WritingMode::HorizontalTb,
        }
    }

    pub fn with_margin(mut self, margin: EdgesF) -> Self {
        self.margin = margin;
        self
    }

    pub fn content_size(&self) -> SizeF {
        SizeF::new(
            (self.page.w - self.margin.horizontal()).max(1.0),
            (self.page.h - self.margin.vertical()).max(1.0),
        )
    }
}

impl Default for PageConstraint {
    fn default() -> Self {
        Self::new(600.0, 800.0)
    }
}

#[derive(Default)]
pub struct Layout;

impl Layout {
    pub fn new() -> Self {
        Self
    }

    pub fn layout(
        &mut self,
        doc: &StyledDoc,
        env: &LayoutEnv<'_>,
        c: &PageConstraint,
    ) -> LayoutResult {
        let mut built = flow::flow(doc, env, c);
        let content = c.content_size();
        let starts = paginate::plan(&units_of(&built.items), content.h);
        let mut builder = paginate::PageBuilder::new(content, c.margin);
        let mut ops: Vec<Op> = Vec::new();
        let mut lines: Vec<LineBox> = Vec::new();

        for (index, item) in built.items.drain(..).enumerate() {
            let page = paginate::page_of(&starts, index);
            let top = starts[page as usize].y;
            builder.sync(page, top, ops.len() as u32, lines.len() as u32);
            match item {
                flow::Item::Line(line) => {
                    builder.set_anchor(line.anchor.as_ref());
                    for run in &line.runs {
                        ops.push(Op::GlyphRun(GlyphRun {
                            font: run.font,
                            size: run.size,
                            color: run.color,
                            origin: run.origin,
                            glyphs: run.glyphs,
                            text: run.text.clone(),
                            dir: run.dir,
                            rotate: run.rotate,
                        }));
                    }
                    ops.extend(line.unresolved);
                    lines.push(LineBox {
                        rect: RectF::new(line.x, line.y, line.w, line.h),
                        baseline: line.baseline,
                        dir: line.dir,
                        src: line.src,
                        anchor: line.anchor,
                    });
                }
                flow::Item::Op(op) => ops.push(op),
            }
        }
        builder.close(ops.len() as u32, lines.len() as u32);

        LayoutResult {
            version: IR_VERSION,
            spine: doc.meta.spine,
            pages: builder.pages,
            display: DisplayList { ops, glyphs: built.glyphs },
            runs: RunMap { lines },
            dirty: built.dirty,
        }
    }
}

fn units_of(items: &[flow::Item]) -> Vec<paginate::Unit> {
    let mut units = Vec::with_capacity(items.len());
    for item in items {
        units.push(match item {
            flow::Item::Line(line) => paginate::Unit {
                y: line.y,
                h: line.h,
                line: true,
                block: line.block,
                order: line.order,
                len: line.len,
                orphans: line.orphans,
                widows: line.widows,
                force: line.force,
                avoid_before: line.avoid_before,
                avoid_after: line.avoid_after,
            },
            flow::Item::Op(op) => {
                let (y, h) = paginate::op_bounds(op);
                paginate::Unit {
                    y,
                    h,
                    line: false,
                    block: u32::MAX,
                    order: 0,
                    len: 0,
                    orphans: 1,
                    widows: 1,
                    force: false,
                    avoid_before: false,
                    avoid_after: false,
                }
            }
        });
    }
    units
}

pub fn layout(doc: &StyledDoc, env: &LayoutEnv<'_>, c: &PageConstraint) -> LayoutResult {
    Layout::new().layout(doc, env, c)
}
