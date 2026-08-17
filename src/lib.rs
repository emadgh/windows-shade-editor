#![cfg(windows)]

pub mod color_conversion;
pub mod conversion_analytics;
pub mod conversion_capabilities;
pub mod conversion_output;
pub mod conversion_preflight;
pub mod conversion_preset_library;
pub mod conversion_presets;
pub mod conversion_queue;
pub mod conversion_recipe;
pub mod conversion_tiff;
pub mod conversion_transaction;
pub mod conversion_workflow;
pub mod custom_optimizer_config;
pub mod device_characterization;
pub mod device_characterization_model;
pub mod device_characterization_package;
pub mod devicelink_conversion;
#[path = "dpi.rs"]
pub mod dpi;
pub mod export;
pub mod export_recipe;
pub mod gradient_continuity;
pub mod gradient_validation;
pub mod icc_conversion;
pub mod icc_conversion_worker;
pub mod inverse_lut_artifact;
pub mod inverse_lut_identity;
pub mod inverse_separation_solver;
pub mod model;
pub mod nchannel_icc;
#[path = "palette.rs"]
pub mod palette;
pub mod png_source;
pub mod production_acceptance;
pub mod production_project;
pub mod production_target;
#[path = "safe_fs.rs"]
pub mod safe_fs;
pub mod separation_optimizer;
#[path = "tiff_io.rs"]
pub mod tiff_io;

#[cfg(test)]
mod inverse_lut_artifact_tests;
#[cfg(test)]
mod tiff_conformance_tests;
