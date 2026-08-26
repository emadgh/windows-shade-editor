//! Binary compatibility facade for the canonical library-owned Custom Optimizer config module.
//!
//! `src/main.rs` still declares `mod custom_optimizer_config;` during the incremental
//! backend ownership cleanup. Re-exporting the library module keeps existing
//! `crate::custom_optimizer_config::...` call sites source-compatible without
//! compiling a second implementation or creating a second type domain.

pub use windows_shade_editor::custom_optimizer_config::*;
