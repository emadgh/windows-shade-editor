//! Binary compatibility facade for the canonical library-owned palette module.
//!
//! `src/main.rs` still declares `mod palette;` while the binary/backend ownership
//! cleanup is migrated incrementally. Re-exporting the library module here
//! keeps existing `crate::palette::...` call sites source-compatible without
//! compiling a second palette implementation or creating a second type domain.

pub use windows_shade_editor::palette::*;
