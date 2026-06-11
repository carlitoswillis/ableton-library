//! als-core: streaming metadata extraction for Ableton Live Sets (.als).
//!
//! See ai/ARCHITECTURE.md for the system design and
//! tools/reference_extract.py for the executable spec this crate mirrors.

pub mod model;
pub mod parser;

pub use model::*;
pub use parser::{parse_set, ParseError};
