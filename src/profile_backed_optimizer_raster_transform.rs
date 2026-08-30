use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;
use crate::device_characterization::LabColor;
use crate::icc_conversion::{IccSourceModel, RuntimeIccProfile};
use crate::inverse_lut_identity::{InverseLutBuildPolicy, quantize_normalized_coverage};
use crate::production_lab_transform::{PcsLabPixel, ProductionPcsLabTransform};
use crate::profile_backed_inverse_lut_artifact::{
    ProfileBackedInverseLutArtifact, ProfileBackedInverseLutForwardModelMethod,
    ProfileBackedInverseLutRuntime,
};
use crate::profile_backed_optimizer_authority::{
    PROFILE_BACKED_OPTIMIZER_AUTHORITY_SCHEMA_VERSION, ProfileBackedExecutionForwardModelMethod,
    ProfileBackedOptimizerAuthority,
};

pub const MAX_PROFILE_BACKED_OPTIMIZER_RASTER_CHUNK_PIXELS: usize =
    crate::custom_optimizer_raster_transform::MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS;

#[derive(Clone, Debug, PartialEq)]
pub enum ProfileBackedOptimizerRasterError {
    InvalidRecipe(Vec<String>),
    NotCustomOptimizer,
    MeasuredCharacterizationTakesPrecedence,
    InvalidSourceProfileIdentity(String),
    SourceProfileMismatch { expected: String, actual: String },
    OutputProfileBytesMismatch { expected: String, actual: String },
    AuthorityMismatch(String),
    Artifact(String),
    Construction(String),
    WrongTargetBitDepth { expected: u8, requested: u8 },
    SourceTopology { source_channels: usize, sample_count: usize },
    DestinationTopology { expected_samples: usize, actual_samples: usize },
    ChunkTooLarge { pixels: usize, maximum: usize },
    SizeOverflow,
    LabTransform(String),
    Lookup { pixel_index: usize, error: String },
    Quantization {
        pixel_index: usize,
        channel_index: usize,
        error: String,
    },
}

#[derive(Serialize)]
struct AuthorityBuildIdentity<'a> {
    forward_model_method: &'a str,
    forward_model_id: &'a str,
    recipe_sha256: &'a str,
    channel_names: &'a [String],
    target_bit_depth: u8,
    build_policy: &'a InverseLutBuildPolicy,
}

/// Production Source ICC -> PCS Lab -> profile-backed inverse-LUT raster transform.
///
/// This constructor is deliberately parallel to `ProductionCustomOptimizerRasterTransform`.
/// It never accepts measured calibration manifests or measured production eligibility, and
/// the measured constructor is not relaxed. The caller must reopen the exact Output ICC and
/// pass its bytes here; authorization binds those bytes, recipe, channel topology and exact
/// profile-backed LUT payload to the immutable authority captured before job execution.
pub struct ProfileBackedCustomOptimizerRasterTransform {
    authority: ProfileBackedOptimizerAuthority,
    kernel: ProfileBackedRasterKernel,
}

impl ProfileBackedCustomOptimizerRasterTransform {
    pub fn authorize(
        source_model: IccSourceModel,
        source_icc: &[u8],
        output_icc: &[u8],
        authority: &ProfileBackedOptimizerAuthority,
        artifact: ProfileBackedInverseLutArtifact,
        recipe: &ConversionRecipe,
    ) -> Result<Self, ProfileBackedOptimizerRasterError> {
        verify_source_icc(source_icc, &recipe.source_profile_identity.sha256)?;
        validate_profile_authority(authority, recipe, output_icc, &artifact)?;
        let kernel = ProfileBackedRasterKernel::new(source_model, source_icc, artifact, recipe)?;
        Ok(Self {
            authority: authority.clone(),
            kernel,
        })
    }

    pub fn authority(&self) -> &ProfileBackedOptimizerAuthority {
        &self.authority
    }

    pub fn output_channels(&self) -> usize {
        self.kernel.channel_count
    }

    pub fn target_bit_depth(&self) -> u8 {
        self.kernel.target_bit_depth
    }

    pub fn transform_u8_chunk(
        &mut self,
        source: &[u16],
        destination: &mut [u8],
    ) -> Result<(), ProfileBackedOptimizerRasterError> {
        self.kernel.transform_u8_chunk(source, destination)
    }

    pub fn transform_u16_chunk(
        &mut self,
        source: &[u16],
        destination: &mut [u16],
    ) -> Result<(), ProfileBackedOptimizerRasterError> {
        self.kernel.transform_u16_chunk(source, destination)
    }
}

struct ProfileBackedRasterKernel {
    source_model: IccSourceModel,
    lab_transform: ProductionPcsLabTransform,
    runtime: ProfileBackedInverseLutRuntime,
    channel_count: usize,
    target_bit_depth: u8,
    lab_scratch: Vec<PcsLabPixel>,
}

impl ProfileBackedRasterKernel {
    fn new(
        source_model: IccSourceModel,
        source_icc: &[u8],
        artifact: ProfileBackedInverseLutArtifact,
        recipe: &ConversionRecipe,
    ) -> Result<Self, ProfileBackedOptimizerRasterError> {
        let runtime = ProfileBackedInverseLutRuntime::new(artifact)
            .map_err(ProfileBackedOptimizerRasterError::Artifact)?;
        let recipe_channels = recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>();
        let runtime_channels = runtime
            .identity()
            .channel_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if recipe_channels != runtime_channels {
            return Err(ProfileBackedOptimizerRasterError::Construction(
                "Profile-backed raster LUT channel order does not match the exact recipe."
                    .to_owned(),
            ));
        }
        if runtime.identity().recipe_sha256
            != recipe_sha256(recipe).map_err(ProfileBackedOptimizerRasterError::Construction)?
        {
            return Err(ProfileBackedOptimizerRasterError::Construction(
                "Profile-backed raster LUT recipe fingerprint does not match the exact recipe."
                    .to_owned(),
            ));
        }
        if runtime.target_bit_depth() != recipe.target.bit_depth {
            return Err(ProfileBackedOptimizerRasterError::Construction(format!(
                "Profile-backed raster LUT bit depth {} does not match recipe {}.",
                runtime.target_bit_depth(),
                recipe.target.bit_depth
            )));
        }
        let channel_count = runtime.output_channels();
        if !(4..=crate::custom_optimizer_config::CUSTOM_OPTIMIZER_MAX_CHANNELS)
            .contains(&channel_count)
        {
            return Err(ProfileBackedOptimizerRasterError::Construction(format!(
                "Profile-backed raster channel count {channel_count} is outside the Custom Optimizer bound."
            )));
        }
        let target_bit_depth = runtime.target_bit_depth();
        let lab_transform = ProductionPcsLabTransform::new(
            source_model,
            RuntimeIccProfile::Embedded(source_icc),
            recipe.rendering_intent,
            recipe.black_point_compensation,
        )
        .map_err(ProfileBackedOptimizerRasterError::Construction)?;
        Ok(Self {
            source_model,
            lab_transform,
            runtime,
            channel_count,
            target_bit_depth,
            lab_scratch: Vec::new(),
        })
    }

    fn transform_u8_chunk(
        &mut self,
        source: &[u16],
        destination: &mut [u8],
    ) -> Result<(), ProfileBackedOptimizerRasterError> {
        self.require_bit_depth(8)?;
        let pixels = self.prepare_lab_chunk(source)?;
        self.validate_destination_len(pixels, destination.len())?;
        if pixels == 0 {
            return Ok(());
        }
        let quantization = self.runtime.identity().build_policy.output_quantization;
        let mut normalized =
            [0.0f32; crate::custom_optimizer_config::CUSTOM_OPTIMIZER_MAX_CHANNELS];
        for (pixel_index, lab) in self.lab_scratch[..pixels].iter().copied().enumerate() {
            self.lookup(pixel_index, lab, &mut normalized[..self.channel_count])?;
            let start = pixel_index * self.channel_count;
            for channel_index in 0..self.channel_count {
                let value = quantize_normalized_coverage(
                    normalized[channel_index],
                    8,
                    quantization,
                )
                .map_err(|error| ProfileBackedOptimizerRasterError::Quantization {
                    pixel_index,
                    channel_index,
                    error,
                })?;
                destination[start + channel_index] = value as u8;
            }
        }
        Ok(())
    }

    fn transform_u16_chunk(
        &mut self,
        source: &[u16],
        destination: &mut [u16],
    ) -> Result<(), ProfileBackedOptimizerRasterError> {
        self.require_bit_depth(16)?;
        let pixels = self.prepare_lab_chunk(source)?;
        self.validate_destination_len(pixels, destination.len())?;
        if pixels == 0 {
            return Ok(());
        }
        let quantization = self.runtime.identity().build_policy.output_quantization;
        let mut normalized =
            [0.0f32; crate::custom_optimizer_config::CUSTOM_OPTIMIZER_MAX_CHANNELS];
        for (pixel_index, lab) in self.lab_scratch[..pixels].iter().copied().enumerate() {
            self.lookup(pixel_index, lab, &mut normalized[..self.channel_count])?;
            let start = pixel_index * self.channel_count;
            for channel_index in 0..self.channel_count {
                destination[start + channel_index] = quantize_normalized_coverage(
                    normalized[channel_index],
                    16,
                    quantization,
                )
                .map_err(|error| ProfileBackedOptimizerRasterError::Quantization {
                    pixel_index,
                    channel_index,
                    error,
                })?;
            }
        }
        Ok(())
    }

    fn prepare_lab_chunk(
        &mut self,
        source: &[u16],
    ) -> Result<usize, ProfileBackedOptimizerRasterError> {
        let source_channels = match self.source_model {
            IccSourceModel::Rgb => 3,
            IccSourceModel::Cmyk => 4,
        };
        if source.len() % source_channels != 0 {
            return Err(ProfileBackedOptimizerRasterError::SourceTopology {
                source_channels,
                sample_count: source.len(),
            });
        }
        let pixels = source.len() / source_channels;
        if pixels > MAX_PROFILE_BACKED_OPTIMIZER_RASTER_CHUNK_PIXELS {
            return Err(ProfileBackedOptimizerRasterError::ChunkTooLarge {
                pixels,
                maximum: MAX_PROFILE_BACKED_OPTIMIZER_RASTER_CHUNK_PIXELS,
            });
        }
        if pixels == 0 {
            self.lab_scratch.clear();
            return Ok(0);
        }
        self.lab_scratch.resize(pixels, [0.0; 3]);
        match self.source_model {
            IccSourceModel::Rgb => self
                .lab_transform
                .transform_rgb_chunk(
                    samples_as_arrays::<3>(source)?,
                    &mut self.lab_scratch[..pixels],
                )
                .map_err(ProfileBackedOptimizerRasterError::LabTransform)?,
            IccSourceModel::Cmyk => self
                .lab_transform
                .transform_cmyk_chunk(
                    samples_as_arrays::<4>(source)?,
                    &mut self.lab_scratch[..pixels],
                )
                .map_err(ProfileBackedOptimizerRasterError::LabTransform)?,
        }
        Ok(pixels)
    }

    fn lookup(
        &self,
        pixel_index: usize,
        lab: PcsLabPixel,
        normalized: &mut [f32],
    ) -> Result<(), ProfileBackedOptimizerRasterError> {
        self.runtime
            .lookup_into(
                LabColor {
                    l: lab[0],
                    a: lab[1],
                    b: lab[2],
                },
                normalized,
            )
            .map_err(|error| ProfileBackedOptimizerRasterError::Lookup { pixel_index, error })
    }

    fn require_bit_depth(
        &self,
        requested: u8,
    ) -> Result<(), ProfileBackedOptimizerRasterError> {
        if self.target_bit_depth == requested {
            Ok(())
        } else {
            Err(ProfileBackedOptimizerRasterError::WrongTargetBitDepth {
                expected: self.target_bit_depth,
                requested,
            })
        }
    }

    fn validate_destination_len(
        &self,
        pixels: usize,
        actual: usize,
    ) -> Result<(), ProfileBackedOptimizerRasterError> {
        let expected = pixels
            .checked_mul(self.channel_count)
            .ok_or(ProfileBackedOptimizerRasterError::SizeOverflow)?;
        if expected == actual {
            Ok(())
        } else {
            Err(ProfileBackedOptimizerRasterError::DestinationTopology {
                expected_samples: expected,
                actual_samples: actual,
            })
        }
    }
}

fn validate_profile_authority(
    authority: &ProfileBackedOptimizerAuthority,
    recipe: &ConversionRecipe,
    output_icc: &[u8],
    artifact: &ProfileBackedInverseLutArtifact,
) -> Result<(), ProfileBackedOptimizerRasterError> {
    recipe
        .validate()
        .map_err(ProfileBackedOptimizerRasterError::InvalidRecipe)?;
    if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return Err(ProfileBackedOptimizerRasterError::NotCustomOptimizer);
    }
    if recipe
        .target
        .characterization_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(ProfileBackedOptimizerRasterError::MeasuredCharacterizationTakesPrecedence);
    }
    artifact
        .validate()
        .map_err(|errors| ProfileBackedOptimizerRasterError::Artifact(errors.join("\n")))?;

    if authority.schema_version != PROFILE_BACKED_OPTIMIZER_AUTHORITY_SCHEMA_VERSION {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(format!(
            "Unsupported profile-backed optimizer authority schema {}.",
            authority.schema_version
        )));
    }
    for (label, value) in [
        ("Output ICC", authority.output_profile_sha256.as_str()),
        ("recipe", authority.recipe_sha256.as_str()),
        (
            "inverse LUT build identity",
            authority.inverse_lut_build_identity_sha256.as_str(),
        ),
        (
            "inverse LUT payload",
            authority.inverse_lut_payload_sha256.as_str(),
        ),
    ] {
        if !is_bare_sha256(value) {
            return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(format!(
                "Profile-backed {label} authority is not canonical lowercase SHA-256."
            )));
        }
    }

    let target_identity = recipe.target.output_profile_identity.as_ref().ok_or_else(|| {
        ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed execution recipe has no Output ICC identity.".to_owned(),
        )
    })?;
    let expected_output_sha = target_identity.sha256.trim();
    if !is_bare_sha256(expected_output_sha) {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed execution recipe Output ICC identity is not canonical lowercase SHA-256."
                .to_owned(),
        ));
    }
    let actual_output_sha = format!("{:x}", Sha256::digest(output_icc));
    if actual_output_sha != expected_output_sha {
        return Err(ProfileBackedOptimizerRasterError::OutputProfileBytesMismatch {
            expected: expected_output_sha.to_owned(),
            actual: actual_output_sha,
        });
    }
    let expected_path = recipe
        .target
        .output_profile_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ProfileBackedOptimizerRasterError::AuthorityMismatch(
                "Profile-backed execution recipe has no exact Output ICC path.".to_owned(),
            )
        })?;
    if authority.output_profile_path != expected_path {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed authority Output ICC path does not match the exact recipe."
                .to_owned(),
        ));
    }
    if authority.output_profile_sha256 != expected_output_sha {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed authority Output ICC SHA-256 does not match reopened profile bytes."
                .to_owned(),
        ));
    }
    if authority.forward_model_method
        != ProfileBackedExecutionForwardModelMethod::OutputIccDeviceToPcsV1
    {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed authority forward-model method is unsupported.".to_owned(),
        ));
    }
    let expected_model_id = format!("output-icc-sha256:{expected_output_sha}");
    if authority.forward_model_id != expected_model_id {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed authority forward-model ID does not match Output ICC identity."
                .to_owned(),
        ));
    }

    let expected_recipe_sha =
        recipe_sha256(recipe).map_err(ProfileBackedOptimizerRasterError::Construction)?;
    if authority.recipe_sha256 != expected_recipe_sha {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed authority recipe fingerprint does not match the exact recipe."
                .to_owned(),
        ));
    }
    let recipe_channels = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    if authority.channel_names != recipe_channels {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed authority channel order does not match the exact recipe.".to_owned(),
        ));
    }
    if authority.target_bit_depth != recipe.target.bit_depth {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed authority bit depth does not match the exact recipe.".to_owned(),
        ));
    }

    let identity = &artifact.identity;
    if identity.forward_model_method
        != ProfileBackedInverseLutForwardModelMethod::OutputIccDeviceToPcsV1
        || identity.output_profile_sha256 != expected_output_sha
        || identity.forward_model_id != expected_model_id
        || identity.recipe_sha256 != expected_recipe_sha
        || identity.channel_names != recipe_channels
        || identity.target_bit_depth != recipe.target.bit_depth
    {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed LUT identity no longer matches the exact Output ICC/recipe/topology."
                .to_owned(),
        ));
    }
    if authority.inverse_lut_payload_sha256 != artifact.payload_sha256 {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed LUT payload hash no longer matches immutable authority.".to_owned(),
        ));
    }
    let build_identity = AuthorityBuildIdentity {
        forward_model_method: "output_icc_device_to_pcs_v1",
        forward_model_id: &identity.forward_model_id,
        recipe_sha256: &identity.recipe_sha256,
        channel_names: &identity.channel_names,
        target_bit_depth: identity.target_bit_depth,
        build_policy: &identity.build_policy,
    };
    let build_bytes = serde_json::to_vec(&build_identity)
        .map_err(|error| ProfileBackedOptimizerRasterError::Construction(error.to_string()))?;
    let actual_build_sha = format!("{:x}", Sha256::digest(build_bytes));
    if authority.inverse_lut_build_identity_sha256 != actual_build_sha {
        return Err(ProfileBackedOptimizerRasterError::AuthorityMismatch(
            "Profile-backed LUT build identity no longer matches immutable authority.".to_owned(),
        ));
    }
    Ok(())
}

fn verify_source_icc(
    source_icc: &[u8],
    expected_sha256: &str,
) -> Result<(), ProfileBackedOptimizerRasterError> {
    let expected = expected_sha256.trim();
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProfileBackedOptimizerRasterError::InvalidSourceProfileIdentity(
            "Captured Source ICC identity is not a full SHA-256.".to_owned(),
        ));
    }
    let actual = format!("{:x}", Sha256::digest(source_icc));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(ProfileBackedOptimizerRasterError::SourceProfileMismatch {
            expected: expected.to_owned(),
            actual,
        })
    }
}

fn samples_as_arrays<const N: usize>(
    samples: &[u16],
) -> Result<&[[u16; N]], ProfileBackedOptimizerRasterError> {
    if samples.len() % N != 0 {
        return Err(ProfileBackedOptimizerRasterError::SourceTopology {
            source_channels: N,
            sample_count: samples.len(),
        });
    }
    // SAFETY: `[u16; N]` has u16 alignment and length divisibility is checked above.
    Ok(unsafe {
        std::slice::from_raw_parts(samples.as_ptr().cast::<[u16; N]>(), samples.len() / N)
    })
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
    use lcms2::Profile;

    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::inverse_lut_identity::{
        INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, InverseLutContinuityFieldMethod,
        InverseLutInterpolationMethod, InverseLutNumericalPrecision,
        InverseLutOutputQuantization, InverseLutValidityEncoding, LabGridSpec,
    };
    use crate::model::IccProfileIdentity;
    use crate::profile_backed_inverse_lut_builder::{
        BuiltProfileBackedInverseLutPayload, ProfileBackedInverseLutBuildStats,
        ProfileBackedForwardModelMethod,
    };

    fn source_bytes() -> Vec<u8> {
        Profile::new_srgb().icc().unwrap()
    }

    fn output_bytes() -> Vec<u8> {
        b"profile-backed-output-fixture".to_vec()
    }

    fn recipe(bit_depth: u8) -> ConversionRecipe {
        let source = source_bytes();
        let output = output_bytes();
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Fixture sRGB".to_owned(),
                sha256: format!("{:x}", Sha256::digest(&source)),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Profile-backed 4C".to_owned(),
                channels: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(1.0),
                    })
                    .collect(),
                bit_depth,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Fixture Output".to_owned(),
                    sha256: format!("{:x}", Sha256::digest(&output)),
                }),
                output_profile_path: Some("C:\\Color\\Fixture.icc".to_owned()),
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

    fn policy() -> InverseLutBuildPolicy {
        InverseLutBuildPolicy {
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
        }
    }

    fn built(bit_depth: u8) -> BuiltProfileBackedInverseLutPayload {
        let recipe = recipe(bit_depth);
        let hash = recipe
            .target
            .output_profile_identity
            .as_ref()
            .unwrap()
            .sha256
            .clone();
        let pattern = [0.0f32, 0.5, 1.0, 0.25];
        let mut coverages = Vec::new();
        for _ in 0..8 {
            coverages.extend_from_slice(&pattern);
        }
        BuiltProfileBackedInverseLutPayload {
            forward_model_method: ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            forward_model_id: format!("output-icc-sha256:{hash}"),
            channel_names: recipe
                .target
                .channels
                .iter()
                .map(|channel| channel.name.clone())
                .collect(),
            target_bit_depth: bit_depth,
            build_policy: policy(),
            validity: vec![true; 8],
            coverages,
            stats: ProfileBackedInverseLutBuildStats {
                node_count: 8,
                supported_nodes: 8,
                ..ProfileBackedInverseLutBuildStats::default()
            },
        }
    }

    fn transform(bit_depth: u8) -> ProfileBackedCustomOptimizerRasterTransform {
        let recipe = recipe(bit_depth);
        let built = built(bit_depth);
        let authority =
            ProfileBackedOptimizerAuthority::capture(&recipe, &output_bytes(), &built).unwrap();
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe, &built).unwrap();
        ProfileBackedCustomOptimizerRasterTransform::authorize(
            IccSourceModel::Rgb,
            &source_bytes(),
            &output_bytes(),
            &authority,
            artifact,
            &recipe,
        )
        .unwrap()
    }

    #[test]
    fn constant_profile_lut_quantizes_deterministically_to_u16() {
        let mut transform = transform(16);
        let source = [0u16, 0, 0, u16::MAX, u16::MAX, u16::MAX];
        let mut first = [0u16; 8];
        let mut second = [0u16; 8];
        transform.transform_u16_chunk(&source, &mut first).unwrap();
        transform.transform_u16_chunk(&source, &mut second).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[..4], &[0, 32_768, 65_535, 16_384]);
        assert_eq!(&first[4..], &[0, 32_768, 65_535, 16_384]);
    }

    #[test]
    fn output_profile_mutation_fails_before_raster_authorization() {
        let recipe = recipe(16);
        let built = built(16);
        let authority =
            ProfileBackedOptimizerAuthority::capture(&recipe, &output_bytes(), &built).unwrap();
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe, &built).unwrap();
        let error = ProfileBackedCustomOptimizerRasterTransform::authorize(
            IccSourceModel::Rgb,
            &source_bytes(),
            b"mutated-output-profile",
            &authority,
            artifact,
            &recipe,
        )
        .err()
        .expect("mutated Output ICC must fail closed");
        assert!(matches!(
            error,
            ProfileBackedOptimizerRasterError::OutputProfileBytesMismatch { .. }
        ));
    }

    #[test]
    fn measured_recipe_cannot_enter_profile_backed_raster_authority() {
        let mut recipe = recipe(16);
        let built = built(16);
        let authority =
            ProfileBackedOptimizerAuthority::capture(&recipe, &output_bytes(), &built).unwrap();
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe, &built).unwrap();
        recipe.target.characterization_id = Some(format!("sha256:{}", "c".repeat(64)));
        let error = ProfileBackedCustomOptimizerRasterTransform::authorize(
            IccSourceModel::Rgb,
            &source_bytes(),
            &output_bytes(),
            &authority,
            artifact,
            &recipe,
        )
        .err()
        .expect("measured recipe must not be downgraded");
        assert!(matches!(
            error,
            ProfileBackedOptimizerRasterError::MeasuredCharacterizationTakesPrecedence
                | ProfileBackedOptimizerRasterError::InvalidRecipe(_)
        ));
    }

    #[test]
    fn profile_raster_runtime_is_disjoint_from_measured_eligibility() {
        let source = include_str!("profile_backed_optimizer_raster_transform.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("ProfileBackedOptimizerAuthority"));
        assert!(runtime.contains("ProfileBackedInverseLutArtifact"));
        assert!(runtime.contains("MeasuredCharacterizationTakesPrecedence"));
        assert!(!runtime.contains("InverseLutProductionEligibility"));
        assert!(!runtime.contains("CalibrationManifest"));
        assert!(!runtime.contains("CalibrationApproval"));
    }
}
