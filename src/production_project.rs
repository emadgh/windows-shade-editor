//! Binary compatibility facade for the canonical library-owned Production project builder.
//!
//! Historical GUI `crate::production_project::...` paths remain stable while the
//! implementation is compiled only once in the package library.

pub use windows_shade_editor::production_project::*;
