#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod codec;
pub mod doc;
pub mod font;
pub mod geom;
pub mod layout;
pub mod render;

pub use codec::{decode, encode, hash_of, read_header, to_bytes, CodecError, MAGIC};
pub use doc::{
    Anchor, Border, Break, BreakPolicy, ComputedStyle, Dir, Display, DocMeta, FontSpec, ImageId,
    ImageMeta, Len, LineHeight, ListStyle, Node, NodeId, NodeKind, Span, StyledDoc, Tag, TextAlign,
    TextStyle, TextTransform, VerticalAlign, WhiteSpace, WordBreak, WritingMode,
};
pub use font::{FontData, FontDesc, FontFamilies, FontFamily, FontId, FontSet, FontStyle};
pub use geom::{EdgesF, PointF, RectF, Rgba8, Sided, Sides, SidesF, SizeF};
pub use layout::{
    BorderBox, DirtyBlock, DisplayList, Fit, Glyph, GlyphRun, IrVersion, Issue, IssueCode,
    LayoutResult, Limits, LineBox, Location, Op, Page, Rotate, RunMap, Unresolved,
    UnresolvedReason, IR_VERSION,
};
pub use render::{GlyphBitmap, ImageData, NoResources, RenderError, Renderer, Resources, Surface};
