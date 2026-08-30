use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use windows_shade_editor::color_conversion::{
    ConversionEngineMode, ConversionRecipe, ConversionRenderingIntent,
};
use windows_shade_editor::conversion_candidate_preview::CandidatePreviewResult;
use windows_shade_editor::conversion_recipe::recipe_sha256;
use windows_shade_editor::conversion_transaction::ConversionCancellation;

use lcms2::{ColorSpaceSignatureExt, Flags, Intent, PixelFormat, Profile, Transform};

use super::conversion_plan::target_channel_rgb;

const PROOF_CHUNK_PIXELS: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateCompositeKind {
    OutputIccSoftProof,
    InkCompositeApproximation,
}

impl CandidateCompositeKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OutputIccSoftProof => "Output ICC soft proof → sRGB",
            Self::InkCompositeApproximation => {
                "Ink composite approximation · no reversible destination proof profile"
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateCompositePreview {
    pub(crate) rgba: Vec<u8>,
    pub(crate) kind: CandidateCompositeKind,
}

pub(crate) fn render_candidate_composite_preview(
    result: &CandidatePreviewResult,
    recipe: &ConversionRecipe,
    cancellation: &ConversionCancellation,
) -> Result<CandidateCompositePreview, String> {
    check_cancelled(cancellation)?;
    let expected_recipe = recipe_sha256(recipe)?;
    if !result.recipe_sha256.eq_ignore_ascii_case(&expected_recipe) {
        return Err(
            "Candidate composite recipe identity does not match the converted target planes."
                .to_owned(),
        );
    }
    validate_result(result)?;

    match recipe.engine_mode {
        ConversionEngineMode::Icc => Ok(CandidateCompositePreview {
            rgba: output_icc_softproof_rgba(result, recipe, cancellation)?,
            kind: CandidateCompositeKind::OutputIccSoftProof,
        }),
        ConversionEngineMode::DeviceLink | ConversionEngineMode::CustomOptimizer => {
            check_cancelled(cancellation)?;
            Ok(CandidateCompositePreview {
                rgba: approximate_ink_composite_rgba(result),
                kind: CandidateCompositeKind::InkCompositeApproximation,
            })
        }
    }
}

fn output_icc_softproof_rgba(
    result: &CandidatePreviewResult,
    recipe: &ConversionRecipe,
    cancellation: &ConversionCancellation,
) -> Result<Vec<u8>, String> {
    let path = recipe
        .target
        .output_profile_path
        .as_deref()
        .ok_or_else(|| "Candidate Output ICC recipe has no target profile path for soft proof.".to_owned())?;
    let identity = recipe
        .target
        .output_profile_identity
        .as_ref()
        .ok_or_else(|| "Candidate Output ICC recipe has no target profile identity for soft proof.".to_owned())?;
    let bytes = fs::read(Path::new(path))
        .map_err(|error| format!("Cannot reopen Output ICC {path} for Candidate soft proof: {error}"))?;
    verify_sha256(&bytes, &identity.sha256, "Output ICC")?;
    check_cancelled(cancellation)?;

    let target = Profile::new_icc(&bytes)
        .map_err(|error| format!("Cannot open Output ICC for Candidate soft proof: {error}"))?;
    if target.color_space().channels() as usize != result.channel_count() {
        return Err(format!(
            "Candidate Output ICC declares {} channels but converted Candidate has {} planes.",
            target.color_space().channels(),
            result.channel_count()
        ));
    }
    let display = Profile::new_srgb();
    match result.channel_count() {
        4 => proof_n::<4>(result, recipe, &target, &display, cancellation),
        5 => proof_n::<5>(result, recipe, &target, &display, cancellation),
        6 => proof_n::<6>(result, recipe, &target, &display, cancellation),
        7 => proof_n::<7>(result, recipe, &target, &display, cancellation),
        8 => proof_n::<8>(result, recipe, &target, &display, cancellation),
        9 => proof_n::<9>(result, recipe, &target, &display, cancellation),
        10 => proof_n::<10>(result, recipe, &target, &display, cancellation),
        11 => proof_n::<11>(result, recipe, &target, &display, cancellation),
        12 => proof_n::<12>(result, recipe, &target, &display, cancellation),
        channels => Err(format!(
            "Candidate soft proof supports 4..=12 target channels; found {channels}."
        )),
    }
}

fn proof_n<const N: usize>(
    result: &CandidatePreviewResult,
    recipe: &ConversionRecipe,
    target: &Profile,
    display: &Profile,
    cancellation: &ConversionCancellation,
) -> Result<Vec<u8>, String> {
    let input_format = target_pixel_format::<N>()?;
    let intent = lcms_intent(recipe.rendering_intent);
    let transform: Transform<[u16; N], [u16; 3]> = if recipe.black_point_compensation {
        Transform::new_flags(
            target,
            input_format,
            display,
            PixelFormat::RGB_16,
            intent,
            Flags::BLACKPOINT_COMPENSATION,
        )
    } else {
        Transform::new(
            target,
            input_format,
            display,
            PixelFormat::RGB_16,
            intent,
        )
    }
    .map_err(|error| format!("Cannot create Output ICC → sRGB Candidate soft-proof transform: {error}"))?;

    let pixels = result.width.saturating_mul(result.height);
    let mut rgba = Vec::with_capacity(pixels.saturating_mul(4));
    for start in (0..pixels).step_by(PROOF_CHUNK_PIXELS) {
        check_cancelled(cancellation)?;
        let end = (start + PROOF_CHUNK_PIXELS).min(pixels);
        let mut source = vec![[0u16; N]; end - start];
        for (local, pixel) in (start..end).enumerate() {
            for channel in 0..N {
                source[local][channel] = result.planes[channel][pixel];
            }
        }
        let mut rgb = vec![[0u16; 3]; end - start];
        transform.transform_pixels(&source, &mut rgb);
        for pixel in rgb {
            rgba.extend_from_slice(&[
                u16_to_u8(pixel[0]),
                u16_to_u8(pixel[1]),
                u16_to_u8(pixel[2]),
                255,
            ]);
        }
    }
    Ok(rgba)
}

fn approximate_ink_composite_rgba(result: &CandidatePreviewResult) -> Vec<u8> {
    let pixels = result.width.saturating_mul(result.height);
    let mut rgba = Vec::with_capacity(pixels.saturating_mul(4));
    for pixel in 0..pixels {
        let mut rgb = [1.0f32; 3];
        for (index, channel) in result.channels.iter().enumerate() {
            let coverage = result.planes[index][pixel] as f32 / u16::MAX as f32;
            let tint = channel
                .display_rgb
                .unwrap_or_else(|| target_channel_rgb(&channel.name, index));
            let tint = [
                tint[0] as f32 / 255.0,
                tint[1] as f32 / 255.0,
                tint[2] as f32 / 255.0,
            ];
            let strength = (coverage * channel.solidity).clamp(0.0, 1.0);
            for component in 0..3 {
                rgb[component] =
                    rgb[component] * (1.0 - strength) + tint[component] * strength;
            }
        }
        rgba.extend_from_slice(&[
            (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            255,
        ]);
    }
    rgba
}

fn validate_result(result: &CandidatePreviewResult) -> Result<(), String> {
    let pixels = result
        .width
        .checked_mul(result.height)
        .ok_or_else(|| "Candidate composite dimensions overflow pixel count.".to_owned())?;
    if pixels == 0 || result.planes.is_empty() {
        return Err("Candidate composite requires converted target planes.".to_owned());
    }
    if result.planes.len() != result.channels.len() {
        return Err("Candidate composite channel metadata does not match target planes.".to_owned());
    }
    if result.planes.iter().any(|plane| plane.len() != pixels) {
        return Err("Candidate composite target planes have inconsistent lengths.".to_owned());
    }
    Ok(())
}

fn target_pixel_format<const N: usize>() -> Result<PixelFormat, String> {
    match N {
        4 => Ok(PixelFormat::CMYK_16),
        5 => Ok(PixelFormat::CMYK5_16),
        6 => Ok(PixelFormat::CMYK6_16),
        7 => Ok(PixelFormat::CMYK7_16),
        8 => Ok(PixelFormat::CMYK8_16),
        9 => Ok(PixelFormat::CMYK9_16),
        10 => Ok(PixelFormat::CMYK10_16),
        11 => Ok(PixelFormat::CMYK11_16),
        12 => Ok(PixelFormat::CMYK12_16),
        _ => Err(format!("Unsupported Candidate proof topology: {N} channels.")),
    }
}

fn lcms_intent(intent: ConversionRenderingIntent) -> Intent {
    match intent {
        ConversionRenderingIntent::Perceptual => Intent::Perceptual,
        ConversionRenderingIntent::RelativeColorimetric => Intent::RelativeColorimetric,
        ConversionRenderingIntent::Saturation => Intent::Saturation,
        ConversionRenderingIntent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
    }
}

fn u16_to_u8(value: u16) -> u8 {
    ((u32::from(value) * 255 + 32_767) / 65_535) as u8
}

fn verify_sha256(bytes: &[u8], expected: &str, label: &str) -> Result<(), String> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "{label} identity changed before Candidate soft proof (expected {}, found {actual}).",
            expected.trim()
        ))
    }
}

fn check_cancelled(cancellation: &ConversionCancellation) -> Result<(), String> {
    if cancellation.is_requested() {
        Err("Candidate soft proof cancelled because the target/recipe changed.".to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_shade_editor::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionTargetDefinition, SeparationStrategy,
        TargetChannelDefinition,
    };
    use windows_shade_editor::conversion_analytics::{ConversionUsageReport, CoveragePercentiles};
    use windows_shade_editor::model::IccProfileIdentity;

    fn usage() -> ConversionUsageReport {
        ConversionUsageReport {
            pixel_count: 1,
            channels: vec![],
            mean_total_ink: 0.0,
            peak_total_ink: 0.0,
            total_ink_percentiles: CoveragePercentiles {
                p50: 0.0,
                p95: 0.0,
                p99: 0.0,
            },
            total_ink_limit_hit_percent: None,
            neutral_black_share: None,
        }
    }

    fn recipe(engine_mode: ConversionEngineMode) -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "1".repeat(64),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Target".to_owned(),
                channels: ["Cyan", "Magenta", "Yellow", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: None,
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: (engine_mode == ConversionEngineMode::Icc).then(|| {
                    IccProfileIdentity {
                        description: "Output".to_owned(),
                        sha256: "2".repeat(64),
                    }
                }),
                output_profile_path: (engine_mode == ConversionEngineMode::Icc)
                    .then(|| "definitely-missing-output.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        }
    }

    fn result(recipe: &ConversionRecipe) -> CandidatePreviewResult {
        CandidatePreviewResult {
            width: 1,
            height: 1,
            recipe_sha256: recipe_sha256(recipe).unwrap(),
            channels: recipe.target.channels.clone(),
            planes: vec![vec![0]; 4],
            histograms: vec![[0; 256]; 4],
            usage: usage(),
        }
    }

    #[test]
    fn output_icc_never_falls_back_to_static_ink_tints_when_proof_profile_is_missing() {
        let recipe = recipe(ConversionEngineMode::Icc);
        let error = render_candidate_composite_preview(
            &result(&recipe),
            &recipe,
            &ConversionCancellation::default(),
        )
        .unwrap_err();
        assert!(error.contains("Cannot reopen Output ICC"));
    }

    #[test]
    fn devicelink_is_explicitly_an_ink_composite_approximation_without_destination_profile() {
        let recipe = recipe(ConversionEngineMode::DeviceLink);
        let composite = render_candidate_composite_preview(
            &result(&recipe),
            &recipe,
            &ConversionCancellation::default(),
        )
        .unwrap();
        assert_eq!(composite.kind, CandidateCompositeKind::InkCompositeApproximation);
        assert_eq!(composite.rgba.len(), 4);
    }

    #[test]
    fn stale_recipe_identity_is_rejected_before_proofing() {
        let recipe = recipe(ConversionEngineMode::DeviceLink);
        let mut result = result(&recipe);
        result.recipe_sha256 = "0".repeat(64);
        let error = render_candidate_composite_preview(
            &result,
            &recipe,
            &ConversionCancellation::default(),
        )
        .unwrap_err();
        assert!(error.contains("recipe identity"));
    }
}
