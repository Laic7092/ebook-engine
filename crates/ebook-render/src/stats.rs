use ebook_ir::LayoutResult;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub pages: usize,
    pub lines: usize,
    pub glyphs: usize,
    pub ops: usize,
    pub dirty: usize,
    pub bytes: usize,
}

pub fn stats(ir: &LayoutResult, bytes: usize) -> Stats {
    Stats {
        pages: ir.pages.len(),
        lines: ir.runs.lines.len(),
        glyphs: ir.display.glyphs.len(),
        ops: ir.display.ops.len(),
        dirty: ir.dirty.len(),
        bytes,
    }
}
