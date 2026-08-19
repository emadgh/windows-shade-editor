use sha2::{Digest, Sha256};

use crate::color_conversion::ConversionRecipe;
use crate::device_characterization::LabColor;
use crate::device_characterization_model::ValidatedLocalForwardModel;
use crate::icc_conversion::{IccSourceModel, RuntimeIccProfile};
use crate::inverse_lut_artifact::VerifiedInverseLutArtifact;
use crate::inverse_lut_identity::quantize_normalized_coverage;
use crate::inverse_lut_production_eligibility::{
    InverseLutProductionEligibility, InverseLutProductionEligibilityError,
    validate_inverse_lut_production_eligibility,
};
use crate::inverse_lut_runtime::{InverseLutLookupError, InverseLutRuntime};
use crate::inverse_lut_threshold_set::{
    InverseLutThresholdCalibrationApproval, InverseLutThresholdCalibrationManifest,
    InverseLutValidationThresholdSet,
};
use crate::inverse_lut_validation_artifact::VerifiedInverseLutValidationArtifact;
use crate::production_colorimetry::ValidatedProductionPcsCompatibility;
use crate::production_lab_transform::{PcsLabPixel, ProductionPcsLabTransform};

/// Hard upper bound for one in-memory production raster transform chunk.
/// The filesystem worker may choose a smaller row window, but can never ask
/// this hot path to allocate scratch proportional to an entire large artwork.
pub const MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS: usize = 262_144;

#[derive(Clone, Debug, PartialEq)]
pub enum ProductionCustomOptimizerRasterError {
    Authorization(InverseLutProductionEligibilityError),
    InvalidLut(InverseLutLookupError),
    InvalidSourceProfileIdentity(String),
    SourceProfileMismatch {
        expected: String,
        actual: String,
    },
    Construction(String),
    WrongTargetBitDepth {
        expected: u8,
        requested: u8,
    },
    SourceTopology {
        source_channels: usize,
        sample_count: usize,
    },
    DestinationTopology {
        expected_samples: usize,
        actual_samples: usize,
    },
    ChunkTooLarge {
        pixels: usize,
        maximum: usize,
    },
    SizeOverflow,
    LabTransform(String),
    Lookup {
        pixel_index: usize,
        error: InverseLutLookupError,
    },
    Quantization {
        pixel_index: usize,
        channel_index: usize,
        error: String,
    },
}

/// Authorized production Source ICC -> PCS Lab -> inverse-LUT raster transform.
///
/// The public constructor always re-runs the complete production eligibility
/// minting path from the exact immutable evidence values. A serialized
/// `InverseLutProductionEligibility` is deliberately not accepted as input.
pub struct ProductionCustomOptimizerRasterTransform {
    eligibility: InverseLutProductionEligibility,
    kernel: CustomOptimizerRasterKernel,
}

impl ProductionCustomOptimizerRasterTransform {
    #[allow(clippy::too_many_arguments)]
    pub fn authorize(
        source_model: IccSourceModel,
        source_icc: &[u8],
        lut: &VerifiedInverseLutArtifact,
        validation: &VerifiedInverseLutValidationArtifact,
        threshold_set: &InverseLutValidationThresholdSet,
        calibration_manifest: &InverseLutThresholdCalibrationManifest,
        calibration_approval: &InverseLutThresholdCalibrationApproval,
        pcs_compatibility: &ValidatedProductionPcsCompatibility,
        recipe: &ConversionRecipe,
        model: &ValidatedLocalForwardModel,
    ) -> Result<Self, ProductionCustomOptimizerRasterError> {
        verify_source_icc(source_icc, &recipe.source_profile_identity.sha256)?;
        let eligibility = validate_inverse_lut_production_eligibility(
            lut,
            validation,
            threshold_set,
            calibration_manifest,
            calibration_approval,
            pcs_compatibility,
            recipe,
            model,
        )
        .map_err(ProductionCustomOptimizerRasterError::Authorization)?;
        let kernel = CustomOptimizerRasterKernel::new(source_model, source_icc, lut, recipe)?;
        Ok(Self {
            eligibility,
            kernel,
        })
    }

    pub fn eligibility(&self) -> &InverseLutProductionEligibility {
        &self.eligibility
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
    ) -> Result<(), ProductionCustomOptimizerRasterError> {
        self.kernel.transform_u8_chunk(source, destination)
    }

    pub fn transform_u16_chunk(
        &mut self,
        source: &[u16],
        destination: &mut [u16],
    ) -> Result<(), ProductionCustomOptimizerRasterError> {
        self.kernel.transform_u16_chunk(source, destination)
    }
}

struct CustomOptimizerRasterKernel {
    source_model: IccSourceModel,
    lab_transform: ProductionPcsLabTransform,
    runtime: InverseLutRuntime,
    channel_count: usize,
    target_bit_depth: u8,
    lab_scratch: Vec<PcsLabPixel>,
}

impl CustomOptimizerRasterKernel {
    fn new(
        source_model: IccSourceModel,
        source_icc: &[u8],
        lut: &VerifiedInverseLutArtifact,
        recipe: &ConversionRecipe,
    ) -> Result<Self, ProductionCustomOptimizerRasterError> {
        let runtime = InverseLutRuntime::from_verified(lut.clone())
            .map_err(ProductionCustomOptimizerRasterError::InvalidLut)?;
        let recipe_channels = recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>();
        let lut_channels = runtime
            .identity()
            .channel_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if recipe_channels != lut_channels {
            return Err(ProductionCustomOptimizerRasterError::Construction(
                "Authorized raster LUT channel order does not match the captured recipe."
                    .to_owned(),
            ));
        }
        if runtime.identity().target_bit_depth != recipe.target.bit_depth {
            return Err(ProductionCustomOptimizerRasterError::Construction(format!(
                "Authorized raster LUT bit depth {} does not match captured recipe {}.",
                runtime.identity().target_bit_depth,
                recipe.target.bit_depth
            )));
        }
        let channel_count = runtime.identity().channel_names.len();
        if channel_count == 0
            || channel_count > crate::custom_optimizer_config::CUSTOM_OPTIMIZER_MAX_CHANNELS
        {
            return Err(ProductionCustomOptimizerRasterError::Construction(format!(
                "Authorized raster channel count {channel_count} is outside the Custom Optimizer bound."
            )));
        }
        let lab_transform = ProductionPcsLabTransform::new(
            source_model,
            RuntimeIccProfile::Embedded(source_icc),
            recipe.rendering_intent,
            recipe.black_point_compensation,
        )
        .map_err(ProductionCustomOptimizerRasterError::Construction)?;
        Ok(Self {
            source_model,
            lab_transform,
            runtime,
            channel_count,
            target_bit_depth: recipe.target.bit_depth,
            lab_scratch: Vec::new(),
        })
    }

    fn transform_u8_chunk(
        &mut self,
        source: &[u16],
        destination: &mut [u8],
    ) -> Result<(), ProductionCustomOptimizerRasterError> {
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
                let value =
                    quantize_normalized_coverage(normalized[channel_index], 8, quantization)
                        .map_err(|error| ProductionCustomOptimizerRasterError::Quantization {
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
    ) -> Result<(), ProductionCustomOptimizerRasterError> {
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
                destination[start + channel_index] =
                    quantize_normalized_coverage(normalized[channel_index], 16, quantization)
                        .map_err(|error| ProductionCustomOptimizerRasterError::Quantization {
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
    ) -> Result<usize, ProductionCustomOptimizerRasterError> {
        let source_channels = match self.source_model {
            IccSourceModel::Rgb => 3,
            IccSourceModel::Cmyk => 4,
        };
        if source.len() % source_channels != 0 {
            return Err(ProductionCustomOptimizerRasterError::SourceTopology {
                source_channels,
                sample_count: source.len(),
            });
        }
        let pixels = source.len() / source_channels;
        if pixels > MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS {
            return Err(ProductionCustomOptimizerRasterError::ChunkTooLarge {
                pixels,
                maximum: MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS,
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
                .map_err(ProductionCustomOptimizerRasterError::LabTransform)?,
            IccSourceModel::Cmyk => self
                .lab_transform
                .transform_cmyk_chunk(
                    samples_as_arrays::<4>(source)?,
                    &mut self.lab_scratch[..pixels],
                )
                .map_err(ProductionCustomOptimizerRasterError::LabTransform)?,
        }
        Ok(pixels)
    }

    fn lookup(
        &self,
        pixel_index: usize,
        lab: PcsLabPixel,
        normalized: &mut [f32],
    ) -> Result<(), ProductionCustomOptimizerRasterError> {
        self.runtime
            .lookup_into(
                LabColor {
                    l: lab[0],
                    a: lab[1],
                    b: lab[2],
                },
                normalized,
            )
            .map_err(|error| ProductionCustomOptimizerRasterError::Lookup { pixel_index, error })
    }

    fn require_bit_depth(&self, requested: u8) -> Result<(), ProductionCustomOptimizerRasterError> {
        if self.target_bit_depth == requested {
            Ok(())
        } else {
            Err(ProductionCustomOptimizerRasterError::WrongTargetBitDepth {
                expected: self.target_bit_depth,
                requested,
            })
        }
    }

    fn validate_destination_len(
        &self,
        pixels: usize,
        actual: usize,
    ) -> Result<(), ProductionCustomOptimizerRasterError> {
        let expected = pixels
            .checked_mul(self.channel_count)
            .ok_or(ProductionCustomOptimizerRasterError::SizeOverflow)?;
        if expected == actual {
            Ok(())
        } else {
            Err(ProductionCustomOptimizerRasterError::DestinationTopology {
                expected_samples: expected,
                actual_samples: actual,
            })
        }
    }
}

fn verify_source_icc(
    source_icc: &[u8],
    expected_sha256: &str,
) -> Result<(), ProductionCustomOptimizerRasterError> {
    let expected = expected_sha256.trim();
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(
            ProductionCustomOptimizerRasterError::InvalidSourceProfileIdentity(
                "Captured Source ICC identity is not a full SHA-256.".to_owned(),
            ),
        );
    }
    let actual = format!("{:x}", Sha256::digest(source_icc));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(
            ProductionCustomOptimizerRasterError::SourceProfileMismatch {
                expected: expected.to_owned(),
                actual,
            },
        )
    }
}

fn samples_as_arrays<const N: usize>(
    samples: &[u16],
) -> Result<&[[u16; N]], ProductionCustomOptimizerRasterError> {
    if samples.len() % N != 0 {
        return Err(ProductionCustomOptimizerRasterError::SourceTopology {
            source_channels: N,
            sample_count: samples.len(),
        });
    }
    // SAFETY: `[u16; N]` has u16 alignment and the exact length divisibility is
    // checked above. The returned view cannot outlive the source slice.
    Ok(unsafe {
        std::slice::from_raw_parts(samples.as_ptr().cast::<[u16; N]>(), samples.len() / N)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lcms2::Profile;
    use sha2::{Digest, Sha256};

    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::conversion_recipe::recipe_sha256;
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::device_characterization::{CharacterizationIdentity, LabColor};
    use crate::inverse_lut_identity::{
        INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
        InverseLutBuildPolicy, InverseLutContinuityFieldMethod, InverseLutForwardModelIdentity,
        InverseLutForwardModelMethod, InverseLutInterpolationMethod,
        InverseLutLocalForwardModelConfigIdentity, InverseLutNumericalPrecision,
        InverseLutOutputQuantization, InverseLutValidityEncoding, LabGridSpec,
    };
    use crate::inverse_lut_path_validation::{
        InverseLutPathDiagnostic, InverseLutValidationPathKind,
    };
    use crate::inverse_lut_threshold_set::{
        INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
        INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
        InverseLutCalibrationSolverFamily, InverseLutThresholdCalibrationObservation,
        InverseLutThresholdSetMethod,
    };
    use crate::inverse_lut_validation::{InverseLutValidationSample, summarize_validation_samples};
    use crate::inverse_lut_validation_reference::InverseLutValidationReferenceMethod;
    use crate::model::IccProfileIdentity;
    use crate::production_colorimetry::{
        PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION, ProductionPcsCompatibilityMethod,
    };

    const CHANNEL_COUNT: usize = 4;

    fn characterization_id() -> String {
        crate::color_conversion_test_support::characterization_id()
    }

    fn channels() -> Vec<String> {
        ["Cyan", "Magenta", "Yellow", "Black"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn srgb_bytes() -> Vec<u8> {
        Profile::new_srgb().icc().unwrap()
    }

    fn recipe(bit_depth: u8, source_icc: &[u8]) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Raster fixture sRGB".to_owned(),
                sha256: format!("{:x}", Sha256::digest(source_icc)),
            },
            target: ConversionTargetDefinition {
                name: "Raster fixture target".to_owned(),
                channels: channels()
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name,
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(1.0),
                    })
                    .collect(),
                bit_depth,
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: None,
                device_link_path: None,
                characterization_id: Some(characterization_id()),
                total_ink_limit: Some(1.8),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    fn identity(
        recipe: &ConversionRecipe,
        a_min: f64,
        a_max: f64,
        b_min: f64,
        b_max: f64,
    ) -> crate::inverse_lut_identity::InverseLutIdentityRecord {
        crate::inverse_lut_identity::InverseLutIdentityRecord {
            schema_version: INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
            characterization_id: characterization_id(),
            forward_model: InverseLutForwardModelIdentity {
                method: InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1,
                config: InverseLutLocalForwardModelConfigIdentity {
                    neighbor_count: 2,
                    distance_power: 2.0,
                    max_support_distance: 0.5,
                },
            },
            recipe_sha256: recipe_sha256(recipe).unwrap(),
            channel_names: channels(),
            target_bit_depth: recipe.target.bit_depth,
            build_policy: InverseLutBuildPolicy {
                schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
                grid: LabGridSpec {
                    l_min: 0.0,
                    l_max: 100.0,
                    l_samples: 2,
                    a_min,
                    a_max,
                    a_samples: 2,
                    b_min,
                    b_max,
                    b_samples: 2,
                },
                interpolation: InverseLutInterpolationMethod::TrilinearV1,
                validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
                numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
                output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
                continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
            },
        }
    }

    fn payload_sha256(validity: &[bool], coverages: &[f32]) -> String {
        let mut hasher = Sha256::new();
        for valid in validity {
            hasher.update([u8::from(*valid)]);
        }
        for value in coverages.iter().copied() {
            hasher.update(value.to_bits().to_le_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    fn lut(recipe: &ConversionRecipe, narrow_ab: bool) -> VerifiedInverseLutArtifact {
        let identity = if narrow_ab {
            identity(recipe, -1.0, 1.0, -1.0, 1.0)
        } else {
            identity(recipe, -128.0, 128.0, -128.0, 128.0)
        };
        let validity = vec![true; 8];
        let pattern = [0.0f32, 0.5, 1.0, 0.25];
        let mut coverages = Vec::with_capacity(validity.len() * CHANNEL_COUNT);
        for _ in 0..validity.len() {
            coverages.extend_from_slice(&pattern);
        }
        VerifiedInverseLutArtifact {
            identity_content_id: identity.content_id().unwrap(),
            identity,
            payload_sha256: payload_sha256(&validity, &coverages),
            validity,
            coverages,
        }
    }

    fn kernel(bit_depth: u8, narrow_ab: bool) -> CustomOptimizerRasterKernel {
        let source_icc = srgb_bytes();
        let recipe = recipe(bit_depth, &source_icc);
        let lut = lut(&recipe, narrow_ab);
        CustomOptimizerRasterKernel::new(IccSourceModel::Rgb, &source_icc, &lut, &recipe).unwrap()
    }

    fn paths() -> Vec<InverseLutPathDiagnostic> {
        [
            InverseLutValidationPathKind::NeutralAxis,
            InverseLutValidationPathKind::NearNeutralWarm,
            InverseLutValidationPathKind::NearNeutralCool,
            InverseLutValidationPathKind::AAxis,
            InverseLutValidationPathKind::BAxis,
            InverseLutValidationPathKind::AbDiagonal,
            InverseLutValidationPathKind::AbOpposedDiagonal,
        ]
        .into_iter()
        .map(|kind| InverseLutPathDiagnostic {
            kind,
            sample_count: 5,
            unsupported_samples: 0,
            max_channel_jump: Some(0.0),
            max_normalized_channel_jump: Some(0.0),
            max_vector_l1_jump: Some(0.0),
            max_vector_l2_jump: Some(0.0),
            max_total_ink_jump: Some(0.0),
            dominant_channel_switches: Some(0),
            max_channel_second_difference: Some(0.0),
            max_normalized_channel_second_difference: Some(0.0),
            max_vector_l1_second_difference: Some(0.0),
            max_vector_l2_second_difference: Some(0.0),
            max_total_ink_second_difference: Some(0.0),
            continuity_violation_count: Some(0),
            curvature_violation_count: Some(0),
        })
        .collect()
    }

    #[test]
    fn constant_lut_quantizes_deterministically_to_u8() {
        let mut kernel = kernel(8, false);
        let source = [0u16, 0, 0, u16::MAX, u16::MAX, u16::MAX];
        let mut first = [0u8; 8];
        let mut second = [0u8; 8];
        kernel.transform_u8_chunk(&source, &mut first).unwrap();
        let capacity = kernel.lab_scratch.capacity();
        kernel.transform_u8_chunk(&source, &mut second).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[..4], &[0, 128, 255, 64]);
        assert_eq!(&first[4..], &[0, 128, 255, 64]);
        assert!(kernel.lab_scratch.capacity() >= capacity);
    }

    #[test]
    fn constant_lut_quantizes_deterministically_to_u16() {
        let mut kernel = kernel(16, false);
        let source = [u16::MAX, 0, 0];
        let mut output = [0u16; 4];
        kernel.transform_u16_chunk(&source, &mut output).unwrap();
        assert_eq!(output, [0, 32_768, 65_535, 16_384]);
    }

    #[test]
    fn target_precision_and_flat_topology_are_strict() {
        let mut kernel = kernel(16, false);
        assert!(matches!(
            kernel.transform_u8_chunk(&[0, 0, 0], &mut [0; 4]),
            Err(ProductionCustomOptimizerRasterError::WrongTargetBitDepth { .. })
        ));
        assert!(matches!(
            kernel.transform_u16_chunk(&[0, 0], &mut [0; 4]),
            Err(ProductionCustomOptimizerRasterError::SourceTopology { .. })
        ));
        assert!(matches!(
            kernel.transform_u16_chunk(&[0, 0, 0], &mut [0; 3]),
            Err(ProductionCustomOptimizerRasterError::DestinationTopology { .. })
        ));
    }

    #[test]
    fn chunk_bound_is_checked_before_lab_scratch_growth() {
        let mut kernel = kernel(16, false);
        let source = vec![0u16; (MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS + 1) * 3];
        assert!(matches!(
            kernel.transform_u16_chunk(&source, &mut []),
            Err(ProductionCustomOptimizerRasterError::ChunkTooLarge { .. })
        ));
        assert!(kernel.lab_scratch.is_empty());
    }

    #[test]
    fn out_of_domain_lab_fails_without_fallback() {
        let mut kernel = kernel(16, true);
        let mut output = [0u16; 4];
        let error = kernel
            .transform_u16_chunk(&[u16::MAX, 0, 0], &mut output)
            .unwrap_err();
        assert!(matches!(
            error,
            ProductionCustomOptimizerRasterError::Lookup {
                error: InverseLutLookupError::OutOfDomain { .. },
                ..
            }
        ));
    }

    #[test]
    fn source_icc_identity_is_rehashed_before_authorization() {
        let source_icc = srgb_bytes();
        let mut recipe = recipe(16, &source_icc);
        recipe.source_profile_identity.sha256 = "a".repeat(64);
        let lut = lut(&recipe, false);
        let model = crate::color_conversion_test_support::default_local_model();
        let thresholds = {
            let mut value = InverseLutValidationThresholdSet::provisional_v1();
            value.method = InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1;
            value
        };
        let sample = InverseLutValidationSample {
            supported: true,
            lut_delta_e00: Some(0.1),
            reference_delta_e00: Some(0.1),
            lut_vs_reference_delta_e00: Some(0.0),
            ink_l1: Some(0.0),
            ink_l2: Some(0.0),
            max_channel_deviation: Some(0.0),
            u8_quantization_l1: Some(0.0),
            u16_quantization_l1: Some(0.0),
            constraints_preserved: true,
        };
        let report = summarize_validation_samples(
            lut.identity_content_id.clone(),
            lut.payload_sha256.clone(),
            recipe_sha256(&recipe).unwrap(),
            characterization_id(),
            thresholds.content_id().unwrap(),
            thresholds.policy,
            InverseLutValidationReferenceMethod::IndependentPointSolveV1,
            paths(),
            &[sample],
        )
        .unwrap();
        let validation = VerifiedInverseLutValidationArtifact {
            report_content_id: report.content_id().unwrap(),
            report,
        };
        let manifest = InverseLutThresholdCalibrationManifest {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            threshold_set_content_id: thresholds.content_id().unwrap(),
            observations: vec![
                InverseLutThresholdCalibrationObservation {
                    solver_family: InverseLutCalibrationSolverFamily::IndependentV1,
                    characterization_id: characterization_id(),
                    recipe_sha256: recipe_sha256(&recipe).unwrap(),
                    lut_identity_content_id: lut.identity_content_id.clone(),
                    validation_report_content_id: validation.report_content_id.clone(),
                },
                InverseLutThresholdCalibrationObservation {
                    solver_family: InverseLutCalibrationSolverFamily::PositiveContinuityV2,
                    characterization_id: characterization_id(),
                    recipe_sha256: recipe_sha256(&recipe).unwrap(),
                    lut_identity_content_id: lut.identity_content_id.clone(),
                    validation_report_content_id: format!("sha256:{}", "f".repeat(64)),
                },
            ],
        };
        let approval = InverseLutThresholdCalibrationApproval {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            threshold_set_content_id: thresholds.content_id().unwrap(),
            calibration_manifest_content_id: manifest.content_id().unwrap(),
        };
        let pcs = ValidatedProductionPcsCompatibility {
            schema_version: PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION,
            method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            characterization_id: characterization_id(),
            canonical_illuminant: "D50".to_owned(),
            canonical_observer: "2deg".to_owned(),
        };
        let error = ProductionCustomOptimizerRasterTransform::authorize(
            IccSourceModel::Rgb,
            &source_icc,
            &lut,
            &validation,
            &thresholds,
            &manifest,
            &approval,
            &pcs,
            &recipe,
            &model,
        )
        .err()
        .expect("stale Source ICC identity must fail before authorization");
        assert!(matches!(
            error,
            ProductionCustomOptimizerRasterError::SourceProfileMismatch { .. }
        ));
    }

    #[test]
    fn structurally_valid_evidence_still_fails_closed_without_approved_calibration_id() {
        let source_icc = srgb_bytes();
        let recipe = recipe(16, &source_icc);
        let lut = lut(&recipe, false);
        let model = crate::color_conversion_test_support::default_local_model();
        let mut thresholds = InverseLutValidationThresholdSet::provisional_v1();
        thresholds.method = InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1;
        let sample = InverseLutValidationSample {
            supported: true,
            lut_delta_e00: Some(0.1),
            reference_delta_e00: Some(0.1),
            lut_vs_reference_delta_e00: Some(0.0),
            ink_l1: Some(0.0),
            ink_l2: Some(0.0),
            max_channel_deviation: Some(0.0),
            u8_quantization_l1: Some(0.0),
            u16_quantization_l1: Some(0.0),
            constraints_preserved: true,
        };
        let report = summarize_validation_samples(
            lut.identity_content_id.clone(),
            lut.payload_sha256.clone(),
            recipe_sha256(&recipe).unwrap(),
            characterization_id(),
            thresholds.content_id().unwrap(),
            thresholds.policy,
            InverseLutValidationReferenceMethod::IndependentPointSolveV1,
            paths(),
            &[sample],
        )
        .unwrap();
        let validation = VerifiedInverseLutValidationArtifact {
            report_content_id: report.content_id().unwrap(),
            report,
        };
        let manifest = InverseLutThresholdCalibrationManifest {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            threshold_set_content_id: thresholds.content_id().unwrap(),
            observations: vec![
                InverseLutThresholdCalibrationObservation {
                    solver_family: InverseLutCalibrationSolverFamily::IndependentV1,
                    characterization_id: characterization_id(),
                    recipe_sha256: recipe_sha256(&recipe).unwrap(),
                    lut_identity_content_id: lut.identity_content_id.clone(),
                    validation_report_content_id: validation.report_content_id.clone(),
                },
                InverseLutThresholdCalibrationObservation {
                    solver_family: InverseLutCalibrationSolverFamily::PositiveContinuityV2,
                    characterization_id: characterization_id(),
                    recipe_sha256: recipe_sha256(&recipe).unwrap(),
                    lut_identity_content_id: lut.identity_content_id.clone(),
                    validation_report_content_id: format!("sha256:{}", "f".repeat(64)),
                },
            ],
        };
        let approval = InverseLutThresholdCalibrationApproval {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            threshold_set_content_id: thresholds.content_id().unwrap(),
            calibration_manifest_content_id: manifest.content_id().unwrap(),
        };
        let pcs = ValidatedProductionPcsCompatibility {
            schema_version: PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION,
            method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            characterization_id: characterization_id(),
            canonical_illuminant: "D50".to_owned(),
            canonical_observer: "2deg".to_owned(),
        };
        let error = ProductionCustomOptimizerRasterTransform::authorize(
            IccSourceModel::Rgb,
            &source_icc,
            &lut,
            &validation,
            &thresholds,
            &manifest,
            &approval,
            &pcs,
            &recipe,
            &model,
        )
        .err()
        .expect("empty production approval allowlist must remain fail-closed");
        assert!(matches!(
            error,
            ProductionCustomOptimizerRasterError::Authorization(
                InverseLutProductionEligibilityError::CalibrationApprovalNotProductionApproved { .. }
            )
        ));
    }
}
