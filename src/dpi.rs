//! Binary compatibility facade for the canonical library-owned DPI module.
//!
//! `src/main.rs` still declares `mod dpi;` while the binary/backend ownership
//! cleanup is migrated incrementally. Re-exporting the library module here
//! keeps existing `crate::dpi::...` call sites source-compatible without
//! compiling a second DPI implementation or creating a second type domain.

pub use windows_shade_editor::dpi::*;
