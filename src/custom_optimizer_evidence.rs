use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;
use crate::device_characterization_model::{
    ForwardModelValidationPolicy, ValidatedLocalForwardModel,
};
use crate::device_characterization_package::{
    CharacterizationTargetError, ValidatedCharacterizationPackage, load_characterization_package,
    validate_characterization_for_target,
};
use crate::inverse_lut_artifact::{VerifiedInverseLutArtifact, load_inverse_lut_artifact};
use crate::inverse_lut_identity::InverseLutForwardModelMethod;
use crate::inverse_lut_production_eligibility::{
    InverseLutProductionEligibility, InverseLutProductionEligibilityError,
    validate_inverse_lut_production_eligibility,
};
use crate::inverse_lut_threshold_set::{
    InverseLutThresholdCalibrationApproval, InverseLutThresholdCalibrationManifest,
    InverseLutValidationThresholdSet,
};
use crate::inverse_lut_validation_artifact::{
    VerifiedInverseLutValidationArtifact, load_inverse_lut_validation_artifact,
};
use crate::production_colorimetry::{
    ProductionPcsCompatibilityError, ProductionPcsCompatibilityMethod,
    ValidatedProductionPcsCompatibility, validate_characterization_for_icc_pcs_lab,
};

pub const CUSTOM_OPTIMIZER_EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// Immutable job-capture references and content identities required to rebuild
/// Custom Optimizer production authorization at execution time.
///
/// Paths are locators only. Every authority-bearing identity is recomputed from
/// the reopened payload before production eligibility can be minted.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CapturedCustomOptimizerEvidence {
    pub schema_version: u32,
    pub lut_artifact_path: PathBuf,
    pub lut_identity_content_id: String,
    pub lut_payload_sha256: String,
    pub validation_artifact_path: PathBuf,
    pub validation_report_content_id: String,
    pub characterization_package_path: PathBuf,
    pub characterization_id: String,
    pub threshold_set: InverseLutValidationThresholdSet,
    pub threshold_set_content_id: String,
    pub calibration_manifest: InverseLutThresholdCalibrationManifest,
    pub calibration_manifest_content_id: String,
    pub calibration_approval: InverseLutThresholdCalibrationApproval,
    pub calibration_approval_content_id: String,
    pub pcs_compatibility_method: ProductionPcsCompatibilityMethod,
    pub pcs_compatibility_content_id: String,
}

impl CapturedCustomOptimizerEvidence {
    #[allow(clippy::too_many_arguments)]
    pub fn from_verified(
        lut_artifact_path: PathBuf,
        lut: &VerifiedInverseLutArtifact,
        validation_artifact_path: PathBuf,
        validation: &VerifiedInverseLutValidationArtifact,
        characterization_package_path: PathBuf,
        characterization: &ValidatedCharacterizationPackage,
        threshold_set: InverseLutValidationThresholdSet,
        calibration_manifest: InverseLutThresholdCalibrationManifest,
        calibration_approval: InverseLutThresholdCalibrationApproval,
        pcs_compatibility: &ValidatedProductionPcsCompatibility,
    ) -> Result<Self, Vec<String>> {
        let threshold_set_content_id = threshold_set
            .content_id()
            .map_err(|error| vec![format!("Cannot identify threshold set: {error}")])?;
        let calibration_manifest_content_id = calibration_manifest
            .content_id()
            .map_err(|error| vec![format!("Cannot identify calibration manifest: {error}")])?;
        let calibration_approval_content_id = calibration_approval
            .content_id()
            .map_err(|error| vec![format!("Cannot identify calibration approval: {error}")])?;
        let pcs_compatibility_content_id = pcs_compatibility
            .content_id()
            .map_err(|error| vec![format!("Cannot identify PCS compatibility: {error}")])?;
        let capture = Self {
            schema_version: CUSTOM_OPTIMIZER_EVIDENCE_SCHEMA_VERSION,
            lut_artifact_path,
            lut_identity_content_id: lut.identity_content_id.clone(),
            lut_payload_sha256: lut.payload_sha256.clone(),
            validation_artifact_path,
            validation_report_content_id: validation.report_content_id.clone(),
            characterization_package_path,
            characterization_id: characterization.identity().id.clone(),
            threshold_set,
            threshold_set_content_id,
            calibration_manifest,
            calibration_manifest_content_id,
            calibration_approval,
            calibration_approval_content_id,
            pcs_compatibility_method: pcs_compatibility.method,
            pcs_compatibility_content_id,
        };
        capture.validate()?;
        Ok(capture)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != CUSTOM_OPTIMIZER_EVIDENCE_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported Custom Optimizer evidence schema {} (expected {}).",
                self.schema_version, CUSTOM_OPTIMIZER_EVIDENCE_SCHEMA_VERSION
            ));
        }
        for (name, path) in [
            ("inverse-LUT artifact", self.lut_artifact_path.as_path()),
            (
                "inverse-LUT validation artifact",
                self.validation_artifact_path.as_path(),
            ),
            (
                "characterization package",
                self.characterization_package_path.as_path(),
            ),
        ] {
            if path.as_os_str().is_empty() {
                errors.push(format!("Captured {name} path cannot be empty."));
            }
        }
        for (name, value) in [
            (
                "lut_identity_content_id",
                self.lut_identity_content_id.as_str(),
            ),
            (
                "validation_report_content_id",
                self.validation_report_content_id.as_str(),
            ),
            ("characterization_id", self.characterization_id.as_str()),
            (
                "threshold_set_content_id",
                self.threshold_set_content_id.as_str(),
            ),
            (
                "calibration_manifest_content_id",
                self.calibration_manifest_content_id.as_str(),
            ),
            (
                "calibration_approval_content_id",
                self.calibration_approval_content_id.as_str(),
            ),
            (
                "pcs_compatibility_content_id",
                self.pcs_compatibility_content_id.as_str(),
            ),
        ] {
            if !is_prefixed_sha256(value) {
                errors.push(format!(
                    "Captured Custom Optimizer evidence {name} must be canonical sha256:<hex>."
                ));
            }
        }
        if !is_bare_sha256(&self.lut_payload_sha256) {
            errors.push(
                "Captured Custom Optimizer LUT payload SHA-256 must be canonical lowercase hex."
                    .to_owned(),
            );
        }
        if self.pcs_compatibility_method
            != ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1
        {
            errors.push(
                "Custom Optimizer production evidence requires ICC PCS Lab D50/2-degree compatibility."
                    .to_owned(),
            );
        }

        if let Err(mut own) = self.threshold_set.validate() {
            errors.append(&mut own);
        } else {
            match self.threshold_set.content_id() {
                Ok(actual) if actual != self.threshold_set_content_id => errors.push(format!(
                    "Captured threshold-set ID mismatch: recorded {}, actual {}.",
                    self.threshold_set_content_id, actual
                )),
                Err(error) => errors.push(format!("Cannot identify threshold set: {error}")),
                _ => {}
            }
        }
        if let Err(mut own) = self.calibration_manifest.validate() {
            errors.append(&mut own);
        } else {
            match self.calibration_manifest.content_id() {
                Ok(actual) if actual != self.calibration_manifest_content_id => {
                    errors.push(format!(
                        "Captured calibration-manifest ID mismatch: recorded {}, actual {}.",
                        self.calibration_manifest_content_id, actual
                    ))
                }
                Err(error) => {
                    errors.push(format!("Cannot identify calibration manifest: {error}"))
                }
                _ => {}
            }
        }
        if let Err(mut own) = self.calibration_approval.validate() {
            errors.append(&mut own);
        } else {
            match self.calibration_approval.content_id() {
                Ok(actual) if actual != self.calibration_approval_content_id => {
                    errors.push(format!(
                        "Captured calibration-approval ID mismatch: recorded {}, actual {}.",
                        self.calibration_approval_content_id, actual
                    ))
                }
                Err(error) => {
                    errors.push(format!("Cannot identify calibration approval: {error}"))
                }
                _ => {}
            }
        }
        if let Err(mut binding_errors) = self
            .calibration_approval
            .validate_bindings(&self.threshold_set, &self.calibration_manifest)
        {
            errors.append(&mut binding_errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoadedCustomOptimizerEvidence {
    pub lut: VerifiedInverseLutArtifact,
    pub validation: VerifiedInverseLutValidationArtifact,
    pub characterization: ValidatedCharacterizationPackage,
    pub model: ValidatedLocalForwardModel,
    pub pcs_compatibility: ValidatedProductionPcsCompatibility,
    pub eligibility: InverseLutProductionEligibility,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CustomOptimizerEvidenceError {
    InvalidCapture(Vec<String>),
    InvalidRecipe(Vec<String>),
    NotCustomOptimizerRecipe,
    LutLoad(String),
    LutIdentityMismatch { expected: String, actual: String },
    LutPayloadMismatch { expected: String, actual: String },
    LutRecipeMismatch { expected: String, actual: String },
    ValidationLoad(String),
    ValidationIdentityMismatch { expected: String, actual: String },
    CharacterizationLoad(String),
    CharacterizationIdentityMismatch { expected: String, actual: String },
    CharacterizationTarget(CharacterizationTargetError),
    PcsCompatibility(ProductionPcsCompatibilityError),
    PcsMethodMismatch {
        expected: ProductionPcsCompatibilityMethod,
        actual: ProductionPcsCompatibilityMethod,
    },
    PcsIdentity(String),
    PcsIdentityMismatch { expected: String, actual: String },
    UnsupportedForwardModel(InverseLutForwardModelMethod),
    ForwardModelBuild(Vec<String>),
    Authorization(InverseLutProductionEligibilityError),
}

/// Reopen and independently revalidate every authority-bearing Custom Optimizer
/// artifact captured by a queued job.
///
/// The exact local forward-model config is derived from the verified LUT identity
/// rather than accepted as a second free configuration source. The production
/// model is rebuilt from the reopened measured characterization using the frozen
/// production validation policy before #235's exact model binding is rechecked.
pub fn load_and_authorize_custom_optimizer_evidence(
    capture: &CapturedCustomOptimizerEvidence,
    recipe: &ConversionRecipe,
) -> Result<LoadedCustomOptimizerEvidence, CustomOptimizerEvidenceError> {
    capture
        .validate()
        .map_err(CustomOptimizerEvidenceError::InvalidCapture)?;
    recipe
        .validate()
        .map_err(CustomOptimizerEvidenceError::InvalidRecipe)?;
    if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return Err(CustomOptimizerEvidenceError::NotCustomOptimizerRecipe);
    }
    let actual_recipe_sha =
        recipe_sha256(recipe).map_err(|error| CustomOptimizerEvidenceError::InvalidRecipe(vec![error]))?;

    let lut = load_inverse_lut_artifact(&capture.lut_artifact_path)
        .map_err(CustomOptimizerEvidenceError::LutLoad)?;
    if lut.identity_content_id != capture.lut_identity_content_id {
        return Err(CustomOptimizerEvidenceError::LutIdentityMismatch {
            expected: capture.lut_identity_content_id.clone(),
            actual: lut.identity_content_id.clone(),
        });
    }
    if lut.payload_sha256 != capture.lut_payload_sha256 {
        return Err(CustomOptimizerEvidenceError::LutPayloadMismatch {
            expected: capture.lut_payload_sha256.clone(),
            actual: lut.payload_sha256.clone(),
        });
    }
    if lut.identity.recipe_sha256 != actual_recipe_sha {
        return Err(CustomOptimizerEvidenceError::LutRecipeMismatch {
            expected: actual_recipe_sha,
            actual: lut.identity.recipe_sha256.clone(),
        });
    }

    let validation = load_inverse_lut_validation_artifact(&capture.validation_artifact_path)
        .map_err(CustomOptimizerEvidenceError::ValidationLoad)?;
    if validation.report_content_id != capture.validation_report_content_id {
        return Err(CustomOptimizerEvidenceError::ValidationIdentityMismatch {
            expected: capture.validation_report_content_id.clone(),
            actual: validation.report_content_id.clone(),
        });
    }

    let characterization = load_characterization_package(&capture.characterization_package_path)
        .map_err(CustomOptimizerEvidenceError::CharacterizationLoad)?;
    if characterization.identity().id != capture.characterization_id {
        return Err(CustomOptimizerEvidenceError::CharacterizationIdentityMismatch {
            expected: capture.characterization_id.clone(),
            actual: characterization.identity().id.clone(),
        });
    }
    validate_characterization_for_target(characterization.package(), &recipe.target, true)
        .map_err(CustomOptimizerEvidenceError::CharacterizationTarget)?;

    let pcs_compatibility = validate_characterization_for_icc_pcs_lab(&characterization)
        .map_err(CustomOptimizerEvidenceError::PcsCompatibility)?;
    if pcs_compatibility.method != capture.pcs_compatibility_method {
        return Err(CustomOptimizerEvidenceError::PcsMethodMismatch {
            expected: capture.pcs_compatibility_method,
            actual: pcs_compatibility.method,
        });
    }
    let actual_pcs_id = pcs_compatibility
        .content_id()
        .map_err(CustomOptimizerEvidenceError::PcsIdentity)?;
    if actual_pcs_id != capture.pcs_compatibility_content_id {
        return Err(CustomOptimizerEvidenceError::PcsIdentityMismatch {
            expected: capture.pcs_compatibility_content_id.clone(),
            actual: actual_pcs_id,
        });
    }

    let model_config = match lut.identity.forward_model.method {
        InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1 => {
            lut.identity.forward_model.config.runtime_config()
        }
    };
    let model = ValidatedLocalForwardModel::build(
        &characterization,
        model_config,
        ForwardModelValidationPolicy::default(),
    )
    .map_err(CustomOptimizerEvidenceError::ForwardModelBuild)?;

    let eligibility = validate_inverse_lut_production_eligibility(
        &lut,
        &validation,
        &capture.threshold_set,
        &capture.calibration_manifest,
        &capture.calibration_approval,
        &pcs_compatibility,
        recipe,
        &model,
    )
    .map_err(CustomOptimizerEvidenceError::Authorization)?;

    Ok(LoadedCustomOptimizerEvidence {
        lut,
        validation,
        characterization,
        model,
        pcs_compatibility,
        eligibility,
    })
}

pub fn evidence_path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

fn is_prefixed_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(is_bare_sha256)
}

fn is_bare_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
