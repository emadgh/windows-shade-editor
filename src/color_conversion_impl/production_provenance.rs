use serde::{Deserialize, Serialize};

use super::{ConversionEngineMode, ProductionProvenance};
use sha2::{Digest, Sha256};

pub const CUSTOM_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CustomOptimizerProductionPcsMethod {
    IccPcsLabD50TwoDegreeV1,
}

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
    pub pcs_compatibility_method: CustomOptimizerProductionPcsMethod,
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
                    "Custom Optimizer production provenance {name} must be canonical sha256:<hex>."
                ));
            }
        }
        for (name, value) in [
            ("lut_payload_sha256", self.lut_payload_sha256.as_str()),
            (
                "conversion_recipe_sha256",
                self.conversion_recipe_sha256.as_str(),
            ),
        ] {
            if !is_bare_sha256(value) {
                errors.push(format!(
                    "Custom Optimizer production provenance {name} must be canonical lowercase SHA-256 hex."
                ));
            }
        }
        if self.pcs_compatibility_method
            != CustomOptimizerProductionPcsMethod::IccPcsLabD50TwoDegreeV1
        {
            errors.push(
                "Custom Optimizer production provenance requires ICC PCS Lab D50/2-degree compatibility."
                    .to_owned(),
            );
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn recipe_sha256(recipe: &super::ConversionRecipe) -> Result<String, String> {
    let bytes = serde_json::to_vec(recipe).map_err(|error| {
        format!("Cannot serialize conversion recipe for fingerprinting: {error}")
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn validate_production_provenance(provenance: &ProductionProvenance) -> Result<(), String> {
    provenance
        .recipe
        .validate()
        .map_err(|errors| format!("Invalid production conversion recipe: {}", errors.join(" ")))?;
    let actual_recipe_sha = recipe_sha256(&provenance.recipe)?;
    match (
        provenance.recipe.engine_mode,
        provenance.custom_optimizer.as_ref(),
        provenance.profile_backed_optimizer.as_ref(),
    ) {
        (ConversionEngineMode::CustomOptimizer, Some(custom), None) => {
            custom.validate().map_err(|errors| {
                format!(
                    "Invalid measured Custom Optimizer production provenance: {}",
                    errors.join(" ")
                )
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
        (ConversionEngineMode::CustomOptimizer, None, Some(profile)) => {
            profile.validate().map_err(|errors| {
                format!(
                    "Invalid profile-backed Custom Optimizer production provenance: {}",
                    errors.join(" ")
                )
            })?;
            if profile.conversion_recipe_sha256 != actual_recipe_sha {
                return Err(format!(
                    "Profile-backed Custom Optimizer production provenance recipe SHA-256 mismatch: recorded {}, actual {}.",
                    profile.conversion_recipe_sha256, actual_recipe_sha
                ));
            }
            if provenance
                .recipe
                .target
                .characterization_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Err(
                    "Profile-backed Custom Optimizer production provenance cannot authorize a measured-characterization recipe."
                        .to_owned(),
                );
            }
            let target = &provenance.recipe.target;
            let output_identity = target.output_profile_identity.as_ref().ok_or_else(|| {
                "Profile-backed Custom Optimizer production provenance requires Output ICC identity."
                    .to_owned()
            })?;
            let output_path = target.output_profile_path.as_deref().ok_or_else(|| {
                "Profile-backed Custom Optimizer production provenance requires Output ICC path."
                    .to_owned()
            })?;
            if output_identity.sha256.trim() != profile.output_profile_sha256
                || output_path != profile.output_profile_path
            {
                return Err(
                    "Profile-backed Custom Optimizer Output ICC provenance does not match the exact recipe."
                        .to_owned(),
                );
            }
            let recipe_channels = target
                .channels
                .iter()
                .map(|channel| channel.name.as_str())
                .collect::<Vec<_>>();
            if recipe_channels.len() != profile.channel_names.len()
                || !recipe_channels
                    .iter()
                    .zip(profile.channel_names.iter())
                    .all(|(left, right)| *left == right)
                || target.bit_depth != profile.target_bit_depth
            {
                return Err(
                    "Profile-backed Custom Optimizer topology/bit-depth provenance does not match the exact recipe."
                        .to_owned(),
                );
            }
        }
        (ConversionEngineMode::CustomOptimizer, Some(_), Some(_)) => {
            return Err(
                "Custom Optimizer Production provenance must carry exactly one authority: measured or profile-backed."
                    .to_owned(),
            );
        }
        (ConversionEngineMode::CustomOptimizer, None, None) => {
            return Err(
                "Custom Optimizer Production provenance requires immutable measured or profile-backed authority identities."
                    .to_owned(),
            );
        }
        (_, Some(_), _) | (_, _, Some(_)) => {
            return Err(
                "ICC/DeviceLink Production provenance cannot carry Custom Optimizer authority identities."
                    .to_owned(),
            );
        }
        (_, None, None) => {}
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
            source_transparency_policy: None,
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
                    CustomOptimizerProductionPcsMethod::IccPcsLabD50TwoDegreeV1,
                pcs_compatibility_content_id: prefixed('a'),
                conversion_recipe_sha256,
            }),
            profile_backed_optimizer: None,
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
        assert!(
            validate_production_provenance(&stale_recipe)
                .unwrap_err()
                .contains("recipe SHA-256 mismatch")
        );

        let mut stale_characterization = custom_provenance(output);
        stale_characterization
            .custom_optimizer
            .as_mut()
            .unwrap()
            .characterization_id = prefixed('d');
        assert!(
            validate_production_provenance(&stale_characterization)
                .unwrap_err()
                .contains("characterization mismatch")
        );
    }

    #[test]
    fn optimizer_requires_one_authority_and_non_optimizer_cannot_carry_it() {
        let output = Path::new(r"C:\Production\Face_NInk.tif");
        let mut missing = custom_provenance(output);
        missing.custom_optimizer = None;
        assert!(
            validate_production_provenance(&missing)
                .unwrap_err()
                .contains("requires immutable")
        );

        let mut icc = custom_provenance(output);
        icc.recipe.engine_mode = ConversionEngineMode::Icc;
        icc.recipe.target.characterization_id = None;
        icc.recipe.target.output_profile_identity = Some(IccProfileIdentity {
            description: "fixture ICC target".to_owned(),
            sha256: bare('e'),
        });
        icc.recipe.target.output_profile_path = Some(r"C:\Color\Target.icc".to_owned());
        icc.recipe.custom_optimizer_solver = None;
        assert!(
            validate_production_provenance(&icc)
                .unwrap_err()
                .contains("cannot carry")
        );

        icc.custom_optimizer = None;
        validate_production_provenance(&icc).unwrap();
        let text = String::from_utf8(serde_json::to_vec(&icc).unwrap()).unwrap();
        assert!(!text.contains("custom_optimizer"));
        assert!(!text.contains("profile_backed_optimizer"));
        let restored: ProductionProvenance = serde_json::from_str(&text).unwrap();
        assert!(restored.custom_optimizer.is_none());
        assert!(restored.profile_backed_optimizer.is_none());
    }
}
