//! Binary compatibility facade for the canonical library-owned source-transparency module.
//!
//! `src/main.rs` still declares `mod source_transparency;` during the incremental
//! backend ownership cleanup. Re-exporting the library module keeps existing
//! `crate::source_transparency::...` call sites source-compatible without compiling
//! a second implementation or creating a second type domain.

pub use windows_shade_editor::source_transparency::*;
