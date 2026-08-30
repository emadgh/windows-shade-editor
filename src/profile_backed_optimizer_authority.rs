use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;
use crate::profile_backed_inverse_lut_builder::{
    BuiltProfileBackedInverseLutPayload, ProfileBackedForwardModelMethod,
};

pub const PROFILE_BACKED_OPTIMIZER_AUTHORITY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileBackedExecutionForwardModelMethod {
    OutputIccDeviceToPcsV1,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileBackedOptimizerAuthority {
    pub schema_version: u32,
    /// Exact external Output ICC path captured by the recipe.
    pub output_profile_path: String,
    /// Canonical lowercase SHA-256 of the exact Output ICC bytes.
    pub output_profile_sha256: String,
    /// Versioned device→PCS interpretation used during inverse-LUT construction.
    pub forward_model_method: ProfileBackedExecutionForwardModelMethod,
    /// Exact forward-model identity produced by #484.
    pub forward_model_id: String,
    /// Exact conversion recipe fingerprint, including strategy + solver policy.
    pub recipe_sha256: String,
    pub channel_names: Vec<String>,
    pub target_bit_depth: u8,
    /// Content address of the profile-backed LUT construction identity.
    pub inverse_lut_build_identity_sha256: String,
    /// Content hash of node-validity bytes + normalized f32 coverages.
    pub inverse_lut_payload_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileBackedOptimizerAuthorityError {
    InvalidRecipe(Vec<String>),
    NotCustomOptimizer,
    MeasuredCharacterizationTakesPrecedence,
    MissingOutputProfileIdentity,
    InvalidOutputProfileIdentity,
    MissingOutputProfilePath,
    OutputProfileBytesMismatch {
        expected: String,
        actual: String,
    },
    WrongForwardModelMethod,
    ForwardModelIdentityMismatch {
        expected: String,
        actual: String,
    },
    ChannelTopologyMismatch,
    TargetBitDepthMismatch,
    RecipeFingerprint(String),
    BuildIdentity(String),
    PayloadIdentity(String),
    AuthorityMismatch(String),
}

#[derive(Serialize)]
struct ProfileBackedLutBuildIdentity<'a> {
    forward_model_method: &'a str,
    forward_model_id: &'a str,
    recipe_sha256: &'a str,
    channel_names: &'a [String],
    target_bit_depth: u8,
    build_policy: &'a crate::inverse_lut_identity::InverseLutBuildPolicy,
}

impl ProfileBackedOptimizerAuthority {
    /// Capture an immutable authority for the profile-backed execution path.
    ///
    /// This is intentionally disjoint from measured `InverseLutProductionEligibility`.
    /// If a measured characterization is present, capture fails so profile-backed
    /// evidence can never substitute for or downgrade the measured-qualified route.
    pub fn capture(
        recipe: &ConversionRecipe,
        output_profile_bytes: &[u8],
        lut: &BuiltProfileBackedInverseLutPayload,
    ) -> Result<Self, ProfileBackedOptimizerAuthorityError> {
        validate_recipe_for_profile_authority(recipe)?;
        let (path, expected_hash) = exact_output_profile_authority(recipe)?;
        verify_output_profile_bytes(output_profile_bytes, &expected_hash)?;
        validate_built_lut(recipe, &expected_hash, lut)?;

        let recipe_sha256 = recipe_sha256(recipe)
            .map_err(ProfileBackedOptimizerAuthorityError::RecipeFingerprint)?;
        let build_identity_sha256 = build_identity_sha256(lut, &recipe_sha256)?;
        let payload_sha256 = payload_sha256(lut)?;
        let authority = Self {
            schema_version: PROFILE_BACKED_OPTIMIZER_AUTHORITY_SCHEMA_VERSION,
            output_profile_path: path,
            output_profile_sha256: expected_hash,
            forward_model_method: ProfileBackedExecutionForwardModelMethod::OutputIccDeviceToPcsV1,
            forward_model_id: lut.forward_model_id.clone(),
            recipe_sha256,
            channel_names: lut.channel_names.clone(),
            target_bit_depth: lut.target_bit_depth,
            inverse_lut_build_identity_sha256: build_identity_sha256,
            inverse_lut_payload_sha256: payload_sha256,
        };
        authority.validate_shape()?;
        Ok(authority)
    }

    /// Re-validate the complete authority at an execution boundary after reopening
    /// the Output ICC. Caller-provided authority is never trusted by serialization alone.
    pub fn validate_against(
        &self,
        recipe: &ConversionRecipe,
        output_profile_bytes: &[u8],
        lut: &BuiltProfileBackedInverseLutPayload,
    ) -> Result<(), ProfileBackedOptimizerAuthorityError> {
        self.validate_shape()?;
        let expected = Self::capture(recipe, output_profile_bytes, lut)?;
        if *self == expected {
            Ok(())
        } else {
            Err(ProfileBackedOptimizerAuthorityError::AuthorityMismatch(
                "Profile-backed optimizer authority no longer matches the exact recipe, Output ICC or inverse LUT payload."
                    .to_owned(),
            ))
        }
    }

    fn validate_shape(&self) -> Result<(), ProfileBackedOptimizerAuthorityError> {
        if self.schema_version != PROFILE_BACKED_OPTIMIZER_AUTHORITY_SCHEMA_VERSION {
            return Err(ProfileBackedOptimizerAuthorityError::AuthorityMismatch(format!(
                "Unsupported profile-backed optimizer authority schema {}.",
                self.schema_version
            )));
        }
        if self.output_profile_path.trim().is_empty() {
            return Err(ProfileBackedOptimizerAuthorityError::MissingOutputProfilePath);
        }
        for (label, value) in [
            ("Output ICC", self.output_profile_sha256.as_str()),
            ("recipe", self.recipe_sha256.as_str()),
            (
                "inverse LUT build identity",
                self.inverse_lut_build_identity_sha256.as_str(),
            ),
            ("inverse LUT payload", self.inverse_lut_payload_sha256.as_str()),
        ] {
            if !is_bare_sha256(value) {
                return Err(ProfileBackedOptimizerAuthorityError::AuthorityMismatch(format!(
                    "Profile-backed {label} identity is not canonical lowercase SHA-256."
                )));
            }
        }
        let expected_model_id = format!("output-icc-sha256:{}", self.output_profile_sha256);
        if self.forward_model_id != expected_model_id {
            return Err(ProfileBackedOptimizerAuthorityError::ForwardModelIdentityMismatch {
                expected: expected_model_id,
                actual: self.forward_model_id.clone(),
            });
        }
        if !(4..=12).contains(&self.channel_names.len()) {
            return Err(ProfileBackedOptimizerAuthorityError::ChannelTopologyMismatch);
        }
        if !matches!(self.target_bit_depth, 8 | 16) {
            return Err(ProfileBackedOptimizerAuthorityError::TargetBitDepthMismatch);
        }
        Ok(())
    }
}

fn validate_recipe_for_profile_authority(
    recipe: &ConversionRecipe,
) -> Result<(), ProfileBackedOptimizerAuthorityError> {
    recipe
        .validate()
        .map_err(ProfileBackedOptimizerAuthorityError::InvalidRecipe)?;
    if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return Err(ProfileBackedOptimizerAuthorityError::NotCustomOptimizer);
    }
    if recipe
        .target
        .characterization_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(ProfileBackedOptimizerAuthorityError::MeasuredCharacterizationTakesPrecedence);
    }
    Ok(())
}

fn exact_output_profile_authority(
    recipe: &ConversionRecipe,
) -> Result<(String, String), ProfileBackedOptimizerAuthorityError> {
    let identity = recipe
        .target
        .output_profile_identity
        .as_ref()
        .ok_or(ProfileBackedOptimizerAuthorityError::MissingOutputProfileIdentity)?;
    let hash = identity.sha256.trim();
    if !is_bare_sha256(&hash.to_ascii_lowercase()) || hash != hash.to_ascii_lowercase() {
        return Err(ProfileBackedOptimizerAuthorityError::InvalidOutputProfileIdentity);
    }
    let path = recipe
        .target
        .output_profile_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ProfileBackedOptimizerAuthorityError::MissingOutputProfilePath)?;
    Ok((path.to_owned(), hash.to_owned()))
}

fn verify_output_profile_bytes(
    bytes: &[u8],
    expected: &str,
) -> Result<(), ProfileBackedOptimizerAuthorityError> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual == expected {
        Ok(())
    } else {
        Err(ProfileBackedOptimizerAuthorityError::OutputProfileBytesMismatch {
            expected: expected.to_owned(),
            actual,
        })
    }
}

fn validate_built_lut(
    recipe: &ConversionRecipe,
    output_profile_sha256: &str,
    lut: &BuiltProfileBackedInverseLutPayload,
) -> Result<(), ProfileBackedOptimizerAuthorityError> {
    if lut.forward_model_method != ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1 {
        return Err(ProfileBackedOptimizerAuthorityError::WrongForwardModelMethod);
    }
    let expected_model_id = format!("output-icc-sha256:{output_profile_sha256}");
    if lut.forward_model_id != expected_model_id {
        return Err(ProfileBackedOptimizerAuthorityError::ForwardModelIdentityMismatch {
            expected: expected_model_id,
            actual: lut.forward_model_id.clone(),
        });
    }
    let recipe_channels = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    if recipe_channels != lut.channel_names {
        return Err(ProfileBackedOptimizerAuthorityError::ChannelTopologyMismatch);
    }
    if recipe.target.bit_depth != lut.target_bit_depth {
        return Err(ProfileBackedOptimizerAuthorityError::TargetBitDepthMismatch);
    }
    let expected_nodes = lut
        .build_policy
        .grid
        .node_count()
        .ok_or_else(|| {
            ProfileBackedOptimizerAuthorityError::PayloadIdentity(
                "Profile-backed LUT node count overflowed.".to_owned(),
            )
        })?;
    if lut.validity.len() as u64 != expected_nodes {
        return Err(ProfileBackedOptimizerAuthorityError::PayloadIdentity(
            "Profile-backed LUT validity length does not match its grid identity.".to_owned(),
        ));
    }
    let expected_coverages = usize::try_from(expected_nodes)
        .ok()
        .and_then(|nodes| nodes.checked_mul(lut.channel_names.len()))
        .ok_or_else(|| {
            ProfileBackedOptimizerAuthorityError::PayloadIdentity(
                "Profile-backed LUT coverage count overflowed.".to_owned(),
            )
        })?;
    if lut.coverages.len() != expected_coverages {
        return Err(ProfileBackedOptimizerAuthorityError::PayloadIdentity(
            "Profile-backed LUT coverage length does not match its grid/topology identity."
                .to_owned(),
        ));
    }
    Ok(())
}

fn build_identity_sha256(
    lut: &BuiltProfileBackedInverseLutPayload,
    recipe_sha256: &str,
) -> Result<String, ProfileBackedOptimizerAuthorityError> {
    let method = match lut.forward_model_method {
        ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1 => "output_icc_device_to_pcs_v1",
    };
    let identity = ProfileBackedLutBuildIdentity {
        forward_model_method: method,
        forward_model_id: &lut.forward_model_id,
        recipe_sha256,
        channel_names: &lut.channel_names,
        target_bit_depth: lut.target_bit_depth,
        build_policy: &lut.build_policy,
    };
    let bytes = serde_json::to_vec(&identity)
        .map_err(|error| ProfileBackedOptimizerAuthorityError::BuildIdentity(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn payload_sha256(
    lut: &BuiltProfileBackedInverseLutPayload,
) -> Result<String, ProfileBackedOptimizerAuthorityError> {
    let mut hasher = Sha256::new();
    for valid in &lut.validity {
        hasher.update([u8::from(*valid)]);
    }
    for (index, value) in lut.coverages.iter().copied().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ProfileBackedOptimizerAuthorityError::PayloadIdentity(format!(
                "Profile-backed LUT coverage {index} is not finite normalized data."
            )));
        }
        hasher.update(value.to_bits().to_le_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
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
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::inverse_lut_identity::{
        INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, InverseLutBuildPolicy,
        InverseLutContinuityFieldMethod, InverseLutInterpolationMethod,
        InverseLutNumericalPrecision, InverseLutOutputQuantization, InverseLutValidityEncoding,
        LabGridSpec,
    };
    use crate::model::IccProfileIdentity;

    fn bytes() -> Vec<u8> {
        b"fixture-output-icc-bytes".to_vec()
    }

    fn recipe() -> ConversionRecipe {
        let bytes = bytes();
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "source-hash".to_owned(),
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
                        max_coverage: Some(0.8),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Output".to_owned(),
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                }),
                output_profile_path: Some("C:\\Color\\Ceramic.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: Some(1.8),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    fn lut(recipe: &ConversionRecipe) -> BuiltProfileBackedInverseLutPayload {
        let hash = recipe.target.output_profile_identity.as_ref().unwrap().sha256.clone();
        BuiltProfileBackedInverseLutPayload {
            forward_model_method: ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            forward_model_id: format!("output-icc-sha256:{hash}"),
            channel_names: recipe
                .target
                .channels
                .iter()
                .map(|channel| channel.name.clone())
                .collect(),
            target_bit_depth: recipe.target.bit_depth,
            build_policy: InverseLutBuildPolicy {
                schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
                grid: LabGridSpec {
                    l_min: 0.0,
                    l_max: 100.0,
                    l_samples: 2,
                    a_min: -1.0,
                    a_max: 1.0,
                    a_samples: 2,
                    b_min: -1.0,
                    b_max: 1.0,
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
            stats: Default::default(),
        }
    }

    #[test]
    fn authority_is_deterministic_and_exactly_revalidates() {
        let recipe = recipe();
        let lut = lut(&recipe);
        let first = ProfileBackedOptimizerAuthority::capture(&recipe, &bytes(), &lut).unwrap();
        let second = ProfileBackedOptimizerAuthority::capture(&recipe, &bytes(), &lut).unwrap();
        assert_eq!(first, second);
        first.validate_against(&recipe, &bytes(), &lut).unwrap();
        assert_eq!(
            first.forward_model_method,
            ProfileBackedExecutionForwardModelMethod::OutputIccDeviceToPcsV1
        );
    }

    #[test]
    fn profile_bytes_recipe_and_lut_mutations_fail_closed() {
        let recipe = recipe();
        let lut = lut(&recipe);
        let authority = ProfileBackedOptimizerAuthority::capture(&recipe, &bytes(), &lut).unwrap();

        assert!(authority
            .validate_against(&recipe, b"different-profile", &lut)
            .is_err());

        let mut changed_recipe = recipe.clone();
        changed_recipe.strategy.black_generation_strength = 0.8;
        assert!(authority
            .validate_against(&changed_recipe, &bytes(), &lut)
            .is_err());

        let mut changed_lut = lut.clone();
        changed_lut.coverages[0] = 0.3;
        assert!(authority
            .validate_against(&recipe, &bytes(), &changed_lut)
            .is_err());
    }

    #[test]
    fn measured_recipe_cannot_be_downgraded_to_profile_authority() {
        let mut recipe = recipe();
        recipe.target.characterization_id = Some(format!("sha256:{}", "c".repeat(64)));
        let lut = lut(&recipe);
        assert_eq!(
            ProfileBackedOptimizerAuthority::capture(&recipe, &bytes(), &lut),
            Err(ProfileBackedOptimizerAuthorityError::MeasuredCharacterizationTakesPrecedence)
        );
    }

    #[test]
    fn authority_runtime_contains_no_measured_eligibility_inputs() {
        let source = include_str!("profile_backed_optimizer_authority.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("InverseLutProductionEligibility"));
        assert!(!runtime.contains("CalibrationManifest"));
        assert!(!runtime.contains("CalibrationApproval"));
        assert!(runtime.contains("MeasuredCharacterizationTakesPrecedence"));
        assert!(runtime.contains("output_profile_sha256"));
        assert!(runtime.contains("inverse_lut_payload_sha256"));
    }
}