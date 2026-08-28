use std::fs;

use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_analytics::{
    ConversionUsageAccumulator, ConversionUsageReport, NeutralClassification,
};
use crate::conversion_recipe::recipe_sha256;
use crate::conversion_transaction::{CapturedSourceProfile, ConversionCancellation};
use crate::custom_optimizer_evidence::{
    CapturedCustomOptimizerEvidence, load_and_authorize_custom_optimizer_evidence,
};
use crate::custom_optimizer_raster_transform::ProductionCustomOptimizerRasterTransform;
use crate::source_profile_fallback::srgb_fallback_icc;

mod standard {
    include!("conversion_candidate_preview.rs");
}

pub use standard::{CandidatePreviewInput, CandidatePreviewResult};

const PREVIEW_CHUNK_PIXELS: usize = 16 * 1024;

/// Preserve the existing ICC / DeviceLink Candidate Preview API. Custom Optimizer
/// deliberately remains fail-closed unless the caller supplies the exact immutable
/// evidence capture through `render_candidate_preview_with_custom_optimizer_evidence`.
pub fn render_candidate_preview(
    input: CandidatePreviewInput,
    cancellation: &ConversionCancellation,
) -> Result<CandidatePreviewResult, String> {
    check_cancelled(cancellation)?;
    if input.recipe.engine_mode == ConversionEngineMode::CustomOptimizer {
        return Err(
            "Custom Optimizer candidate preview requires immutable approved production evidence. Reopen the exact #205/#191 evidence before rendering this recipe."
                .to_owned(),
        );
    }
    standard::render_candidate_preview(input, cancellation)
}

/// Render a Custom Optimizer candidate only after reopening and independently
/// re-authorizing the exact captured production evidence.
///
/// This path intentionally uses the same `ProductionCustomOptimizerRasterTransform`
/// constructor as the filesystem production worker. The Candidate Preview therefore
/// cannot acquire authority from UI state, a preset, or a previously serialized
/// eligibility token. If the measured approval allowlist is empty/stale, rendering
/// fails closed before any candidate samples are returned.
pub fn render_candidate_preview_with_custom_optimizer_evidence(
    input: CandidatePreviewInput,
    evidence: &CapturedCustomOptimizerEvidence,
    cancellation: &ConversionCancellation,
) -> Result<CandidatePreviewResult, String> {
    if input.recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return standard::render_candidate_preview(input, cancellation);
    }

    input.recipe.validate().map_err(|errors| {
        format!("Candidate preview recipe is invalid: {}", errors.join(" "))
    })?;
    check_cancelled(cancellation)?;
    validate_source_planes(&input)?;

    let source_icc = load_source_icc(
        &input.source_profile,
        input.embedded_source_icc.as_deref(),
        &input.recipe.source_profile_identity.sha256,
    )?;
    check_cancelled(cancellation)?;

    let loaded = load_and_authorize_custom_optimizer_evidence(evidence, &input.recipe)
        .map_err(|error| format!("Custom Optimizer candidate evidence rejected: {error:?}"))?;
    let mut transform = ProductionCustomOptimizerRasterTransform::authorize(
        input.source_model,
        &source_icc,
        &loaded.lut,
        &loaded.validation,
        &evidence.threshold_set,
        &evidence.calibration_manifest,
        &evidence.calibration_approval,
        &loaded.pcs_compatibility,
        &input.recipe,
        &loaded.model,
    )
    .map_err(|error| format!("Cannot authorize Custom Optimizer candidate transform: {error:?}"))?;
    if transform.eligibility() != &loaded.eligibility {
        return Err(
            "Custom Optimizer candidate authorization identity changed after evidence reload."
                .to_owned(),
        );
    }
    if transform.output_channels() != input.recipe.target.channels.len() {
        return Err(
            "Custom Optimizer candidate transform topology does not match the exact recipe target."
                .to_owned(),
        );
    }
    if transform.target_bit_depth() != input.recipe.target.bit_depth {
        return Err(
            "Custom Optimizer candidate transform bit depth does not match the exact recipe target."
                .to_owned(),
        );
    }

    let planes = transform_custom_optimizer_planes(&input, &mut transform, cancellation)?;
    let histograms = build_histograms(&planes);
    let usage = build_usage_report(&input.recipe, &planes, cancellation)?;
    Ok(CandidatePreviewResult {
        width: input.width,
        height: input.height,
        recipe_sha256: recipe_sha256(&input.recipe)?,
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
            "Candidate {:?} source requires {source_channels} planes; found {}.",
            input.source_model,
            input.source_planes.len()
        ));
    }
    let pixels = input
        .width
        .checked_mul(input.height)
        .ok_or_else(|| "Candidate preview dimensions overflow pixel count.".to_owned())?;
    if pixels == 0 {
        return Err("Candidate preview raster is empty.".to_owned());
    }
    for (index, plane) in input.source_planes.iter().enumerate() {
        if plane.len() != pixels {
            return Err(format!(
                "Candidate source plane {} has {} samples; expected {pixels}.",
                index + 1,
                plane.len()
            ));
        }
    }
    Ok(())
}

fn transform_custom_optimizer_planes(
    input: &CandidatePreviewInput,
    transform: &mut ProductionCustomOptimizerRasterTransform,
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

    for start in (0..pixels).step_by(PREVIEW_CHUNK_PIXELS) {
        check_cancelled(cancellation)?;
        let end = (start + PREVIEW_CHUNK_PIXELS).min(pixels);
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
                        format!("Custom Optimizer candidate transform failed: {error:?}")
                    })?;
                for local_pixel in 0..chunk_pixels {
                    for channel in 0..output_channels {
                        let sample = destination[local_pixel * output_channels + channel];
                        planes[channel][start + local_pixel] = u16::from(sample) * 257;
                    }
                }
            }
            16 => {
                let mut destination = vec![0u16; chunk_pixels * output_channels];
                transform
                    .transform_u16_chunk(&source, &mut destination)
                    .map_err(|error| {
                        format!("Custom Optimizer candidate transform failed: {error:?}")
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
                    "Custom Optimizer candidate target bit depth {bit_depth} is unsupported."
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
            "Candidate analytics has {} planes for {} target channels.",
            planes.len(),
            recipe.target.channels.len()
        ));
    }
    let pixels = planes
        .first()
        .map(Vec::len)
        .ok_or_else(|| "Candidate analytics requires output planes.".to_owned())?;
    if planes.iter().any(|plane| plane.len() != pixels) {
        return Err("Candidate analytics output planes have inconsistent lengths.".to_owned());
    }

    let mut accumulator = ConversionUsageAccumulator::from_recipe(recipe)?;
    let mut pixel = vec![0u16; planes.len()];
    for index in 0..pixels {
        if index % PREVIEW_CHUNK_PIXELS == 0 {
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
                "Cannot reopen assigned Source ICC {} for Custom Optimizer candidate preview: {error}",
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
            "{label} identity changed before Custom Optimizer candidate preview (expected {}, found {actual}).",
            expected.trim()
        ))
    }
}

fn check_cancelled(cancellation: &ConversionCancellation) -> Result<(), String> {
    if cancellation.is_requested() {
        Err("Candidate preview cancelled because the target/recipe changed.".to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::icc_conversion::IccSourceModel;
    use crate::model::IccProfileIdentity;

    fn custom_recipe() -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "0".repeat(64),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Measured target".to_owned(),
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
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: None,
                device_link_path: None,
                characterization_id: Some(format!("sha256:{}", "c".repeat(64))),
                total_ink_limit: Some(1.8),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    fn custom_input() -> CandidatePreviewInput {
        CandidatePreviewInput {
            width: 1,
            height: 1,
            source_model: IccSourceModel::Rgb,
            source_planes: vec![vec![0], vec![0], vec![0]],
            source_profile: CapturedSourceProfile::Embedded,
            embedded_source_icc: None,
            recipe: custom_recipe(),
        }
    }

    #[test]
    fn custom_optimizer_standard_candidate_api_fails_closed_without_evidence() {
        let error = render_candidate_preview(custom_input(), &ConversionCancellation::default())
            .unwrap_err();
        assert!(error.contains("requires immutable approved production evidence"));
        assert!(!error.contains("ICC/DeviceLink candidate payload"));
    }

    #[test]
    fn cancellation_wins_before_custom_optimizer_evidence_or_profile_io() {
        let cancellation = ConversionCancellation::default();
        cancellation.request();
        let error = render_candidate_preview(custom_input(), &cancellation).unwrap_err();
        assert!(error.contains("cancelled"));
    }

    #[test]
    fn custom_optimizer_candidate_runtime_has_no_serialized_eligibility_bypass() {
        let source = include_str!("conversion_candidate_preview_runtime.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("load_and_authorize_custom_optimizer_evidence"));
        assert!(runtime.contains("ProductionCustomOptimizerRasterTransform::authorize"));
        assert!(!runtime.contains("InverseLutProductionEligibility::new"));
        assert!(!runtime.contains("test_only"));
    }
}
