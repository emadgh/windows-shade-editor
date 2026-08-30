use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_analytics::{
    ConversionUsageAccumulator, ConversionUsageReport, NeutralClassification,
};
use crate::conversion_candidate_preview::{CandidatePreviewInput, CandidatePreviewResult};
use crate::conversion_recipe::recipe_sha256;
use crate::conversion_transaction::{CapturedSourceProfile, ConversionCancellation};
use crate::profile_backed_inverse_lut_artifact::load_profile_backed_inverse_lut_artifact;
use crate::profile_backed_optimizer_authority::ProfileBackedOptimizerAuthority;
use crate::profile_backed_optimizer_raster_transform::ProfileBackedCustomOptimizerRasterTransform;
use crate::source_profile_fallback::srgb_fallback_icc;

const PROFILE_PREVIEW_CHUNK_PIXELS: usize = 16 * 1024;

/// Render a Candidate Preview from the profile-backed Custom Optimizer route.
///
/// Both authority-bearing external inputs are reopened at this boundary: the exact
/// Output ICC path captured by the recipe/authority and the exact profile-backed LUT
/// artifact path supplied by the caller. The raster constructor then independently
/// revalidates Source ICC, Output ICC, recipe, authority and LUT identities before any
/// candidate samples are returned.
pub fn render_profile_backed_candidate_preview(
    input: CandidatePreviewInput,
    authority: &ProfileBackedOptimizerAuthority,
    lut_artifact_path: &Path,
    cancellation: &ConversionCancellation,
) -> Result<CandidatePreviewResult, String> {
    input.recipe.validate().map_err(|errors| {
        format!(
            "Profile-backed candidate preview recipe is invalid: {}",
            errors.join(" ")
        )
    })?;
    if input.recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return Err(
            "Profile-backed candidate preview requires a Custom Optimizer recipe.".to_owned(),
        );
    }
    if input
        .recipe
        .target
        .characterization_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(
            "Measured-characterization recipes must use the measured Candidate Preview authority path."
                .to_owned(),
        );
    }
    check_cancelled(cancellation)?;
    validate_source_planes(&input)?;

    let source_icc = load_source_icc(
        &input.source_profile,
        input.embedded_source_icc.as_deref(),
        &input.recipe.source_profile_identity.sha256,
    )?;
    check_cancelled(cancellation)?;

    let output_path = Path::new(&authority.output_profile_path);
    let output_icc = fs::read(output_path).map_err(|error| {
        format!(
            "Cannot reopen profile-backed Output ICC {} for Candidate Preview: {error}",
            output_path.display()
        )
    })?;
    check_cancelled(cancellation)?;

    let artifact = load_profile_backed_inverse_lut_artifact(lut_artifact_path).map_err(|error| {
        format!(
            "Cannot reopen profile-backed inverse LUT {} for Candidate Preview: {error}",
            lut_artifact_path.display()
        )
    })?;
    check_cancelled(cancellation)?;

    let mut transform = ProfileBackedCustomOptimizerRasterTransform::authorize(
        input.source_model,
        &source_icc,
        &output_icc,
        authority,
        artifact,
        &input.recipe,
    )
    .map_err(|error| {
        format!("Cannot authorize profile-backed Candidate Preview transform: {error:?}")
    })?;
    if transform.output_channels() != input.recipe.target.channels.len() {
        return Err(
            "Profile-backed Candidate Preview topology does not match the exact recipe target."
                .to_owned(),
        );
    }
    if transform.target_bit_depth() != input.recipe.target.bit_depth {
        return Err(
            "Profile-backed Candidate Preview bit depth does not match the exact recipe target."
                .to_owned(),
        );
    }

    let planes = transform_planes(&input, &mut transform, cancellation)?;
    let histograms = build_histograms(&planes);
    let usage = build_usage_report(&input.recipe, &planes, cancellation)?;
    let recipe_sha256 = recipe_sha256(&input.recipe)?;
    Ok(CandidatePreviewResult {
        width: input.width,
        height: input.height,
        recipe_sha256,
        channels: input.recipe.target.channels,
        planes,
        histograms,
        usage,
    })
}

fn validate_source_planes(input: &CandidatePreviewInput) -> Result<(), String> {
    let source_channels = match input.source_model {
        crate::icc_conversion::IccSourceModel::Rgb => 3,
        crate::icc_conversion::IccSourceModel::Cmyk => 4,
    };
    if input.source_planes.len() != source_channels {
        return Err(format!(
            "Profile-backed candidate {:?} source requires {source_channels} planes; found {}.",
            input.source_model,
            input.source_planes.len()
        ));
    }
    let pixels = input
        .width
        .checked_mul(input.height)
        .ok_or_else(|| "Profile-backed Candidate Preview dimensions overflow pixel count.".to_owned())?;
    if pixels == 0 {
        return Err("Profile-backed Candidate Preview raster is empty.".to_owned());
    }
    for (index, plane) in input.source_planes.iter().enumerate() {
        if plane.len() != pixels {
            return Err(format!(
                "Profile-backed candidate source plane {} has {} samples; expected {pixels}.",
                index + 1,
                plane.len()
            ));
        }
    }
    Ok(())
}

fn transform_planes(
    input: &CandidatePreviewInput,
    transform: &mut ProfileBackedCustomOptimizerRasterTransform,
    cancellation: &ConversionCancellation,
) -> Result<Vec<Vec<u16>>, String> {
    let source_channels = match input.source_model {
        crate::icc_conversion::IccSourceModel::Rgb => 3,
        crate::icc_conversion::IccSourceModel::Cmyk => 4,
    };
    let output_channels = transform.output_channels();
    let pixels = input.width * input.height;
    let mut planes = (0..output_channels)
        .map(|_| vec![0u16; pixels])
        .collect::<Vec<_>>();

    for start in (0..pixels).step_by(PROFILE_PREVIEW_CHUNK_PIXELS) {
        check_cancelled(cancellation)?;
        let end = (start + PROFILE_PREVIEW_CHUNK_PIXELS).min(pixels);
        let chunk_pixels = end - start;
        let mut source = Vec::with_capacity(chunk_pixels * source_channels);
        for pixel in start..end {
            for channel in 0..source_channels {
                source.push(input.source_planes[channel][pixel]);
            }
        }

        match transform.target_bit_depth() {
            8 => {
                let mut destination = vec![0u8; chunk_pixels * output_channels];
                transform
                    .transform_u8_chunk(&source, &mut destination)
                    .map_err(|error| {
                        format!("Profile-backed Candidate Preview transform failed: {error:?}")
                    })?;
                for local_pixel in 0..chunk_pixels {
                    for channel in 0..output_channels {
                        planes[channel][start + local_pixel] = u16::from(
                            destination[local_pixel * output_channels + channel],
                        ) * 257;
                    }
                }
            }
            16 => {
                let mut destination = vec![0u16; chunk_pixels * output_channels];
                transform
                    .transform_u16_chunk(&source, &mut destination)
                    .map_err(|error| {
                        format!("Profile-backed Candidate Preview transform failed: {error:?}")
                    })?;
                for local_pixel in 0..chunk_pixels {
                    for channel in 0..output_channels {
                        planes[channel][start + local_pixel] =
                            destination[local_pixel * output_channels + channel];
                    }
                }
            }
            bit_depth => {
                return Err(format!(
                    "Profile-backed Candidate Preview target bit depth {bit_depth} is unsupported."
                ));
            }
        }
    }
    Ok(planes)
}

fn build_histograms(planes: &[Vec<u16>]) -> Vec<[u32; 256]> {
    planes
        .iter()
        .map(|plane| {
            let mut histogram = [0u32; 256];
            for sample in plane {
                histogram[(*sample >> 8) as usize] += 1;
            }
            histogram
        })
        .collect()
}

fn build_usage_report(
    recipe: &ConversionRecipe,
    planes: &[Vec<u16>],
    cancellation: &ConversionCancellation,
) -> Result<ConversionUsageReport, String> {
    if planes.len() != recipe.target.channels.len() {
        return Err(format!(
            "Profile-backed Candidate Preview analytics has {} planes for {} target channels.",
            planes.len(),
            recipe.target.channels.len()
        ));
    }
    let pixels = planes
        .first()
        .map(Vec::len)
        .ok_or_else(|| "Profile-backed Candidate Preview analytics requires output planes.".to_owned())?;
    if planes.iter().any(|plane| plane.len() != pixels) {
        return Err(
            "Profile-backed Candidate Preview output planes have inconsistent lengths."
                .to_owned(),
        );
    }
    let mut accumulator = ConversionUsageAccumulator::from_recipe(recipe)?;
    let mut pixel = vec![0u16; planes.len()];
    for index in 0..pixels {
        if index % PROFILE_PREVIEW_CHUNK_PIXELS == 0 {
            check_cancelled(cancellation)?;
        }
        for (channel, plane) in planes.iter().enumerate() {
            pixel[channel] = plane[index];
        }
        accumulator.observe_u16(&pixel, NeutralClassification::Unknown)?;
    }
    Ok(accumulator.finish())
}

fn load_source_icc(
    source_profile: &CapturedSourceProfile,
    embedded: Option<&[u8]>,
    expected_sha256: &str,
) -> Result<Vec<u8>, String> {
    let bytes = match source_profile {
        CapturedSourceProfile::Embedded => match embedded {
            Some(bytes) => bytes.to_vec(),
            None => srgb_fallback_icc()?,
        },
        CapturedSourceProfile::External { path } => fs::read(path).map_err(|error| {
            format!(
                "Cannot reopen assigned Source ICC {} for profile-backed Candidate Preview: {error}",
                path.display()
            )
        })?,
    };
    verify_sha256(&bytes, expected_sha256, "Source ICC")?;
    Ok(bytes)
}

fn verify_sha256(bytes: &[u8], expected: &str, label: &str) -> Result<(), String> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "{label} identity changed before profile-backed Candidate Preview (expected {}, found {actual}).",
            expected.trim()
        ))
    }
}

fn check_cancelled(cancellation: &ConversionCancellation) -> Result<(), String> {
    if cancellation.is_requested() {
        Err("Profile-backed Candidate Preview cancelled because the target/recipe changed.".to_owned())
    } else {
        Ok(())
    }
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
    use crate::icc_conversion::IccSourceModel;
    use crate::inverse_lut_identity::{
        INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, InverseLutBuildPolicy,
        InverseLutContinuityFieldMethod, InverseLutInterpolationMethod,
        InverseLutNumericalPrecision, InverseLutOutputQuantization, InverseLutValidityEncoding,
        LabGridSpec,
    };
    use crate::model::IccProfileIdentity;
    use crate::profile_backed_inverse_lut_artifact::{
        ProfileBackedInverseLutArtifact, save_profile_backed_inverse_lut_artifact,
    };
    use crate::profile_backed_inverse_lut_builder::{
        BuiltProfileBackedInverseLutPayload, ProfileBackedInverseLutBuildStats,
        ProfileBackedForwardModelMethod,
    };

    fn source_bytes() -> Vec<u8> {
        Profile::new_srgb().icc().unwrap()
    }

    fn unique_temp_path(name: &str, extension: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "shade-profile-candidate-{name}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
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

    fn fixture() -> (
        CandidatePreviewInput,
        ProfileBackedOptimizerAuthority,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let source = source_bytes();
        let output = b"candidate-output-profile".to_vec();
        let output_path = unique_temp_path("output", "icc");
        fs::write(&output_path, &output).unwrap();
        let artifact_path = unique_temp_path("lut", "json");
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Candidate sRGB".to_owned(),
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
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Candidate Output".to_owned(),
                    sha256: format!("{:x}", Sha256::digest(&output)),
                }),
                output_profile_path: Some(output_path.to_string_lossy().into_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: Some(4.0),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        };
        let output_hash = recipe
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
        let built = BuiltProfileBackedInverseLutPayload {
            forward_model_method: ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            forward_model_id: format!("output-icc-sha256:{output_hash}"),
            channel_names: recipe
                .target
                .channels
                .iter()
                .map(|channel| channel.name.clone())
                .collect(),
            target_bit_depth: 16,
            build_policy: policy(),
            validity: vec![true; 8],
            coverages,
            stats: ProfileBackedInverseLutBuildStats {
                node_count: 8,
                supported_nodes: 8,
                ..ProfileBackedInverseLutBuildStats::default()
            },
        };
        let authority =
            ProfileBackedOptimizerAuthority::capture(&recipe, &output, &built).unwrap();
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe, &built).unwrap();
        save_profile_backed_inverse_lut_artifact(&artifact_path, &artifact).unwrap();
        let input = CandidatePreviewInput {
            width: 2,
            height: 1,
            source_model: IccSourceModel::Rgb,
            source_planes: vec![
                vec![0, u16::MAX],
                vec![0, u16::MAX],
                vec![0, u16::MAX],
            ],
            source_profile: CapturedSourceProfile::Embedded,
            embedded_source_icc: Some(source),
            recipe,
        };
        (input, authority, artifact_path, output_path)
    }

    #[test]
    fn candidate_reopens_authority_inputs_and_returns_exact_profile_lut_planes() {
        let (input, authority, artifact_path, output_path) = fixture();
        let result = render_profile_backed_candidate_preview(
            input,
            &authority,
            &artifact_path,
            &ConversionCancellation::default(),
        )
        .unwrap();
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 1);
        assert_eq!(result.planes.len(), 4);
        assert_eq!(result.planes[0], vec![0, 0]);
        assert_eq!(result.planes[1], vec![32_768, 32_768]);
        assert_eq!(result.planes[2], vec![65_535, 65_535]);
        assert_eq!(result.planes[3], vec![16_384, 16_384]);
        let _ = fs::remove_file(artifact_path);
        let _ = fs::remove_file(output_path);
    }

    #[test]
    fn candidate_rejects_output_icc_changed_after_authority_capture() {
        let (input, authority, artifact_path, output_path) = fixture();
        fs::write(&output_path, b"mutated-after-capture").unwrap();
        let error = render_profile_backed_candidate_preview(
            input,
            &authority,
            &artifact_path,
            &ConversionCancellation::default(),
        )
        .unwrap_err();
        assert!(error.contains("OutputProfileBytesMismatch"));
        let _ = fs::remove_file(artifact_path);
        let _ = fs::remove_file(output_path);
    }

    #[test]
    fn candidate_runtime_has_no_measured_eligibility_dependency() {
        let source = include_str!("profile_backed_candidate_preview.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("load_profile_backed_inverse_lut_artifact"));
        assert!(runtime.contains("ProfileBackedCustomOptimizerRasterTransform::authorize"));
        assert!(!runtime.contains("load_and_authorize_custom_optimizer_evidence"));
        assert!(!runtime.contains("InverseLutProductionEligibility"));
        assert!(!runtime.contains("CalibrationManifest"));
    }
}
