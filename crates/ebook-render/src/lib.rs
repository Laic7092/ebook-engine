#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod paint;
pub mod scale;
pub mod stats;
pub mod svg;

pub use paint::paint;
pub use scale::ScaledSurface;
pub use stats::{stats, Stats};
pub use svg::{SvgRenderer, SvgSurface};
