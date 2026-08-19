from pathlib import Path

EXPECTED_HEAD = "497caed0921ca9ef2abcbcb63d9611e29ad5846f"


def once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 occurrence, got {count}")
    return text.replace(old, new, 1)


# Child module of color_conversion is visible to both lib.rs and the binary's
# independent module tree, so main.rs does not need to change.
provenance_path = Path("src/color_conversion/production_provenance.rs")
provenance_path.parent.mkdir(parents=True, exist_ok=True)
provenance_path.write_text(
r'''use serde::{Deserialize, Serialize};

use super::{ConversionEngineMode, ProductionProvenance};
use crate::conversion_recipe::recipe_sha256;
use crate::production_colorimetry::ProductionPcsCompatibilityMethod;

pub const CUSTOM_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CustomOptimizerProductionProvenance {
    pub schema_version: u32,
    pub lut_identity_content_id: String,
    pub lut_payload_sha256: String,
    pub validation_report_content_id: String,
    pub characterization_id: String,
    pub threshold_set_content_id: String,
    pub calibration_manifest_content_id: String,
    pub calibration_approval_content_id: String,
    pub pcs_compatibility_method: ProductionPcsCompatibilityMethod,
    pub pcs_compatibility_content_id: String,
    pub conversion_recipe_sha256: String,
}

impl CustomOptimizerProductionProvenance {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != CUSTOM_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported Custom Optimizer production provenance schema {} (expected {}).",
                self.schema_version, CUSTOM_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION
            ));
        }
        for (name, value) in [
            ("lut_identity_content_id", self.lut_identity_content_id.as_str()),
            ("validation_report_content_id", self.validation_report_content_id.as_str()),
            ("characterization_id", self.characterization_id.as_str()),
            ("threshold_set_content_id", self.threshold_set_content_id.as_str()),
            ("calibration_manifest_content_id", self.calibration_manifest_content_id.as_str()),
            ("calibration_approval_content_id", self.calibration_approval_content_id.as_str()),
            ("pcs_compatibility_content_id", self.pcs_compatibility_content_id.as_str()),
        ] {
            if !is_prefixed_sha256(value) {
                errors.push(format!(
                    "Custom Optimizer production provenance {name} must be canonical sha256:<hex>."
                ));
            }
        }
        for (name, value) in [
            ("lut_payload_sha256", self.lut_payload_sha256.as_str()),
            ("conversion_recipe_sha256", self.conversion_recipe_sha256.as_str()),
        ] {
            if !is_bare_sha256(value) {
                errors.push(format!(
                    "Custom Optimizer production provenance {name} must be canonical lowercase SHA-256 hex."
                ));
            }
        }
        if self.pcs_compatibility_method
            != ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1
        {
            errors.push(
                "Custom Optimizer production provenance requires ICC PCS Lab D50/2-degree compatibility."
                    .to_owned(),
            );
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

pub fn validate_production_provenance(provenance: &ProductionProvenance) -> Result<(), String> {
    provenance
        .recipe
        .validate()
        .map_err(|errors| format!("Invalid production conversion recipe: {}", errors.join(" ")))?;
    let actual_recipe_sha = recipe_sha256(&provenance.recipe)?;
    match (provenance.recipe.engine_mode, provenance.custom_optimizer.as_ref()) {
        (ConversionEngineMode::CustomOptimizer, Some(custom)) => {
            custom.validate().map_err(|errors| {
                format!("Invalid Custom Optimizer production provenance: {}", errors.join(" "))
            })?;
            if custom.conversion_recipe_sha256 != actual_recipe_sha {
                return Err(format!(
                    "Custom Optimizer production provenance recipe SHA-256 mismatch: recorded {}, actual {}.",
                    custom.conversion_recipe_sha256, actual_recipe_sha
                ));
            }
            let target_characterization = provenance
                .recipe
                .target
                .characterization_id
                .as_deref()
                .unwrap_or_default();
            if custom.characterization_id != target_characterization {
                return Err(format!(
                    "Custom Optimizer production provenance characterization mismatch: recorded {}, recipe {}.",
                    custom.characterization_id, target_characterization
                ));
            }
        }
        (ConversionEngineMode::CustomOptimizer, None) => {
            return Err(
                "Custom Optimizer Production provenance requires immutable LUT/validation evidence identities."
                    .to_owned(),
            );
        }
        (_, Some(_)) => {
            return Err(
                "ICC/DeviceLink Production provenance cannot carry Custom Optimizer evidence identities."
                    .to_owned(),
            );
        }
        (_, None) => {}
    }
    Ok(())
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
    use std::path::Path;

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRecipe, ConversionRenderingIntent,
        ConversionSourceRef, ConversionTargetDefinition, SeparationStrategy,
        TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::model::IccProfileIdentity;
    use crate::production_project::{ProductionProjectSpec, build_production_project};

    fn prefixed(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn bare(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn custom_recipe() -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "fixture source".to_owned(),
                sha256: bare('1'),
            },
            target: ConversionTargetDefinition {
                name: "fixture target".to_owned(),
                channels: ["Cyan", "Magenta", "Yellow", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(1.0),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: None,
                device_link_path: None,
                characterization_id: Some(prefixed('2')),
                total_ink_limit: Some(1.8),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    fn custom_provenance(output: &Path) -> ProductionProvenance {
        let recipe = custom_recipe();
        let conversion_recipe_sha256 = recipe_sha256(&recipe).unwrap();
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: r"C:\Design\Source.shade".to_owned(),
                source_face_path: r"C:\Design\Face.tif".to_owned(),
                source_snapshot_id: Some(7),
                source_file_sha256: bare('3'),
            },
            recipe,
            custom_optimizer: Some(CustomOptimizerProductionProvenance {
                schema_version: CUSTOM_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION,
                lut_identity_content_id: prefixed('4'),
                lut_payload_sha256: bare('5'),
                validation_report_content_id: prefixed('6'),
                characterization_id: prefixed('2'),
                threshold_set_content_id: prefixed('7'),
                calibration_manifest_content_id: prefixed('8'),
                calibration_approval_content_id: prefixed('9'),
                pcs_compatibility_method:
                    ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
                pcs_compatibility_content_id: prefixed('a'),
                conversion_recipe_sha256,
            }),
            output_path: output.display().to_string(),
            output_sha256: bare('b'),
            converted_at_unix_ms: 1234,
        }
    }

    #[test]
    fn exact_optimizer_provenance_round_trips_and_is_retained_by_project() {
        let output = Path::new(r"C:\Production\Face_NInk.tif");
        let provenance = custom_provenance(output);
        validate_production_provenance(&provenance).unwrap();
        let bytes = serde_json::to_vec(&provenance).unwrap();
        let restored: ProductionProvenance = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored, provenance);
        let project = build_production_project(ProductionProjectSpec {
            project_name: "Production N-Ink",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: output,
            output_face_label: "Face N-Ink",
            provenance: restored.clone(),
        })
        .unwrap();
        assert_eq!(project.production_provenance[0], restored);
    }

    #[test]
    fn stale_recipe_and_characterization_bindings_are_rejected() {
        let output = Path::new(r"C:\Production\Face_NInk.tif");
        let mut stale_recipe = custom_provenance(output);
        stale_recipe
            .custom_optimizer
            .as_mut()
            .unwrap()
            .conversion_recipe_sha256 = bare('c');
        assert!(validate_production_provenance(&stale_recipe)
            .unwrap_err()
            .contains("recipe SHA-256 mismatch"));

        let mut stale_characterization = custom_provenance(output);
        stale_characterization
            .custom_optimizer
            .as_mut()
            .unwrap()
            .characterization_id = prefixed('d');
        assert!(validate_production_provenance(&stale_characterization)
            .unwrap_err()
            .contains("characterization mismatch"));
    }

    #[test]
    fn optimizer_requires_evidence_and_non_optimizer_cannot_carry_it() {
        let output = Path::new(r"C:\Production\Face_NInk.tif");
        let mut missing = custom_provenance(output);
        missing.custom_optimizer = None;
        assert!(validate_production_provenance(&missing)
            .unwrap_err()
            .contains("requires immutable"));

        let mut icc = custom_provenance(output);
        icc.recipe.engine_mode = ConversionEngineMode::Icc;
        icc.recipe.target.characterization_id = None;
        icc.recipe.target.output_profile_identity = Some(IccProfileIdentity {
            description: "fixture ICC target".to_owned(),
            sha256: bare('e'),
        });
        icc.recipe.target.output_profile_path = Some(r"C:\Color\Target.icc".to_owned());
        icc.recipe.custom_optimizer_solver = None;
        assert!(validate_production_provenance(&icc)
            .unwrap_err()
            .contains("cannot carry"));

        icc.custom_optimizer = None;
        validate_production_provenance(&icc).unwrap();
        let text = String::from_utf8(serde_json::to_vec(&icc).unwrap()).unwrap();
        assert!(!text.contains("custom_optimizer"));
        let restored: ProductionProvenance = serde_json::from_str(&text).unwrap();
        assert!(restored.custom_optimizer.is_none());
    }
}
''',
encoding="utf-8",
newline="\n",
)

# Parent module declaration + migration-safe optional field. No main.rs change.
p = Path("src/color_conversion.rs")
text = p.read_text(encoding="utf-8")
text = once(
    text,
    "use std::collections::{BTreeMap, BTreeSet};\n\n",
    "use std::collections::{BTreeMap, BTreeSet};\n\npub mod production_provenance;\n\n",
    "child provenance module declaration",
)
text = once(
    text,
    "    pub recipe: ConversionRecipe,\n    pub output_path: String,\n",
    "    pub recipe: ConversionRecipe,\n    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    pub custom_optimizer: Option<production_provenance::CustomOptimizerProductionProvenance>,\n    pub output_path: String,\n",
    "optional production provenance field",
)
p.write_text(text, encoding="utf-8", newline="\n")

# Captured evidence projects deterministically to the compact provenance record.
p = Path("src/custom_optimizer_evidence.rs")
text = p.read_text(encoding="utf-8")
text = once(
    text,
    "use crate::production_colorimetry::{\n",
    "use crate::color_conversion::production_provenance::{\n    CUSTOM_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION,\n    CustomOptimizerProductionProvenance,\n};\nuse crate::production_colorimetry::{\n",
    "evidence provenance import",
)
marker = "}\n\n#[derive(Clone, Debug)]\npub struct LoadedCustomOptimizerEvidence"
method = r'''

    pub fn production_provenance(
        &self,
        conversion_recipe_sha256: &str,
    ) -> Result<CustomOptimizerProductionProvenance, Vec<String>> {
        self.validate()?;
        if !is_bare_sha256(conversion_recipe_sha256) {
            return Err(vec![
                "Custom Optimizer production provenance requires a canonical captured recipe SHA-256."
                    .to_owned(),
            ]);
        }
        let provenance = CustomOptimizerProductionProvenance {
            schema_version: CUSTOM_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION,
            lut_identity_content_id: self.lut_identity_content_id.clone(),
            lut_payload_sha256: self.lut_payload_sha256.clone(),
            validation_report_content_id: self.validation_report_content_id.clone(),
            characterization_id: self.characterization_id.clone(),
            threshold_set_content_id: self.threshold_set_content_id.clone(),
            calibration_manifest_content_id: self.calibration_manifest_content_id.clone(),
            calibration_approval_content_id: self.calibration_approval_content_id.clone(),
            pcs_compatibility_method: self.pcs_compatibility_method,
            pcs_compatibility_content_id: self.pcs_compatibility_content_id.clone(),
            conversion_recipe_sha256: conversion_recipe_sha256.to_owned(),
        };
        provenance.validate()?;
        Ok(provenance)
    }
}

#[derive(Clone, Debug)]
pub struct LoadedCustomOptimizerEvidence'''
text = once(text, marker, method, "evidence provenance method")
p.write_text(text, encoding="utf-8", newline="\n")

# Transaction persists exact capture identities after the TIFF commit boundary.
p = Path("src/conversion_transaction.rs")
text = p.read_text(encoding="utf-8")
marker = "    let provenance = ProductionProvenance {\n"
addition = r'''    let custom_optimizer = match capture.custom_optimizer_evidence.as_ref() {
        Some(evidence) => match evidence.production_provenance(&capture.conversion_recipe_sha256) {
            Ok(provenance) => Some(provenance),
            Err(errors) => {
                return ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
                    committed_output,
                    production_project_path: capture.production_project_path.clone(),
                    production_project: None,
                    error: format!(
                        "Cannot persist Custom Optimizer production provenance: {}",
                        errors.join(" ")
                    ),
                };
            }
        },
        None => None,
    };

    let provenance = ProductionProvenance {
'''
text = once(text, marker, addition, "transaction provenance prelude")
text = once(
    text,
    "        recipe: capture.conversion_recipe.clone(),\n        output_path:",
    "        recipe: capture.conversion_recipe.clone(),\n        custom_optimizer,\n        output_path:",
    "transaction provenance field",
)
p.write_text(text, encoding="utf-8", newline="\n")

# Project construction validates the nested audit record without replacing the
# existing recipe validation. This is a one-line semantic addition.
p = Path("src/production_project.rs")
text = p.read_text(encoding="utf-8")
text = once(
    text,
    "use crate::model::{FaceRef, FaceStatus, ShadeProject};\n",
    "use crate::model::{FaceRef, FaceStatus, ShadeProject};\nuse crate::color_conversion::production_provenance::validate_production_provenance;\n",
    "project provenance import",
)
fn_marker = "pub fn build_production_project(spec: ProductionProjectSpec<'_>) -> Result<ShadeProject, String> {\n"
text = once(
    text,
    fn_marker,
    fn_marker + "    validate_production_provenance(&spec.provenance)?;\n",
    "project provenance validation call",
)
text = once(
    text,
    "            output_path: output.display().to_string(),\n",
    "            custom_optimizer: None,\n            output_path: output.display().to_string(),\n",
    "ICC fixture compatibility field",
)
p.write_text(text, encoding="utf-8", newline="\n")

# Existing file-backed evidence fixture proves exact capture -> audit identity copy.
p = Path("src/custom_optimizer_evidence_tests.rs")
text = p.read_text(encoding="utf-8")
text += r'''

#[test]
fn production_provenance_copies_exact_validated_capture_identities() {
    let fixture = file_fixture("production-provenance");
    let recipe_sha = recipe_sha256(&fixture.recipe).unwrap();
    let record = fixture.capture.production_provenance(&recipe_sha).unwrap();
    assert_eq!(record.lut_identity_content_id, fixture.capture.lut_identity_content_id);
    assert_eq!(record.lut_payload_sha256, fixture.capture.lut_payload_sha256);
    assert_eq!(record.validation_report_content_id, fixture.capture.validation_report_content_id);
    assert_eq!(record.characterization_id, fixture.capture.characterization_id);
    assert_eq!(record.threshold_set_content_id, fixture.capture.threshold_set_content_id);
    assert_eq!(record.calibration_manifest_content_id, fixture.capture.calibration_manifest_content_id);
    assert_eq!(record.calibration_approval_content_id, fixture.capture.calibration_approval_content_id);
    assert_eq!(record.pcs_compatibility_content_id, fixture.capture.pcs_compatibility_content_id);
    assert_eq!(record.conversion_recipe_sha256, recipe_sha);
    assert!(record.validate().is_ok());
}
'''
p.write_text(text, encoding="utf-8", newline="\n")
