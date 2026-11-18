pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod glue;

use ebook_ir::{codec, EdgesF};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

pub fn layout_text(text: &str, width: f32, height: f32, margin: f32) -> Vec<u8> {
    let doc = ebook_format::txt::build(text.as_bytes());
    layout_doc(&doc, width, height, margin)
}

#[cfg(feature = "epub")]
pub fn layout_chapter(
    source: &[u8],
    name: &str,
    spine: usize,
    width: f32,
    height: f32,
    margin: f32,
) -> Vec<u8> {
    let mut src = ebook_format::MemorySource::new();
    src.insert(name, source.to_vec());
    let Ok(opened) = ebook_format::open(&src) else {
        return Vec::new();
    };
    let Ok(doc) = opened.chapter(&src, spine) else {
        return Vec::new();
    };
    layout_doc(&doc, width, height, margin)
}

fn layout_doc(doc: &ebook_ir::StyledDoc, width: f32, height: f32, margin: f32) -> Vec<u8> {
    let env = LayoutEnv { measurer: &StubMeasurer, resources: &NoImageSizes };
    let constraint = PageConstraint::new(width, height).with_margin(EdgesF::all(margin));
    let ir = Layout::new().layout(doc, &env, &constraint);
    codec::to_bytes(&ir).unwrap_or_default()
}
