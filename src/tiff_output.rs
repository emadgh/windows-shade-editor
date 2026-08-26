//! Binary compatibility facade for the canonical library-owned TIFF output module.
//!
//! The binary keeps `mod tiff_output;` during the incremental backend ownership cleanup.
//! Re-exporting the library module preserves existing `crate::tiff_output::...` paths
//! without compiling a second implementation or creating a second type domain.

pub use windows_shade_editor::tiff_output::*;
