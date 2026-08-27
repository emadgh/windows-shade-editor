//! Binary compatibility facade for the canonical library-owned project/domain model.
//!
//! Keep historical `crate::model::...` call sites in the GUI while compiling the
//! implementation only once in the package library.

pub use windows_shade_editor::model::*;
