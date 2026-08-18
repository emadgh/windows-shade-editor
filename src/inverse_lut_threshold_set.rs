use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inverse_lut_validation::InverseLutValidationPolicy;
use crate::production_colorimetry::ProductionPcsCompatibilityMethod;

pub const INVERSE_LUT_THRESHOLD_SET_SCHEMA_VERSION: u32 = 1;
pub const INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// No threshold-set ID is production approved yet. #205 must add an exact
/// measured-fixture-derived content ID here only after calibration evidence is
/// reviewed. Keeping this list empty preserves the #192 fail-closed barrier.
const PRODUCTION_APPROVED_THRESHOLD_SET_IDS: &[&str] = &[];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutThresholdSetMethod {
    /// Current repository defaults. Useful for diagnostics and collecting
    /// calibration observations, but never production-authorizing.
    ProvisionalV1,
    /// A future threshold set derived from representative measured ceramic
    /// characterization/validation fixtures in the ICC PCS D50/2° basis.
    MeasuredCeramicD50TwoDegreeV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutCalibrationSolverFamily {
    IndependentV1,
    PositiveContinuityV2,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutThresholdCalibrationObservation {
    pub solver_family: InverseLutCalibrationSolverFamily,
    pub characterization_id: String,
    pub recipe_sha256: String,
    pub lut_identity_content_id: String,
    pub validation_report_content_id: String,
}

impl InverseLutThresholdCalibrationObservation {
    fn validate(&self, index: usize) -> Result<(), String> {
        for (name, value) in [
            ("characterization_id", self.characterization_id.as_str()),
            ("lut_identity_content_id", self.lut_identity_content_id.as_str()),
            (
                "validation_report_content_id",
                self.validation_report_content_id.as_str(),
            ),
        ] {
            if !is_prefixed_sha256(value) {
                return Err(format!(
                    "Threshold calibration observation {index} {name} must be canonical sha256:<hex>."
                ));
            }
        }
        if !is_bare_sha256(&self.recipe_sha256) {
            return Err(format!(
                "Threshold calibration observation {index} recipe_sha256 must be canonical lowercase SHA-256 hex."
            ));
        }
        Ok(())
    }
}

/// Content-addressed evidence manifest describing the exact measured validation
/// reports reviewed when a production threshold set is frozen.
///
/// This deliberately stores identities, not mutable file paths. It also requires
/// observations from both independent V1 and positive-continuity V2 so a future
/// production approval cannot silently calibrate only one solver family.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutThresholdCalibrationManifest {
    pub schema_version: u32,
    pub pcs_method: ProductionPcsCompatibilityMethod,
    pub observations: Vec<InverseLutThresholdCalibrationObservation>,
}

impl InverseLutThresholdCalibrationManifest {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT threshold calibration manifest schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION
            ));
        }
        if self.pcs_method != ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1 {
            errors.push(
                "Inverse-LUT threshold calibration V1 requires ICC PCS Lab D50/2° compatibility."
                    .to_owned(),
            );
        }
        if self.observations.is_empty() {
            errors.push("Inverse-LUT threshold calibration requires observations.".to_owned());
        }

        let mut report_ids = BTreeSet::new();
        let mut solver_families = BTreeSet::new();
        for (index, observation) in self.observations.iter().enumerate() {
            if let Err(error) = observation.validate(index) {
                errors.push(error);
            }
            if !report_ids.insert(observation.validation_report_content_id.as_str()) {
                errors.push(format!(
                    "Threshold calibration observation {index} duplicates validation report {}.",
                    observation.validation_report_content_id
                ));
            }
            solver_families.insert(observation.solver_family);
        }
        for required in [
            InverseLutCalibrationSolverFamily::IndependentV1,
            InverseLutCalibrationSolverFamily::PositiveContinuityV2,
        ] {
            if !solver_families.contains(&required) {
                errors.push(format!(
                    "Inverse-LUT threshold calibration manifest is missing required solver family {required:?}."
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutValidationThresholdSet {
    pub schema_version: u32,
    pub method: InverseLutThresholdSetMethod,
    pub policy: InverseLutValidationPolicy,
    /// Required only for measured production-candidate sets. This is the exact
    /// content ID of `InverseLutThresholdCalibrationManifest` reviewed for the set.
    pub calibration_manifest_content_id: Option<String>,
}

impl InverseLutValidationThresholdSet {
    pub fn provisional_v1() -> Self {
        Self {
            schema_version: INVERSE_LUT_THRESHOLD_SET_SCHEMA_VERSION,
            method: InverseLutThresholdSetMethod::ProvisionalV1,
            policy: InverseLutValidationPolicy::default(),
            calibration_manifest_content_id: None,
        }
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_THRESHOLD_SET_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT threshold-set schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_THRESHOLD_SET_SCHEMA_VERSION
            ));
        }
        if let Err(policy_errors) = self.policy.validate() {
            errors.extend(policy_errors);
        }
        match self.method {
            InverseLutThresholdSetMethod::ProvisionalV1 => {
                if self.calibration_manifest_content_id.is_some() {
                    errors.push(
                        "Provisional inverse-LUT threshold set must not claim calibration evidence."
                            .to_owned(),
                    );
                }
            }
            InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1 => {
                match self.calibration_manifest_content_id.as_deref() {
                    Some(value) if is_prefixed_sha256(value) => {}
                    _ => errors.push(
                        "Measured inverse-LUT threshold set requires a canonical calibration manifest content ID."
                            .to_owned(),
                    ),
                }
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    /// Production authorization is an explicit code-reviewed allowlist of exact
    /// threshold-set content IDs, never a boolean embedded in mutable JSON.
    pub fn is_production_approved(&self) -> Result<bool, String> {
        let id = self.content_id()?;
        Ok(PRODUCTION_APPROVED_THRESHOLD_SET_IDS
            .iter()
            .any(|approved| *approved == id))
    }
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

    fn prefixed(hex: char) -> String {
        format!("sha256:{}", hex.to_string().repeat(64))
    }

    fn bare(hex: char) -> String {
        hex.to_string().repeat(64)
    }

    fn observation(
        family: InverseLutCalibrationSolverFamily,
        report_hex: char,
    ) -> InverseLutThresholdCalibrationObservation {
        InverseLutThresholdCalibrationObservation {
            solver_family: family,
            characterization_id: prefixed('1'),
            recipe_sha256: bare('2'),
            lut_identity_content_id: prefixed('3'),
            validation_report_content_id: prefixed(report_hex),
        }
    }

    fn complete_manifest() -> InverseLutThresholdCalibrationManifest {
        InverseLutThresholdCalibrationManifest {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            observations: vec![
                observation(InverseLutCalibrationSolverFamily::IndependentV1, '4'),
                observation(InverseLutCalibrationSolverFamily::PositiveContinuityV2, '5'),
            ],
        }
    }

    #[test]
    fn provisional_set_is_deterministic_and_never_implicitly_approved() {
        let first = InverseLutValidationThresholdSet::provisional_v1();
        let second = InverseLutValidationThresholdSet::provisional_v1();
        assert_eq!(first.content_id().unwrap(), second.content_id().unwrap());
        assert!(!first.is_production_approved().unwrap());
    }

    #[test]
    fn threshold_set_identity_changes_when_policy_changes() {
        let base = InverseLutValidationThresholdSet::provisional_v1();
        let base_id = base.content_id().unwrap();
        let mut changed = base.clone();
        changed.policy.max_delta_e00 += 0.25;
        assert_ne!(changed.content_id().unwrap(), base_id);
    }

    #[test]
    fn measured_candidate_requires_calibration_manifest_identity() {
        let mut candidate = InverseLutValidationThresholdSet::provisional_v1();
        candidate.method = InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1;
        assert!(candidate.validate().is_err());
        candidate.calibration_manifest_content_id = Some(prefixed('6'));
        assert!(candidate.validate().is_ok());
        assert!(!candidate.is_production_approved().unwrap());
    }

    #[test]
    fn calibration_manifest_requires_both_solver_families() {
        let mut manifest = complete_manifest();
        manifest.observations.pop();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn calibration_manifest_identity_is_deterministic_and_binds_reports() {
        let manifest = complete_manifest();
        let id = manifest.content_id().unwrap();
        let mut changed = manifest.clone();
        changed.observations[1].validation_report_content_id = prefixed('6');
        assert_ne!(changed.content_id().unwrap(), id);
    }

    #[test]
    fn duplicate_validation_reports_are_rejected() {
        let mut manifest = complete_manifest();
        manifest.observations[1].validation_report_content_id =
            manifest.observations[0].validation_report_content_id.clone();
        assert!(manifest.validate().is_err());
    }
}
