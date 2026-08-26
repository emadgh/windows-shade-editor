//! Binary compatibility facade for the canonical library-owned TIFF IO module.
//!
//! `src/main.rs` still declares `mod tiff_io;` while backend ownership is
//! migrated incrementally. Re-exporting the library module keeps existing
//! `crate::tiff_io::...` call sites source-compatible while ensuring metadata,
//! streaming and decoded-image types have one canonical ownership domain.

pub use windows_shade_editor::tiff_io::*;
