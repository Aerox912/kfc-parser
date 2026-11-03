mod error;
mod file;
mod header;
mod reader;
mod static_map;
mod writer;

/// Identifier helpers for consumers that need to map reflected names to KFC resource IDs.
/// The module is additive and does not change container parsing or EML behavior.
pub mod resource_lookup;

pub use error::*;
pub use file::*;
pub use reader::*;
pub use static_map::*;
pub use writer::*;
