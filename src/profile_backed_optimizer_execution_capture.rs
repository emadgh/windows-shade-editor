use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::color_conversion::ConversionRecipe;
use crate::conversion_recipe::recipe_sha256;
use crate::profile_backed_inverse_lut_artifact::ProfileBackedInverseLutArtifact;
use crate::profile_backed_optimizer_authority::{
    PROFILE_BACKED_OPTIMIZER_AUTHORITY_SCHEMA_VERSION, ProfileBackedExecutionForwardModelMethod,
    ProfileBackedOptimizerAuthority,
};

pub const PROFILE_BACKED_OPTIMIZER_EXECUTION_CAPTURE_SCHEMA_VERSION: u32 = 1;
pub const PROFILE_BACKED_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION: u32 = 1;

/// Immutable locator + content identities captured for one profile-backed production job.
///
/// The LUT bytes are deliberately not serialized into the queued job. The filesystem worker
/// must reopen `lut_artifact_path`, recompute its content identities and independently authorize
/// it against `authority`, the exact recipe and the reopened Output ICC before rendering.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CapturedProfileBackedOptimizerExecution {
    pub schema_version: u32,
    pub lut_artifact_path: PathBuf,
    pub lut_identity_content_id: String,
    pub lut_payload_sha256: String,
    pub authority: ProfileBackedOptimizerAuthority,
}

impl CapturedProfileBackedOptimizerExecution {
    pub fn from_verified(
        lut_artifact_path: PathBuf,
        artifact: &ProfileBackedInverseLutArtifact,
        authority: ProfileBackedOptimizerAuthority,
        recipe: &ConversionRecipe,
    ) -> Result<Self, Vec<String>> {
        artifact.validate()?;
        let capture = Self {
            schema_version: PROFILE_BACKED_OPTIMIZER_EXECUTION_CAPTURE_SCHEMA_VERSION,
            lut_artifact_path,
            lut_identity_content_id: artifact.identity_content_id.clone(),
            lut_payload_sha256: artifact.payload_sha256.clone(),
            authority,
        };
        capture.validate_for_recipe(recipe)?;
        Ok(capture)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != PROFILE_BACKED_OPTIMIZER_EXECUTION_CAPTURE_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported profile-backed execution capture schema {} (expected {}).",
                self.schema_version, PROFILE_BACKED_OPTIMIZER_EXECUTION_CAPTURE_SCHEMA_VERSION
            ));
        }
        if self.lut_artifact_path.as_os_str().is_empty() {
            errors.push("Captured profile-backed inverse-LUT path cannot be empty.".to_owned());
        }
        if !is_prefixed_sha256(&self.lut_identity_content_id) {
            errors.push(
                "Captured profile-backed LUT identity must be canonical sha256:<hex>.".to_owned(),
            );
        }
        if !is_bare_sha256(&self.lut_payload_sha256) {
            errors.push(
                "Captured profile-backed LUT payload identity must be canonical lowercase SHA-256."
                    .to_owned(),
            );
        }
        validate_authority_shape(&self.authority, &mut errors);
        if self.authority.inverse_lut_payload_sha256 != self.lut_payload_sha256 {
            errors.push(
                "Captured profile-backed LUT payload does not match immutable optimizer authority."
                    .to_owned(),
            );
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn validate_for_recipe(&self, recipe: &ConversionRecipe) -> Result<(), Vec<String>> {
        self.validate()?;
        let mut errors = Vec::new();
        if let Err(mut recipe_errors) = recipe.validate() {
            errors.append(&mut recipe_errors);
        }
        if recipe.engine_mode != crate::color_conversion::ConversionEngineMode::CustomOptimizer {
            errors.push(
                "Profile-backed execution capture requires a Custom Optimizer recipe.".to_owned(),
            );
        }
        if recipe
            .target
            .characterization_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            errors.push(
                "Measured-characterization recipes cannot carry profile-backed execution authority."
                    .to_owned(),
            );
        }
        let actual_recipe_sha = match recipe_sha256(recipe) {
            Ok(value) => Some(value),
            Err(error) => {
                errors.push(error);
                None
            }
        };
        if actual_recipe_sha.as_deref() != Some(self.authority.recipe_sha256.as_str()) {
            errors.push(
                "Profile-backed execution capture recipe fingerprint does not match the exact recipe."
                    .to_owned(),
            );
        }
        let recipe_channels = recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        if recipe_channels != self.authority.channel_names {
            errors.push(
                "Profile-backed execution capture channel order does not match the exact recipe."
                    .to_owned(),
            );
        }
        if recipe.target.bit_depth != self.authority.target_bit_depth {
            errors.push(
                "Profile-backed execution capture bit depth does not match the exact recipe."
                    .to_owned(),
            );
        }
        match (
            recipe.target.output_profile_identity.as_ref(),
            recipe.target.output_profile_path.as_deref(),
        ) {
            (Some(identity), Some(path))
                if identity.sha256.trim() == self.authority.output_profile_sha256
                    && path == self.authority.output_profile_path => {}
            _ => errors.push(
                "Profile-backed execution capture Output ICC path/SHA does not match the exact recipe."
                    .to_owned(),
            ),
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn production_provenance(
        &self,
        conversion_recipe_sha256: &str,
    ) -> Result<ProfileBackedOptimizerProductionProvenance, Vec<String>> {
        self.validate()?;
        if !is_bare_sha256(conversion_recipe_sha256) {
            return Err(vec![
                "Profile-backed production provenance requires a canonical conversion recipe SHA-256."
                    .to_owned(),
            ]);
        }
        if self.authority.recipe_sha256 != conversion_recipe_sha256 {
            return Err(vec![
                "Profile-backed production provenance recipe SHA-256 does not match immutable authority."
                    .to_owned(),
            ]);
        }
        let provenance = ProfileBackedOptimizerProductionProvenance {
            schema_version: PROFILE_BACKED_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION,
            output_profile_path: self.authority.output_profile_path.clone(),
            output_profile_sha256: self.authority.output_profile_sha256.clone(),
            forward_model_method: self.authority.forward_model_method,
            forward_model_id: self.authority.forward_model_id.clone(),
            lut_identity_content_id: self.lut_identity_content_id.clone(),
            lut_build_identity_sha256: self.authority.inverse_lut_build_identity_sha256.clone(),
            lut_payload_sha256: self.lut_payload_sha256.clone(),
            channel_names: self.authority.channel_names.clone(),
            target_bit_depth: self.authority.target_bit_depth,
            conversion_recipe_sha256: conversion_recipe_sha256.to_owned(),
        };
        provenance.validate()?;
        Ok(provenance)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileBackedOptimizerProductionProvenance {
    pub schema_version: u32,
    pub output_profile_path: String,
    pub output_profile_sha256: String,
    pub forward_model_method: ProfileBackedExecutionForwardModelMethod,
    pub forward_model_id: String,
    pub lut_identity_content_id: String,
    pub lut_build_identity_sha256: String,
    pub lut_payload_sha256: String,
    pub channel_names: Vec<String>,
    pub target_bit_depth: u8,
    pub conversion_recipe_sha256: String,
}

impl ProfileBackedOptimizerProductionProvenance {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != PROFILE_BACKED_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported profile-backed production provenance schema {} (expected {}).",
                self.schema_version, PROFILE_BACKED_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION
            ));
        }
        if self.output_profile_path.trim().is_empty() {
            errors.push("Profile-backed production provenance requires Output ICC path.".to_owned());
        }
        for (label, value) in [
            ("Output ICC", self.output_profile_sha256.as_str()),
            ("LUT build identity", self.lut_build_identity_sha256.as_str()),
            ("LUT payload", self.lut_payload_sha256.as_str()),
            ("recipe", self.conversion_recipe_sha256.as_str()),
        ] {
            if !is_bare_sha256(value) {
                errors.push(format!(
                    "Profile-backed production provenance {label} must be canonical lowercase SHA-256."
                ));
            }
        }
        if !is_prefixed_sha256(&self.lut_identity_content_id) {
            errors.push(
                "Profile-backed production provenance LUT identity must be canonical sha256:<hex>."
                    .to_owned(),
            );
        }
        if self.forward_model_method
            != ProfileBackedExecutionForwardModelMethod::OutputIccDeviceToPcsV1
        {
            errors.push("Unsupported profile-backed production forward-model method.".to_owned());
        }
        let expected_model = format!("output-icc-sha256:{}", self.output_profile_sha256);
        if self.forward_model_id != expected_model {
            errors.push(
                "Profile-backed production forward-model ID does not match Output ICC SHA-256."
                    .to_owned(),
            );
        }
        validate_channel_names(&self.channel_names, &mut errors);
        if !matches!(self.target_bit_depth, 8 | 16) {
            errors.push(
                "Profile-backed production provenance target bit depth must be 8 or 16."
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

fn validate_authority_shape(
    authority: &ProfileBackedOptimizerAuthority,
    errors: &mut Vec<String>,
) {
    if authority.schema_version != PROFILE_BACKED_OPTIMIZER_AUTHORITY_SCHEMA_VERSION {
        errors.push(format!(
            "Unsupported profile-backed optimizer authority schema {}.",
            authority.schema_version
        ));
    }
    if authority.output_profile_path.trim().is_empty() {
        errors.push("Profile-backed optimizer authority requires Output ICC path.".to_owned());
    }
    for (label, value) in [
        ("Output ICC", authority.output_profile_sha256.as_str()),
        ("recipe", authority.recipe_sha256.as_str()),
        (
            "LUT build identity",
            authority.inverse_lut_build_identity_sha256.as_str(),
        ),
        ("LUT payload", authority.inverse_lut_payload_sha256.as_str()),
    ] {
        if !is_bare_sha256(value) {
            errors.push(format!(
                "Profile-backed optimizer authority {label} must be canonical lowercase SHA-256."
            ));
        }
    }
    if authority.forward_model_method
        != ProfileBackedExecutionForwardModelMethod::OutputIccDeviceToPcsV1
    {
        errors.push("Unsupported profile-backed optimizer authority forward-model method.".to_owned());
    }
    if authority.forward_model_id
        != format!("output-icc-sha256:{}", authority.output_profile_sha256)
    {
        errors.push(
            "Profile-backed optimizer authority forward-model ID does not match Output ICC SHA-256."
                .to_owned(),
        );
    }
    validate_channel_names(&authority.channel_names, errors);
    if !matches!(authority.target_bit_depth, 8 | 16) {
        errors.push("Profile-backed optimizer authority target bit depth must be 8 or 16.".to_owned());
    }
}

fn validate_channel_names(channel_names: &[String], errors: &mut Vec<String>) {
    if !(4..=12).contains(&channel_names.len()) {
        errors.push(format!(
            "Profile-backed production topology must contain 4..=12 channels, got {}.",
            channel_names.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in channel_names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            errors.push("Profile-backed production channel names cannot be empty.".to_owned());
        } else if !seen.insert(trimmed.to_ascii_lowercase()) {
            errors.push(format!("Duplicate profile-backed production channel '{trimmed}'."));
        }
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
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::inverse_lut_identity::{
        INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, InverseLutBuildPolicy,
        InverseLutContinuityFieldMethod, InverseLutInterpolationMethod,
        InverseLutNumericalPrecision, InverseLutOutputQuantization, InverseLutValidityEncoding,
        LabGridSpec,
    };
    use crate::model::IccProfileIdentity;
    use crate::profile_backed_inverse_lut_builder::{
        BuiltProfileBackedInverseLutPayload, ProfileBackedInverseLutBuildStats,
        ProfileBackedForwardModelMethod,
    };
    use sha2::{Digest, Sha256};

    fn output_bytes() -> Vec<u8> {
        b"profile-backed-capture-output".to_vec()
    }

    fn recipe() -> ConversionRecipe {
        let output = output_bytes();
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "1".repeat(64),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Profile target".to_owned(),
                channels: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(1.0),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Output".to_owned(),
                    sha256: format!("{:x}", Sha256::digest(&output)),
                }),
                output_profile_path: Some("C:\\Color\\Ceramic.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: Some(4.0),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    fn built(recipe: &ConversionRecipe) -> BuiltProfileBackedInverseLutPayload {
        let output_hash = recipe
            .target
            .output_profile_identity
            .as_ref()
            .unwrap()
            .sha256
            .clone();
        BuiltProfileBackedInverseLutPayload {
            forward_model_method: ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            forward_model_id: format!("output-icc-sha256:{output_hash}"),
            channel_names: recipe
                .target
                .channels
                .iter()
                .map(|channel| channel.name.clone())
                .collect(),
            target_bit_depth: 16,
            build_policy: InverseLutBuildPolicy {
                schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
                grid: LabGridSpec {
                    l_min: 0.0,
                    l_max: 100.0,
                    l_samples: 2,
                    a_min: -128.0,
                    a_max: 128.0,
                    a_samples: 2,
                    b_min: -128.0,
                    b_max: 128.0,
                    b_samples: 2,
                },
                interpolation: InverseLutInterpolationMethod::TrilinearV1,
                validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
                numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
                output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
                continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
            },
            validity: vec![true; 8],
            coverages: vec![0.25; 8 * 4],
            stats: ProfileBackedInverseLutBuildStats {
                node_count: 8,
                supported_nodes: 8,
                ..ProfileBackedInverseLutBuildStats::default()
            },
        }
    }

    #[test]
    fn capture_binds_exact_artifact_authority_and_recipe() {
        let recipe = recipe();
        let built = built(&recipe);
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe, &built).unwrap();
        let authority = ProfileBackedOptimizerAuthority::capture(&recipe, &output_bytes(), &built)
            .unwrap();
        let capture = CapturedProfileBackedOptimizerExecution::from_verified(
            PathBuf::from("C:\\Jobs\\profile-lut.json"),
            &artifact,
            authority,
            &recipe,
        )
        .unwrap();
        capture.validate_for_recipe(&recipe).unwrap();
        assert_eq!(capture.lut_identity_content_id, artifact.identity_content_id);
        assert_eq!(capture.lut_payload_sha256, artifact.payload_sha256);
        let provenance = capture
            .production_provenance(&recipe_sha256(&recipe).unwrap())
            .unwrap();
        assert_eq!(provenance.output_profile_sha256, artifact.identity.output_profile_sha256);
        assert_eq!(provenance.lut_identity_content_id, artifact.identity_content_id);
    }

    #[test]
    fn measured_recipe_cannot_bind_profile_execution_capture() {
        let mut recipe = recipe();
        let built = built(&recipe);
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe, &built).unwrap();
        let authority = ProfileBackedOptimizerAuthority::capture(&recipe, &output_bytes(), &built)
            .unwrap();
        recipe.target.characterization_id = Some(format!("sha256:{}", "c".repeat(64)));
        let error = CapturedProfileBackedOptimizerExecution::from_verified(
            PathBuf::from("C:\\Jobs\\profile-lut.json"),
            &artifact,
            authority,
            &recipe,
        )
        .unwrap_err();
        assert!(error.iter().any(|value| value.contains("Measured-characterization")));
    }

    #[test]
    fn capture_rejects_lut_payload_identity_not_bound_by_authority() {
        let recipe = recipe();
        let built = built(&recipe);
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe, &built).unwrap();
        let authority = ProfileBackedOptimizerAuthority::capture(&recipe, &output_bytes(), &built)
            .unwrap();
        let mut capture = CapturedProfileBackedOptimizerExecution::from_verified(
            PathBuf::from("C:\\Jobs\\profile-lut.json"),
            &artifact,
            authority,
            &recipe,
        )
        .unwrap();
        capture.lut_payload_sha256 = "f".repeat(64);
        assert!(capture.validate().is_err());
    }

    #[test]
    fn runtime_capture_has_no_measured_calibration_authority() {
        let source = include_str!("profile_backed_optimizer_execution_capture.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("ProfileBackedOptimizerAuthority"));
        assert!(runtime.contains("lut_artifact_path"));
        assert!(!runtime.contains("InverseLutProductionEligibility"));
        assert!(!runtime.contains("CalibrationManifest"));
        assert!(!runtime.contains("CalibrationApproval"));
    }
}
