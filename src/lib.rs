#![cfg(windows)]

pub mod color_conversion;
pub mod conversion_analytics;
pub mod conversion_capabilities;
pub mod conversion_workflow;
#[path = "dpi.rs"]
pub mod dpi;
pub mod export;
pub mod model;
#[path = "palette.rs"]
pub mod palette;
pub mod production_acceptance;
#[path = "safe_fs.rs"]
pub mod safe_fs;
#[path = "tiff_io.rs"]
pub mod tiff_io;

#[cfg(test)]
mod tiff_conformance_tests;
