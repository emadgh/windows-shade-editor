use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::device_characterization_package::ValidatedCharacterizationPackage;

pub const PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProductionPcsCompatibilityMethod {
    /// V1 accepts measured CIE Lab only when its declared colorimetric basis is
    /// D50 with the CIE 1931 2-degree standard observer, matching ICC PCS Lab.
    /// No chromatic adaptation is implied or performed.
    IccPcsLabD50TwoDegreeV1,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidatedProductionPcsCompatibility {
    pub schema_version: u32,
    pub method: ProductionPcsCompatibilityMethod,
    pub characterization_id: String,
    pub canonical_illuminant: String,
    pub canonical_observer: String,
}

impl ValidatedProductionPcsCompatibility {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported production PCS compatibility schema {} (expected {}).",
                self.schema_version, PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION
            ));
        }
        if !is_prefixed_sha256(&self.characterization_id) {
            errors.push(
                "Production PCS compatibility characterization ID must be canonical sha256:<hex>."
                    .to_owned(),
            );
        }
        match self.method {
            ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1 => {
                if self.canonical_illuminant != "D50" {
                    errors.push(
                        "ICC PCS Lab D50/2-degree compatibility must persist canonical illuminant D50."
                            .to_owned(),
                    );
                }
                if self.canonical_observer != "2deg" {
                    errors.push(
                        "ICC PCS Lab D50/2-degree compatibility must persist canonical observer 2deg."
                            .to_owned(),
                    );
                }
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
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProductionPcsCompatibilityError {
    UnsupportedIlluminant {
        declared: String,
        required: &'static str,
    },
    UnsupportedObserver {
        declared: String,
        required: &'static str,
    },
}

/// Validate that a measured characterization can be compared directly with the
/// D50 CIE Lab coordinates produced by the production Source-ICC -> PCS-Lab
/// transform.
///
/// This is intentionally a production-path gate rather than a generic package
/// validation rule. Characterizations measured under other conditions can still
/// exist for analysis, but they cannot feed the Custom Optimizer production path
/// until an explicit, separately versioned chromatic-adaptation method exists.
pub fn validate_characterization_for_icc_pcs_lab(
    package: &ValidatedCharacterizationPackage,
) -> Result<ValidatedProductionPcsCompatibility, ProductionPcsCompatibilityError> {
    let measurement = &package.package().payload.measurement;

    if !is_d50_illuminant(&measurement.illuminant) {
        return Err(ProductionPcsCompatibilityError::UnsupportedIlluminant {
            declared: measurement.illuminant.clone(),
            required: "D50",
        });
    }
    if !is_two_degree_observer(&measurement.observer) {
        return Err(ProductionPcsCompatibilityError::UnsupportedObserver {
            declared: measurement.observer.clone(),
            required: "CIE 1931 2-degree observer",
        });
    }

    let compatibility = ValidatedProductionPcsCompatibility {
        schema_version: PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION,
        method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
        characterization_id: package.identity().id.clone(),
        canonical_illuminant: "D50".to_owned(),
        canonical_observer: "2deg".to_owned(),
    };
    debug_assert!(compatibility.validate().is_ok());
    Ok(compatibility)
}

fn is_d50_illuminant(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("d50")
}

fn is_two_degree_observer(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "2deg" | "2°" | "2 degree" | "2 degrees" | "2-degree" | "2-deg"
    )
}

fn is_prefixed_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|bare| {
        bare.len() == 64
            && bare
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_characterization_package::{
        CharacterizationMeasurementMetadata, CharacterizationPackage, CharacterizationPayload,
        CharacterizationProductionContext, CharacterizationSample, CharacterizationValidationLevel,
        MeasuredLabColor,
    };

    fn payload(illuminant: &str, observer: &str) -> CharacterizationPayload {
        CharacterizationPayload {
            revision: "pcs-gate-fixture-v1".to_owned(),
            validation_level: CharacterizationValidationLevel::ProductionValidated,
            output_bit_depth: 16,
            channel_names: ["Cyan", "Magenta", "Yellow", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            measured_channel_max_coverage: vec![1.0; 4],
            measured_total_ink_limit: 4.0,
            production_context: CharacterizationProductionContext {
                machine_id: "fixture-machine".to_owned(),
                rip_name: "fixture-rip".to_owned(),
                rip_version: "1".to_owned(),
                linearization_id: "fixture-linearization".to_owned(),
                substrate: "fixture-substrate".to_owned(),
                glaze: None,
                body: None,
                product_family: None,
            },
            measurement: CharacterizationMeasurementMetadata {
                instrument_model: "fixture-spectrophotometer".to_owned(),
                instrument_serial: None,
                illuminant: illuminant.to_owned(),
                observer: observer.to_owned(),
                measurement_condition: "M1".to_owned(),
                measured_at_unix_ms: None,
                operator_or_lab: None,
            },
            samples: vec![
                sample([0.0, 0.0, 0.0, 0.0]),
                sample([0.5, 0.0, 0.0, 0.0]),
                sample([0.0, 0.5, 0.0, 0.0]),
                sample([0.0, 0.0, 0.5, 0.0]),
                sample([0.0, 0.0, 0.0, 0.5]),
            ],
        }
    }

    fn sample(coverages: [f32; 4]) -> CharacterizationSample {
        CharacterizationSample {
            coverages: coverages.to_vec(),
            lab: MeasuredLabColor {
                l: 50.0,
                a: 0.0,
                b: 0.0,
            },
        }
    }

    fn validated(illuminant: &str, observer: &str) -> ValidatedCharacterizationPackage {
        CharacterizationPackage::new(payload(illuminant, observer))
            .unwrap()
            .validated()
            .unwrap()
    }

    #[test]
    fn canonical_d50_two_degree_passes_and_binds_characterization_identity() {
        let package = validated("D50", "2deg");
        let result = validate_characterization_for_icc_pcs_lab(&package).unwrap();
        assert_eq!(
            result.method,
            ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1
        );
        assert_eq!(result.characterization_id, package.identity().id);
        assert_eq!(result.canonical_illuminant, "D50");
        assert_eq!(result.canonical_observer, "2deg");
        assert!(result.validate().is_ok());
        assert!(result.content_id().is_ok());
    }

    #[test]
    fn compatibility_content_identity_binds_method_and_characterization() {
        let package = validated("D50", "2deg");
        let base = validate_characterization_for_icc_pcs_lab(&package).unwrap();
        let base_id = base.content_id().unwrap();

        let mut changed = base.clone();
        changed.characterization_id = format!("sha256:{}", "a".repeat(64));
        assert_ne!(changed.content_id().unwrap(), base_id);

        let mut invalid = base;
        invalid.canonical_observer = "10deg".to_owned();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn d50_case_and_whitespace_normalization_is_explicit() {
        let package = validated("  d50  ", "  2DEG ");
        assert!(validate_characterization_for_icc_pcs_lab(&package).is_ok());
    }

    #[test]
    fn accepted_two_degree_aliases_are_frozen_by_v1_tests() {
        for observer in ["2deg", "2°", "2 degree", "2 degrees", "2-degree", "2-deg"] {
            let package = validated("D50", observer);
            assert!(
                validate_characterization_for_icc_pcs_lab(&package).is_ok(),
                "observer alias {observer:?} should be accepted"
            );
        }
    }

    #[test]
    fn d65_fails_before_production_lut_execution() {
        let package = validated("D65", "2deg");
        assert_eq!(
            validate_characterization_for_icc_pcs_lab(&package),
            Err(ProductionPcsCompatibilityError::UnsupportedIlluminant {
                declared: "D65".to_owned(),
                required: "D50",
            })
        );
    }

    #[test]
    fn ten_degree_observer_fails_before_production_lut_execution() {
        let package = validated("D50", "10deg");
        assert!(matches!(
            validate_characterization_for_icc_pcs_lab(&package),
            Err(ProductionPcsCompatibilityError::UnsupportedObserver { .. })
        ));
    }

    #[test]
    fn unknown_aliases_fail_instead_of_being_inferred() {
        for (illuminant, observer) in [
            ("daylight", "2deg"),
            ("D50 adapted", "2deg"),
            ("D50", "standard observer"),
            ("D50", "1931"),
        ] {
            let package = validated(illuminant, observer);
            assert!(validate_characterization_for_icc_pcs_lab(&package).is_err());
        }
    }
}
