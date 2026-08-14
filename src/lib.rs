#![cfg(windows)]

#[path = "dpi.rs"]
pub mod dpi;
#[path = "export_v6.rs"]
pub mod export;
#[path = "model_v6.rs"]
pub mod model;
#[path = "palette.rs"]
pub mod palette;
#[path = "safe_fs.rs"]
pub mod safe_fs;
#[path = "tiff_io.rs"]
pub mod tiff_io;

#[cfg(test)]
mod tiff_conformance_tests;
