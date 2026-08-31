#![cfg(windows)]

pub mod characterization_acquisition;
pub mod characterization_intake;
#[path = "color_conversion_impl/mod.rs"]
pub mod color_conversion;
pub mod conversion_analytics;
pub mod conversion_audit;
pub mod conversion_batch;
pub mod conversion_batch_execution;
pub mod conversion_batch_queue;
pub mod conversion_candidate_comparison;
pub mod conversion_candidate_promotion;
#[path = "conversion_candidate_preview_runtime.rs"]
pub mod conversion_candidate_preview;
pub mod conversion_capabilities;
pub mod conversion_job_authority;
pub mod conversion_output;
pub mod conversion_preflight;
pub mod conversion_preset_library;
pub mod conversion_preset_runtime;
pub mod conversion_preset_store;
pub mod conversion_presets;
pub mod conversion_queue;
pub mod conversion_recipe;
#[path = "conversion_recovery_route.rs"]
pub mod conversion_recovery;
pub mod conversion_route;
pub mod conversion_route_migration;
pub mod conversion_route_migration_checkpoint;
pub mod conversion_route_migration_discovery;
pub mod conversion_route_migration_executor;
pub mod conversion_route_migration_project;
pub mod conversion_route_migration_runtime;
#[path = "conversion_tiff_impl.rs"]
pub mod conversion_tiff;
pub mod conversion_transaction;
#[path = "conversion_transaction_disposition_wrapper.rs"]
pub mod conversion_transaction_disposition;
pub mod conversion_workflow;
#[path = "custom_optimizer_config_impl.rs"]
pub mod custom_optimizer_config;
pub mod custom_optimizer_bundle_plan;
pub mod custom_optimizer_evidence;
pub mod custom_optimizer_evidence_binding;
pub mod custom_optimizer_evidence_bundle;
pub mod custom_optimizer_operator_binding;
pub mod custom_optimizer_operator_controls;
pub mod custom_optimizer_raster_transform;
pub mod custom_optimizer_strategy_capability;
pub mod design_source;
pub mod design_source_preview;
pub mod device_characterization;
pub mod device_characterization_model;
pub mod device_characterization_package;
pub mod devicelink_conversion;
#[path = "dpi_impl.rs"]
pub mod dpi;
#[path = "export_impl.rs"]
pub mod export;
pub mod external_validation_evidence;
#[path = "export_recipe_impl.rs"]
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
#[path = "model_impl.rs"]
pub mod model;
pub mod nchannel_icc;
pub mod optimizer_forward_model_authority;
pub mod output_icc_forward_model;
#[path = "palette_impl.rs"]
pub mod palette;
pub mod png_source;
pub mod production_acceptance;
pub mod production_colorimetry;
pub mod production_destination;
pub mod production_destination_selection;
pub mod production_lab_transform;
pub mod production_lineage;
pub mod production_profile_catalog;
#[path = "production_project_impl.rs"]
pub mod production_project;
pub mod production_project_compat;
pub mod production_project_disposition;
pub mod production_replacement;
pub mod production_staleness;
pub mod production_target;
pub mod profile_backed_candidate_preview;
pub mod profile_backed_inverse_lut_artifact;
pub mod profile_backed_inverse_lut_builder;
pub mod profile_backed_optimizer_authority;
pub mod profile_backed_optimizer_execution_capture;
pub mod profile_backed_optimizer_raster_transform;
pub mod profile_backed_optimizer_ui_contract;
pub mod profile_backed_optimizer_ui_execution;
pub mod project_link_navigation;
pub mod queue_core;
pub mod reconversion_policy;
#[path = "safe_fs_impl.rs"]
pub mod safe_fs;
pub use safe_fs::{staging, tiff_performance};
pub mod separation_optimizer;
#[path = "source_tiff_writer_impl.rs"]
pub mod source_tiff_writer;
pub mod source_profile_fallback;
#[path = "source_transparency_impl.rs"]
pub mod source_transparency;
pub mod test_stack;
#[path = "tiff_output_impl.rs"]
pub mod tiff_output;
#[path = "tiff_io_impl.rs"]
pub mod tiff_io;
pub mod tiff_io_inspection;
pub mod unified_optimizer_job_authority;

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