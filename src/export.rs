//! Binary compatibility facade for the canonical library-owned export backend.
//!
//! Keep historical `crate::export::...` GUI paths while compiling the export
//! implementation only once in the package library.

pub use windows_shade_editor::export::*;
