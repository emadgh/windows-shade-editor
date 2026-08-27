//! Binary compatibility facade for the canonical library-owned export recipe.
//!
//! Historical GUI `crate::export_recipe::...` paths remain stable while the
//! implementation is compiled only once in the package library.

pub use windows_shade_editor::export_recipe::*;
