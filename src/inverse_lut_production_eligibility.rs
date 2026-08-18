use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::color_conversion::ConversionRecipe;
use crate::conversion_recipe::recipe_sha256;
use crate::device_characterization::DeviceForwardModel;
use crate::inverse_lut_artifact::VerifiedInverseLutArtifact;
use crate::inverse_lut_runtime::{InverseLutLookupError, InverseLutRuntime};
use crate::inverse_lut_validation_artifact::VerifiedInverseLutValidationArtifact;

pub const INVERSE_LUT_PRODUCTION_ELIGIBILITY_SCHEMA_VERSION: u32 = 1;

/// Immutable evidence that an exact inverse LUT has a passing validation report
/// for the exact recipe and measured characterization expected by production.
/// #191 should require this evidence rather than accepting a LUT path/ID alone.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutProductionEligibility {
    pub schema_version: u32,
    pub lut_identity_content_id: String,
    pub lut_payload_sha256: String,
    pub validation_report_content_id: String,
    pub recipe_sha256: String,
    pub characterization_id: String,
}

impl InverseLutProductionEligibility {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_PRODUCTION_ELIGIBILITY_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT production-eligibility schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_PRODUCTION_ELIGIBILITY_SCHEMA_VERSION
            ));
        }
        for (name, value) in [
            ("lut_identity_content_id", self.lut_identity_content_id.as_str()),
            (
                "validation_report_content_id",
                self.validation_report_content_id.as_str(),
            ),
            ("characterization_id", self.characterization_id.as_str()),
        ] {
            if !is_prefixed_sha256(value) {
                errors.push(format!(
                    "Inverse-LUT production eligibility {name} must be canonical sha256:<hex>."
                ));
            }
        }
        for (name, value) in [
            ("lut_payload_sha256", self.lut_payload_sha256.as_str()),
            ("recipe_sha256", self.recipe_sha256.as_str()),
        ] {
            if !is_bare_sha256(value) {
                errors.push(format!(
                    "Inverse-LUT production eligibility {name} must be canonical lowercase SHA-256 hex."
                ));
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum InverseLutProductionEligibilityError {
    InvalidLut(InverseLutLookupError),
    InvalidRecipe(Vec<String>),
    RecipeIdentity(String),
    InvalidValidationReport(Vec<String>),
    ValidationArtifactIdentity(String),
    ValidationFailed,
    LutIdentityMismatch { report: String, lut: String },
    LutPayloadMismatch { report: String, lut: String },
    RecipeMismatch { report: String, actual: String },
    LutRecipeMismatch { lut: String, actual: String },
    CharacterizationMismatch {
        report: String,
        lut: String,
        model: String,
    },
    ChannelTopologyMismatch {
        lut: Vec<String>,
        model: Vec<String>,
    },
}

/// Revalidate every production-critical binding and mint eligibility evidence.
///
/// This deliberately accepts verified artifact values instead of loose IDs. The
/// LUT payload digest is rechecked by `InverseLutRuntime::from_verified`, while
/// the validation report content ID is recomputed from all report metrics and
/// ordered path evidence. A stale passing report from another LUT/recipe/device
/// therefore cannot authorize raster conversion.
pub fn validate_inverse_lut_production_eligibility(
    lut: &VerifiedInverseLutArtifact,
    validation: &VerifiedInverseLutValidationArtifact,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
) -> Result<InverseLutProductionEligibility, InverseLutProductionEligibilityError> {
    let runtime = InverseLutRuntime::from_verified(lut.clone())
        .map_err(InverseLutProductionEligibilityError::InvalidLut)?;
    recipe
        .validate()
        .map_err(InverseLutProductionEligibilityError::InvalidRecipe)?;
    validation
        .report
        .validate()
        .map_err(InverseLutProductionEligibilityError::InvalidValidationReport)?;
    let actual_report_id = validation
        .report
        .content_id()
        .map_err(InverseLutProductionEligibilityError::ValidationArtifactIdentity)?;
    if validation.report_content_id != actual_report_id {
        return Err(InverseLutProductionEligibilityError::ValidationArtifactIdentity(
            format!(
                "Validation artifact records {}, but report recomputes to {}.",
                validation.report_content_id, actual_report_id
            ),
        ));
    }
    if !validation.report.passed {
        return Err(InverseLutProductionEligibilityError::ValidationFailed);
    }

    let runtime_identity_id = runtime.identity_content_id().to_owned();
    if validation.report.lut_identity_content_id != runtime_identity_id {
        return Err(InverseLutProductionEligibilityError::LutIdentityMismatch {
            report: validation.report.lut_identity_content_id.clone(),
            lut: runtime_identity_id,
        });
    }
    if validation.report.lut_payload_sha256 != lut.payload_sha256 {
        return Err(InverseLutProductionEligibilityError::LutPayloadMismatch {
            report: validation.report.lut_payload_sha256.clone(),
            lut: lut.payload_sha256.clone(),
        });
    }

    let actual_recipe_sha = recipe_sha256(recipe)
        .map_err(InverseLutProductionEligibilityError::RecipeIdentity)?;
    if validation.report.recipe_sha256 != actual_recipe_sha {
        return Err(InverseLutProductionEligibilityError::RecipeMismatch {
            report: validation.report.recipe_sha256.clone(),
            actual: actual_recipe_sha,
        });
    }
    if runtime.identity().recipe_sha256 != actual_recipe_sha {
        return Err(InverseLutProductionEligibilityError::LutRecipeMismatch {
            lut: runtime.identity().recipe_sha256.clone(),
            actual: actual_recipe_sha,
        });
    }

    let model_characterization = model.identity().id.clone();
    if validation.report.characterization_id != runtime.identity().characterization_id
        || validation.report.characterization_id != model_characterization
    {
        return Err(InverseLutProductionEligibilityError::CharacterizationMismatch {
            report: validation.report.characterization_id.clone(),
            lut: runtime.identity().characterization_id.clone(),
            model: model_characterization,
        });
    }
    if runtime.identity().channel_names != model.identity().channel_names {
        return Err(InverseLutProductionEligibilityError::ChannelTopologyMismatch {
            lut: runtime.identity().channel_names.clone(),
            model: model.identity().channel_names.clone(),
        });
    }

    let evidence = InverseLutProductionEligibility {
        schema_version: INVERSE_LUT_PRODUCTION_ELIGIBILITY_SCHEMA_VERSION,
        lut_identity_content_id: runtime.identity_content_id().to_owned(),
        lut_payload_sha256: lut.payload_sha256.clone(),
        validation_report_content_id: actual_report_id,
        recipe_sha256: actual_recipe_sha,
        characterization_id: validation.report.characterization_id.clone(),
    };
    debug_assert!(evidence.validate().is_ok());
    Ok(evidence)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility_content_identity_binds_validation_report_identity() {
        let base = InverseLutProductionEligibility {
            schema_version: INVERSE_LUT_PRODUCTION_ELIGIBILITY_SCHEMA_VERSION,
            lut_identity_content_id: format!("sha256:{}", "1".repeat(64)),
            lut_payload_sha256: "2".repeat(64),
            validation_report_content_id: format!("sha256:{}", "3".repeat(64)),
            recipe_sha256: "4".repeat(64),
            characterization_id: format!("sha256:{}", "5".repeat(64)),
        };
        let base_id = base.content_id().unwrap();
        let mut changed = base.clone();
        changed.validation_report_content_id = format!("sha256:{}", "6".repeat(64));
        assert_ne!(changed.content_id().unwrap(), base_id);
    }

    #[test]
    fn eligibility_schema_rejects_noncanonical_id_forms() {
        let bad = InverseLutProductionEligibility {
            schema_version: INVERSE_LUT_PRODUCTION_ELIGIBILITY_SCHEMA_VERSION,
            lut_identity_content_id: "not-a-sha".to_owned(),
            lut_payload_sha256: "2".repeat(64),
            validation_report_content_id: format!("sha256:{}", "3".repeat(64)),
            recipe_sha256: "4".repeat(64),
            characterization_id: format!("sha256:{}", "5".repeat(64)),
        };
        assert!(bad.validate().is_err());
    }
}
