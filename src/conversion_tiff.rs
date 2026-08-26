//! Binary compatibility facade for the canonical library-owned conversion TIFF module.
//!
//! The conversion writer implementation and its crate-private LZW strip writer
//! now live inside the package library. Existing binary `crate::conversion_tiff::...`
//! call sites keep using the exact library-owned public API without compiling a
//! second writer implementation.

pub use windows_shade_editor::conversion_tiff::*;
