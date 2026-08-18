use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inverse_lut_validation::InverseLutValidationPolicy;
use crate::production_colorimetry::ProductionPcsCompatibilityMethod;

pub const INVERSE_LUT_THRESHOLD_SET_SCHEMA_VERSION: u32 = 2;
pub const INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION: u32 = 2;
pub const INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION: u32 = 1;

/// No calibration approval is production-approved yet. #205 may add an exact,
/// code-reviewed approval content ID only after representative measured ceramic
/// D50/2° evidence has been reviewed. Keeping this list empty preserves the
/// production fail-closed barrier without creating a content-identity cycle.
const PRODUCTION_APPROVED_THRESHOLD_APPROVAL_IDS: &[&str] = &[];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutThresholdSetMethod {
    /// Current repository defaults. Useful for diagnostics and collecting
    /// calibration observations, but never production-authorizing by itself.
    ProvisionalV1,
    /// A numerical policy derived from representative measured ceramic fixtures
    /// in the ICC PCS D50/2° basis. Approval evidence is intentionally separate.
    MeasuredCeramicD50TwoDegreeV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutCalibrationSolverFamily {
    IndependentV1,
    PositiveContinuityV2,
}

/// Pure numerical threshold identity. It deliberately does not contain a
/// calibration-manifest ID: reports bind this ID, manifests bind report IDs, and
/// a separate approval record binds the threshold set to the manifest. This
/// one-way graph avoids the impossible threshold-set -> manifest -> report ->
/// threshold-set hash cycle.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutValidationThresholdSet {
    pub schema_version: u32,
    pub method: InverseLutThresholdSetMethod,
    pub policy: InverseLutValidationPolicy,
}

impl InverseLutValidationThresholdSet {
    pub fn provisional_v1() -> Self {
        Self {
            schema_version: INVERSE_LUT_THRESHOLD_SET_SCHEMA_VERSION,
            method: InverseLutThresholdSetMethod::ProvisionalV1,
            policy: InverseLutValidationPolicy::default(),
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
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        content_id(self)
    }
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
            (
                "lut_identity_content_id",
                self.lut_identity_content_id.as_str(),
            ),
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
/// reports reviewed for one exact numerical threshold set.
///
/// The manifest points downstream to report identities. Reports point only to the
/// already-stable threshold-set identity; the threshold set never points back to
/// this manifest. That makes every identity constructible and independently
/// verifiable.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutThresholdCalibrationManifest {
    pub schema_version: u32,
    pub pcs_method: ProductionPcsCompatibilityMethod,
    pub threshold_set_content_id: String,
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
                "Inverse-LUT threshold calibration requires ICC PCS Lab D50/2° compatibility."
                    .to_owned(),
            );
        }
        if !is_prefixed_sha256(&self.threshold_set_content_id) {
            errors.push(
                "Inverse-LUT threshold calibration manifest threshold-set ID must be canonical sha256:<hex>."
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

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        content_id(self)
    }
}

/// Explicit review/approval edge between one numerical threshold set and one
/// immutable measured calibration manifest. Production approval is based on the
/// content ID of this entire record, not on a mutable boolean inside JSON.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutThresholdCalibrationApproval {
    pub schema_version: u32,
    pub pcs_method: ProductionPcsCompatibilityMethod,
    pub threshold_set_content_id: String,
    pub calibration_manifest_content_id: String,
}

impl InverseLutThresholdCalibrationApproval {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT threshold calibration approval schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION
            ));
        }
        if self.pcs_method != ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1 {
            errors.push(
                "Inverse-LUT threshold calibration approval requires ICC PCS Lab D50/2°."
                    .to_owned(),
            );
        }
        for (name, value) in [
            (
                "threshold_set_content_id",
                self.threshold_set_content_id.as_str(),
            ),
            (
                "calibration_manifest_content_id",
                self.calibration_manifest_content_id.as_str(),
            ),
        ] {
            if !is_prefixed_sha256(value) {
                errors.push(format!(
                    "Inverse-LUT threshold calibration approval {name} must be canonical sha256:<hex>."
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        content_id(self)
    }

    pub fn validate_bindings(
        &self,
        threshold_set: &InverseLutValidationThresholdSet,
        manifest: &InverseLutThresholdCalibrationManifest,
    ) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if let Err(mut own) = self.validate() {
            errors.append(&mut own);
        }
        if let Err(mut threshold_errors) = threshold_set.validate() {
            errors.append(&mut threshold_errors);
        }
        if let Err(mut manifest_errors) = manifest.validate() {
            errors.append(&mut manifest_errors);
        }

        let threshold_set_id = threshold_set.content_id();
        let manifest_id = manifest.content_id();
        match threshold_set_id {
            Ok(ref actual) => {
                if threshold_set.method
                    != InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1
                {
                    errors.push(
                        "Only a measured ceramic D50/2° threshold set can receive production calibration approval."
                            .to_owned(),
                    );
                }
                if self.threshold_set_content_id != *actual {
                    errors.push(format!(
                        "Threshold calibration approval records threshold set {}, actual is {}.",
                        self.threshold_set_content_id, actual
                    ));
                }
                if manifest.threshold_set_content_id != *actual {
                    errors.push(format!(
                        "Threshold calibration manifest records threshold set {}, actual is {}.",
                        manifest.threshold_set_content_id, actual
                    ));
                }
            }
            Err(error) => errors.push(format!("Cannot identify threshold set: {error}")),
        }
        match manifest_id {
            Ok(ref actual) if self.calibration_manifest_content_id != *actual => {
                errors.push(format!(
                    "Threshold calibration approval records manifest {}, actual is {}.",
                    self.calibration_manifest_content_id, actual
                ))
            }
            Err(error) => errors.push(format!("Cannot identify calibration manifest: {error}")),
            _ => {}
        }
        if self.pcs_method != manifest.pcs_method {
            errors.push(
                "Threshold calibration approval PCS method does not match manifest.".to_owned(),
            );
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Production authorization is an explicit code-reviewed allowlist of exact
    /// approval content IDs. With the current empty list this always fails closed
    /// after all structural bindings are verified.
    pub fn is_production_approved(
        &self,
        threshold_set: &InverseLutValidationThresholdSet,
        manifest: &InverseLutThresholdCalibrationManifest,
    ) -> Result<bool, String> {
        self.validate_bindings(threshold_set, manifest)
            .map_err(|errors| errors.join("\n"))?;
        let id = self.content_id()?;
        Ok(PRODUCTION_APPROVED_THRESHOLD_APPROVAL_IDS
            .iter()
            .any(|approved| *approved == id))
    }
}

fn content_id<T: Serialize>(value: &T) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
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

    fn measured_set() -> InverseLutValidationThresholdSet {
        let mut set = InverseLutValidationThresholdSet::provisional_v1();
        set.method = InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1;
        set
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

    fn complete_manifest(
        threshold_set_content_id: String,
    ) -> InverseLutThresholdCalibrationManifest {
        InverseLutThresholdCalibrationManifest {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            threshold_set_content_id,
            observations: vec![
                observation(InverseLutCalibrationSolverFamily::IndependentV1, '4'),
                observation(InverseLutCalibrationSolverFamily::PositiveContinuityV2, '5'),
            ],
        }
    }

    #[test]
    fn provisional_set_identity_is_deterministic() {
        let first = InverseLutValidationThresholdSet::provisional_v1();
        let second = InverseLutValidationThresholdSet::provisional_v1();
        assert_eq!(first.content_id().unwrap(), second.content_id().unwrap());
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
    fn measured_set_identity_does_not_depend_on_downstream_manifest() {
        let measured = measured_set();
        assert!(measured.validate().is_ok());
        assert!(measured.content_id().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn calibration_manifest_requires_both_solver_families() {
        let measured = measured_set();
        let mut manifest = complete_manifest(measured.content_id().unwrap());
        manifest.observations.pop();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn calibration_manifest_identity_is_deterministic_and_binds_reports() {
        let measured = measured_set();
        let manifest = complete_manifest(measured.content_id().unwrap());
        let id = manifest.content_id().unwrap();
        let mut changed = manifest.clone();
        changed.observations[1].validation_report_content_id = prefixed('6');
        assert_ne!(changed.content_id().unwrap(), id);
    }

    #[test]
    fn duplicate_validation_reports_are_rejected() {
        let measured = measured_set();
        let mut manifest = complete_manifest(measured.content_id().unwrap());
        manifest.observations[1].validation_report_content_id = manifest.observations[0]
            .validation_report_content_id
            .clone();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn approval_binds_threshold_set_and_manifest_without_hash_cycle() {
        let measured = measured_set();
        let threshold_id = measured.content_id().unwrap();
        let manifest = complete_manifest(threshold_id.clone());
        let approval = InverseLutThresholdCalibrationApproval {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            threshold_set_content_id: threshold_id,
            calibration_manifest_content_id: manifest.content_id().unwrap(),
        };
        assert!(approval.validate_bindings(&measured, &manifest).is_ok());
        assert!(
            !approval
                .is_production_approved(&measured, &manifest)
                .unwrap()
        );
    }

    #[test]
    fn approval_rejects_stale_threshold_set_identity() {
        let measured = measured_set();
        let threshold_id = measured.content_id().unwrap();
        let manifest = complete_manifest(threshold_id);
        let approval = InverseLutThresholdCalibrationApproval {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            threshold_set_content_id: prefixed('9'),
            calibration_manifest_content_id: manifest.content_id().unwrap(),
        };
        assert!(approval.validate_bindings(&measured, &manifest).is_err());
    }
}
