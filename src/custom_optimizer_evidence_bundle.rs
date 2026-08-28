use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_evidence::CapturedCustomOptimizerEvidence;
use crate::inverse_lut_threshold_set::InverseLutThresholdCalibrationObservation;

pub const CUSTOM_OPTIMIZER_EVIDENCE_BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const MAX_CUSTOM_OPTIMIZER_EVIDENCE_BUNDLE_BYTES: u64 = 16 * 1024 * 1024;

/// Selection-time transport for one exact Custom Optimizer recipe and the
/// immutable evidence locators captured for that recipe.
///
/// This bundle is deliberately not a production-authorization token. Loading it
/// validates only the serialized recipe/evidence bindings. Candidate Preview and
/// final conversion must still reopen every referenced artifact and independently
/// mint `InverseLutProductionEligibility` through the production evidence gate.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CustomOptimizerEvidenceBundle {
    pub schema_version: u32,
    pub recipe_sha256: String,
    pub recipe: ConversionRecipe,
    pub evidence: CapturedCustomOptimizerEvidence,
}

impl CustomOptimizerEvidenceBundle {
    pub fn new(
        recipe: ConversionRecipe,
        evidence: CapturedCustomOptimizerEvidence,
    ) -> Result<Self, Vec<String>> {
        let recipe_sha256 = recipe_sha256(&recipe)
            .map_err(|error| vec![format!("Cannot identify Custom Optimizer recipe: {error}")])?;
        let bundle = Self {
            schema_version: CUSTOM_OPTIMIZER_EVIDENCE_BUNDLE_SCHEMA_VERSION,
            recipe_sha256,
            recipe,
            evidence,
        };
        bundle.validate()?;
        Ok(bundle)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != CUSTOM_OPTIMIZER_EVIDENCE_BUNDLE_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported Custom Optimizer evidence-bundle schema {} (expected {}).",
                self.schema_version, CUSTOM_OPTIMIZER_EVIDENCE_BUNDLE_SCHEMA_VERSION
            ));
        }
        if self.recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            errors.push(
                "Custom Optimizer evidence bundle must contain a Custom Optimizer recipe."
                    .to_owned(),
            );
        }
        if let Err(mut recipe_errors) = self.recipe.validate() {
            errors.append(&mut recipe_errors);
        }
        let actual_recipe_sha256 = match recipe_sha256(&self.recipe) {
            Ok(actual) => {
                if actual != self.recipe_sha256 {
                    errors.push(format!(
                        "Custom Optimizer evidence bundle recipe SHA-256 mismatch: recorded {}, actual {}.",
                        self.recipe_sha256, actual
                    ));
                }
                Some(actual)
            }
            Err(error) => {
                errors.push(format!("Cannot identify bundled Custom Optimizer recipe: {error}"));
                None
            }
        };
        if let Err(mut evidence_errors) = self.evidence.validate() {
            errors.append(&mut evidence_errors);
        }

        match self.recipe.target.characterization_id.as_deref() {
            Some(target_id) if target_id == self.evidence.characterization_id => {}
            Some(target_id) => errors.push(format!(
                "Bundled recipe characterization {} does not match captured evidence {}.",
                target_id, self.evidence.characterization_id
            )),
            None => errors.push(
                "Bundled Custom Optimizer recipe has no target characterization ID.".to_owned(),
            ),
        }

        if let Some(recipe_sha256) = actual_recipe_sha256.as_deref() {
            let has_exact_observation = self
                .evidence
                .calibration_manifest
                .observations
                .iter()
                .any(|observation| {
                    observation_matches_capture(
                        observation,
                        recipe_sha256,
                        &self.evidence,
                    )
                });
            if !has_exact_observation {
                errors.push(
                    "Calibration manifest contains no observation binding the exact bundled recipe, LUT, validation report, and characterization."
                        .to_owned(),
                );
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn to_pretty_json(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }
}

pub fn load_custom_optimizer_evidence_bundle(
    path: &Path,
) -> Result<CustomOptimizerEvidenceBundle, String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "Cannot inspect Custom Optimizer evidence bundle {}: {error}",
            path.display()
        )
    })?;
    if metadata.len() > MAX_CUSTOM_OPTIMIZER_EVIDENCE_BUNDLE_BYTES {
        return Err(format!(
            "Custom Optimizer evidence bundle {} is {} bytes; maximum accepted size is {} bytes.",
            path.display(),
            metadata.len(),
            MAX_CUSTOM_OPTIMIZER_EVIDENCE_BUNDLE_BYTES
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "Cannot read Custom Optimizer evidence bundle {}: {error}",
            path.display()
        )
    })?;
    let bundle: CustomOptimizerEvidenceBundle = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Cannot parse Custom Optimizer evidence bundle {}: {error}",
            path.display()
        )
    })?;
    bundle.validate().map_err(|errors| errors.join("\n"))?;
    Ok(bundle)
}

fn observation_matches_capture(
    observation: &InverseLutThresholdCalibrationObservation,
    recipe_sha256: &str,
    evidence: &CapturedCustomOptimizerEvidence,
) -> bool {
    observation.recipe_sha256 == recipe_sha256
        && observation.characterization_id == evidence.characterization_id
        && observation.lut_identity_content_id == evidence.lut_identity_content_id
        && observation.validation_report_content_id == evidence.validation_report_content_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inverse_lut_threshold_set::InverseLutCalibrationSolverFamily;

    fn observation(
        recipe_sha256: &str,
        characterization_id: &str,
        lut_id: &str,
        validation_id: &str,
    ) -> InverseLutThresholdCalibrationObservation {
        InverseLutThresholdCalibrationObservation {
            solver_family: InverseLutCalibrationSolverFamily::IndependentV1,
            characterization_id: characterization_id.to_owned(),
            recipe_sha256: recipe_sha256.to_owned(),
            lut_identity_content_id: lut_id.to_owned(),
            validation_report_content_id: validation_id.to_owned(),
        }
    }

    #[test]
    fn observation_binding_requires_all_exact_authority_identities() {
        let source = include_str!("custom_optimizer_evidence_bundle.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        for required in [
            "observation.recipe_sha256 == recipe_sha256",
            "observation.characterization_id == evidence.characterization_id",
            "observation.lut_identity_content_id == evidence.lut_identity_content_id",
            "observation.validation_report_content_id == evidence.validation_report_content_id",
        ] {
            assert!(runtime.contains(required));
        }

        let exact = observation("a", "b", "c", "d");
        assert_eq!(exact.recipe_sha256, "a");
        assert_eq!(exact.characterization_id, "b");
        assert_eq!(exact.lut_identity_content_id, "c");
        assert_eq!(exact.validation_report_content_id, "d");
    }

    #[test]
    fn bundle_loader_never_claims_production_authority() {
        let source = include_str!("custom_optimizer_evidence_bundle.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("InverseLutProductionEligibility"));
        assert!(!runtime.contains("is_production_approved("));
        assert!(!runtime.contains("production_authorized: bool"));
        assert!(runtime.contains("self.evidence.validate()"));
        assert!(runtime.contains("recipe_sha256(&self.recipe)"));
    }
}
