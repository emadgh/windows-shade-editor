#![cfg(windows)]

pub mod color_conversion;
pub mod conversion_analytics;
pub mod conversion_audit;
pub mod conversion_batch;
pub mod conversion_batch_execution;
pub mod conversion_batch_queue;
pub mod conversion_capabilities;
pub mod conversion_output;
pub mod conversion_preflight;
pub mod conversion_preset_library;
pub mod conversion_presets;
pub mod conversion_queue;
pub mod conversion_recipe;
pub mod conversion_recovery;
pub mod conversion_tiff;
pub mod conversion_transaction;
pub mod conversion_transaction_disposition;
pub mod conversion_workflow;
pub mod custom_optimizer_config;
pub mod custom_optimizer_evidence;
pub mod custom_optimizer_raster_transform;
pub mod design_source;
pub mod design_source_preview;
pub mod device_characterization;
pub mod device_characterization_model;
pub mod device_characterization_package;
pub mod devicelink_conversion;
#[path = "dpi.rs"]
pub mod dpi;
pub mod export;
pub mod export_recipe;
pub mod file_observer;
pub mod gradient_continuity;
pub mod gradient_validation;
pub mod icc_conversion;
pub mod icc_conversion_worker;
#[path = "icc_profile_registry.rs"]
mod icc_profile_registry_raw;
#[path = "icc_profile_registry_observed.rs"]
pub mod icc_profile_registry;
pub mod inverse_lut_artifact;
pub mod inverse_lut_calibration_analysis;
pub mod inverse_lut_calibration_corpus;
pub mod inverse_lut_continuity_builder;
pub mod inverse_lut_continuity_field;
pub mod inverse_lut_holdout;
pub mod inverse_lut_identity;
pub mod inverse_lut_path_validation;
pub mod inverse_lut_production_eligibility;
pub mod inverse_lut_runtime;
pub mod inverse_lut_threshold_set;
pub mod inverse_lut_validation;
pub mod inverse_lut_validation_artifact;
pub mod inverse_lut_validation_eval;
pub mod inverse_lut_validation_reference;
pub mod inverse_lut_validation_runner;
pub mod inverse_separation_solver;
pub mod jpeg_source;
pub mod model;
pub mod nchannel_icc;
#[path = "palette.rs"]
pub mod palette;
pub mod png_source;
pub mod production_acceptance;
pub mod production_colorimetry;
pub mod production_destination;
pub mod production_destination_selection;
pub mod production_lab_transform;
pub mod production_lineage;
pub mod production_profile_catalog;
pub mod production_project;
pub mod production_project_compat;
pub mod production_project_disposition;
pub mod production_replacement;
pub mod production_staleness;
pub mod production_target;
pub mod project_link_navigation;
pub mod queue_core;
pub mod reconversion_policy;
#[path = "safe_fs.rs"]
pub mod safe_fs;
pub mod separation_optimizer;
pub mod source_tiff_writer;
pub mod source_transparency;
pub mod test_stack;
pub mod tiff_output;
#[path = "tiff_io.rs"]
pub mod tiff_io;

#[cfg(test)]
pub(crate) mod color_conversion_test_support;
#[cfg(test)]
mod conversion_capture_compat_tests;
#[cfg(test)]
mod custom_optimizer_evidence_tests;
#[cfg(test)]
mod inverse_lut_artifact_tests;
#[cfg(test)]
mod inverse_lut_build_tests;
#[cfg(test)]
mod inverse_lut_continuity_field_tests;
#[cfg(test)]
mod inverse_lut_continuity_path_validation_tests;
#[cfg(test)]
mod inverse_lut_continuity_sensitivity_tests;
#[cfg(test)]
mod inverse_lut_production_eligibility_tests;
#[cfg(test)]
mod inverse_lut_runtime_tests;
#[cfg(test)]
mod inverse_lut_validation_tests;
#[cfg(test)]
mod test_stack_tests;
#[cfg(test)]
mod tiff_conformance_tests;
