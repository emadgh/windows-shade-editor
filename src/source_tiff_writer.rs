//! Binary compatibility facade for the canonical library-owned source TIFF writer.
//!
//! Keeping this writer in the same library ownership boundary as
//! `conversion_tiff` preserves access to the shared crate-private LZW strip
//! writer while existing binary `crate::source_tiff_writer::...` call sites use
//! the canonical metadata and writer types.

pub use windows_shade_editor::source_tiff_writer::*;
