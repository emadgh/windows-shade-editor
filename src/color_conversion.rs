//! Binary compatibility facade for the canonical library-owned Color Conversion domain.
//!
//! This preserves historical `crate::color_conversion::...` GUI paths without
//! compiling a second copy of conversion types or provenance structures.

pub use windows_shade_editor::color_conversion::*;
