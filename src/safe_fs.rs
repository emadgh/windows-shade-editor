//! Binary compatibility facade for the canonical library-owned safe filesystem module.
//!
//! The binary keeps `mod safe_fs;` during the incremental backend ownership cleanup.
//! Re-exporting the library module preserves existing `crate::safe_fs::...` paths while
//! the implementation, staging registry, and TIFF performance hooks compile only once.

pub use windows_shade_editor::safe_fs::*;
