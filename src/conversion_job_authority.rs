use std::path::PathBuf;

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_transaction::{
    CapturedOutputPolicy, CapturedSourceProfile, ConversionJobCapture,
};
use crate::custom_optimizer_evidence::CapturedCustomOptimizerEvidence;
use crate::model::ShadeProject;

/// Authority payload attached to one exact immutable conversion recipe when a
/// production job is captured.
///
/// ICC and DeviceLink jobs must use `Standard`. Custom Optimizer jobs must carry
/// the exact immutable evidence capture that will be reopened and independently
/// authorized again by the filesystem worker. This enum is deliberately not a
/// serialized production-eligibility token and does not itself grant execution.
#[derive(Clone, Debug)]
pub enum ConversionJobAuthority {
    Standard,
    CustomOptimizer(CapturedCustomOptimizerEvidence),
}

impl ConversionJobAuthority {
    pub fn for_recipe(
        recipe: &ConversionRecipe,
        custom_optimizer_evidence: Option<CapturedCustomOptimizerEvidence>,
    ) -> Result<Self, String> {
        validate_authority_binding(recipe.engine_mode, custom_optimizer_evidence.is_some())?;
        match custom_optimizer_evidence {
            Some(evidence) => Ok(Self::CustomOptimizer(evidence)),
            None => Ok(Self::Standard),
        }
    }
}

pub fn validate_authority_binding(
    engine_mode: ConversionEngineMode,
    has_custom_optimizer_evidence: bool,
) -> Result<(), String> {
    match (engine_mode, has_custom_optimizer_evidence) {
        (ConversionEngineMode::CustomOptimizer, false) => Err(
            "Custom Optimizer final conversion requires immutable production evidence captured for the exact recipe."
                .to_owned(),
        ),
        (ConversionEngineMode::CustomOptimizer, true) => Ok(()),
        (ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink, true) => Err(
            "ICC/DeviceLink final conversion cannot carry Custom Optimizer production evidence."
                .to_owned(),
        ),
        (ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink, false) => Ok(()),
    }
}

/// Capture one final production job through the authority-correct constructor.
///
/// The Custom Optimizer branch always delegates to
/// `ConversionJobCapture::capture_custom_optimizer`; the standard branch always
/// delegates to `ConversionJobCapture::capture`. No caller can accidentally
/// capture a Custom Optimizer recipe through the standard constructor or attach
/// optimizer evidence to ICC/DeviceLink work.
#[allow(clippy::too_many_arguments)]
pub fn capture_conversion_job_with_authority(
    authority: ConversionJobAuthority,
    source_project: &ShadeProject,
    source_project_path: PathBuf,
    source_project_file_sha256: String,
    source_face_path: PathBuf,
    source_snapshot_id: Option<u64>,
    source_file_sha256: String,
    source_profile: CapturedSourceProfile,
    conversion_recipe: ConversionRecipe,
    output_policy: CapturedOutputPolicy,
    output_tiff_path: PathBuf,
    production_project_path: PathBuf,
    production_project_name: String,
    output_face_label: String,
) -> Result<ConversionJobCapture, String> {
    match (conversion_recipe.engine_mode, authority) {
        (ConversionEngineMode::CustomOptimizer, ConversionJobAuthority::CustomOptimizer(evidence)) => {
            ConversionJobCapture::capture_custom_optimizer(
                source_project,
                source_project_path,
                source_project_file_sha256,
                source_face_path,
                source_snapshot_id,
                source_file_sha256,
                source_profile,
                conversion_recipe,
                evidence,
                output_policy,
                output_tiff_path,
                production_project_path,
                production_project_name,
                output_face_label,
            )
        }
        (ConversionEngineMode::CustomOptimizer, ConversionJobAuthority::Standard) => Err(
            "Custom Optimizer final conversion cannot be captured without immutable production evidence."
                .to_owned(),
        ),
        (
            ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink,
            ConversionJobAuthority::Standard,
        ) => ConversionJobCapture::capture(
            source_project,
            source_project_path,
            source_project_file_sha256,
            source_face_path,
            source_snapshot_id,
            source_file_sha256,
            source_profile,
            conversion_recipe,
            output_policy,
            output_tiff_path,
            production_project_path,
            production_project_name,
            output_face_label,
        ),
        (
            ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink,
            ConversionJobAuthority::CustomOptimizer(_),
        ) => Err(
            "ICC/DeviceLink final conversion cannot be captured with Custom Optimizer production evidence."
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_optimizer_requires_evidence_at_final_job_capture_boundary() {
        assert!(validate_authority_binding(ConversionEngineMode::CustomOptimizer, true).is_ok());
        let error = validate_authority_binding(ConversionEngineMode::CustomOptimizer, false)
            .unwrap_err();
        assert!(error.contains("requires immutable production evidence"));
    }

    #[test]
    fn standard_engines_reject_optimizer_evidence() {
        for mode in [ConversionEngineMode::Icc, ConversionEngineMode::DeviceLink] {
            assert!(validate_authority_binding(mode, false).is_ok());
            let error = validate_authority_binding(mode, true).unwrap_err();
            assert!(error.contains("cannot carry Custom Optimizer"));
        }
    }

    #[test]
    fn router_contains_no_production_eligibility_mint_or_boolean_bypass() {
        let source = include_str!("conversion_job_authority.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("ConversionJobCapture::capture_custom_optimizer"));
        assert!(runtime.contains("ConversionJobCapture::capture("));
        assert!(!runtime.contains("InverseLutProductionEligibility"));
        assert!(!runtime.contains("production_authorized: bool"));
        assert!(!runtime.contains("test_only"));
    }
}
