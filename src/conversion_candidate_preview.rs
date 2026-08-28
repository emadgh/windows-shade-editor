use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe, TargetChannelDefinition};
use crate::conversion_analytics::{
    ConversionUsageAccumulator, ConversionUsageReport, NeutralClassification,
};
use crate::conversion_recipe::recipe_sha256;
use crate::conversion_transaction::{CapturedSourceProfile, ConversionCancellation};
use crate::devicelink_conversion::ProductionDeviceLinkTransform;
use crate::icc_conversion::{IccSourceModel, ProductionCmykTransform, RuntimeIccProfile};
use crate::nchannel_icc::ProductionNChannelTransform;
use crate::source_profile_fallback::srgb_fallback_icc;

const PREVIEW_CHUNK_PIXELS: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct CandidatePreviewInput {
    pub width: usize,
    pub height: usize,
    pub source_model: IccSourceModel,
    /// Source-adjusted downsampled planes in production working polarity.
    pub source_planes: Vec<Vec<u16>>,
    pub source_profile: CapturedSourceProfile,
    pub embedded_source_icc: Option<Vec<u8>>,
    pub recipe: ConversionRecipe,
}

#[derive(Clone, Debug)]
pub struct CandidatePreviewResult {
    pub width: usize,
    pub height: usize,
    pub recipe_sha256: String,
    pub channels: Vec<TargetChannelDefinition>,
    pub planes: Vec<Vec<u16>>,
    pub histograms: Vec<[u32; 256]>,
    /// Coverage/limit diagnostics computed from these exact cached candidate
    /// planes under the same immutable recipe. Neutrality remains unknown until
    /// a valid PCS/characterization classifier is available.
    pub usage: ConversionUsageReport,
}

impl CandidatePreviewResult {
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
}

/// Execute the same characterized ICC/N-channel/DeviceLink transforms used by
/// production conversion, but against an already downsampled Source-adjusted raster.
/// No TIFF, Production project, or Source state is written by this function.
pub fn render_candidate_preview(
    input: CandidatePreviewInput,
    cancellation: &ConversionCancellation,
) -> Result<CandidatePreviewResult, String> {
    input.recipe.validate().map_err(|errors| {
        format!("Candidate preview recipe is invalid: {}", errors.join(" "))
    })?;
    check_cancelled(cancellation)?;

    let source_channels = match input.source_model {
        IccSourceModel::Rgb => 3,
        IccSourceModel::Cmyk => 4,
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

    let source_icc = load_source_icc(
        &input.source_profile,
        input.embedded_source_icc.as_deref(),
        &input.recipe.source_profile_identity.sha256,
    )?;
    let target_icc = load_target_icc(&input.recipe)?;
    check_cancelled(cancellation)?;

    let planes = match input.recipe.target.channels.len() {
        4 => transform_4(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        5 => transform_n::<5>(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        6 => transform_n::<6>(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        7 => transform_n::<7>(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        8 => transform_n::<8>(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        9 => transform_n::<9>(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        10 => transform_n::<10>(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        11 => transform_n::<11>(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        12 => transform_n::<12>(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        channels => {
            return Err(format!(
                "Candidate preview supports production target topology 4..=12 channels; found {channels}."
            ));
        }
    };
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

fn transform_4(
    model: IccSourceModel,
    planes: &[Vec<u16>],
    source_icc: &[u8],
    target_icc: &[u8],
    recipe: &ConversionRecipe,
    cancellation: &ConversionCancellation,
) -> Result<Vec<Vec<u16>>, String> {
    match recipe.engine_mode {
        ConversionEngineMode::Icc => {
            let transform = ProductionCmykTransform::new(
                model,
                RuntimeIccProfile::Embedded(source_icc),
                RuntimeIccProfile::Embedded(target_icc),
                recipe.rendering_intent,
                recipe.black_point_compensation,
            )?;
            dispatch::<4, _, _>(
                model,
                planes,
                cancellation,
                |source, destination| transform.transform_rgb_chunk(source, destination),
                |source, destination| transform.transform_cmyk_chunk(source, destination),
            )
        }
        ConversionEngineMode::DeviceLink => {
            let transform = ProductionDeviceLinkTransform::<4>::new(
                model,
                RuntimeIccProfile::Embedded(target_icc),
            )?;
            dispatch::<4, _, _>(
                model,
                planes,
                cancellation,
                |source, destination| transform.transform_rgb_chunk(source, destination),
                |source, destination| transform.transform_cmyk_chunk(source, destination),
            )
        }
        ConversionEngineMode::CustomOptimizer => Err(
            "Custom Optimizer candidate preview requires its characterized-target runtime."
                .to_owned(),
        ),
    }
}

fn transform_n<const N: usize>(
    model: IccSourceModel,
    planes: &[Vec<u16>],
    source_icc: &[u8],
    target_icc: &[u8],
    recipe: &ConversionRecipe,
    cancellation: &ConversionCancellation,
) -> Result<Vec<Vec<u16>>, String> {
    match recipe.engine_mode {
        ConversionEngineMode::Icc => {
            let transform = ProductionNChannelTransform::<N>::new(
                model,
                RuntimeIccProfile::Embedded(source_icc),
                RuntimeIccProfile::Embedded(target_icc),
                recipe.rendering_intent,
                recipe.black_point_compensation,
            )?;
            dispatch::<N, _, _>(
                model,
                planes,
                cancellation,
                |source, destination| transform.transform_rgb_chunk(source, destination),
                |source, destination| transform.transform_cmyk_chunk(source, destination),
            )
        }
        ConversionEngineMode::DeviceLink => {
            let transform = ProductionDeviceLinkTransform::<N>::new(
                model,
                RuntimeIccProfile::Embedded(target_icc),
            )?;
            dispatch::<N, _, _>(
                model,
                planes,
                cancellation,
                |source, destination| transform.transform_rgb_chunk(source, destination),
                |source, destination| transform.transform_cmyk_chunk(source, destination),
            )
        }
        ConversionEngineMode::CustomOptimizer => Err(
            "Custom Optimizer candidate preview requires its characterized-target runtime."
                .to_owned(),
        ),
    }
}

fn dispatch<const N: usize, R, C>(
    model: IccSourceModel,
    planes: &[Vec<u16>],
    cancellation: &ConversionCancellation,
    rgb_transform: R,
    cmyk_transform: C,
) -> Result<Vec<Vec<u16>>, String>
where
    R: FnMut(&[[u16; 3]], &mut [[u16; N]]) -> Result<(), String>,
    C: FnMut(&[[u16; 4]], &mut [[u16; N]]) -> Result<(), String>,
{
    match model {
        IccSourceModel::Rgb => transform_rgb::<N, R>(planes, cancellation, rgb_transform),
        IccSourceModel::Cmyk => transform_cmyk::<N, C>(planes, cancellation, cmyk_transform),
    }
}

fn transform_rgb<const N: usize, F>(
    planes: &[Vec<u16>],
    cancellation: &ConversionCancellation,
    mut transform: F,
) -> Result<Vec<Vec<u16>>, String>
where
    F: FnMut(&[[u16; 3]], &mut [[u16; N]]) -> Result<(), String>,
{
    let pixels = planes[0].len();
    let source = (0..pixels)
        .map(|pixel| [planes[0][pixel], planes[1][pixel], planes[2][pixel]])
        .collect::<Vec<_>>();
    let mut destination = vec![[0u16; N]; pixels];
    for start in (0..pixels).step_by(PREVIEW_CHUNK_PIXELS) {
        check_cancelled(cancellation)?;
        let end = (start + PREVIEW_CHUNK_PIXELS).min(pixels);
        transform(&source[start..end], &mut destination[start..end])?;
    }
    Ok(deinterleave(&destination))
}

fn transform_cmyk<const N: usize, F>(
    planes: &[Vec<u16>],
    cancellation: &ConversionCancellation,
    mut transform: F,
) -> Result<Vec<Vec<u16>>, String>
where
    F: FnMut(&[[u16; 4]], &mut [[u16; N]]) -> Result<(), String>,
{
    let pixels = planes[0].len();
    let source = (0..pixels)
        .map(|pixel| [
            planes[0][pixel],
            planes[1][pixel],
            planes[2][pixel],
            planes[3][pixel],
        ])
        .collect::<Vec<_>>();
    let mut destination = vec![[0u16; N]; pixels];
    for start in (0..pixels).step_by(PREVIEW_CHUNK_PIXELS) {
        check_cancelled(cancellation)?;
        let end = (start + PREVIEW_CHUNK_PIXELS).min(pixels);
        transform(&source[start..end], &mut destination[start..end])?;
    }
    Ok(deinterleave(&destination))
}

fn deinterleave<const N: usize>(pixels: &[[u16; N]]) -> Vec<Vec<u16>> {
    let mut planes = (0..N)
        .map(|_| Vec::with_capacity(pixels.len()))
        .collect::<Vec<_>>();
    for pixel in pixels {
        for channel in 0..N {
            planes[channel].push(pixel[channel]);
        }
    }
    planes
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
                "Cannot reopen assigned Source ICC {} for candidate preview: {error}",
                path.display()
            )
        })?,
    };
    verify_sha256(&bytes, expected_sha256, "Source ICC")?;
    Ok(bytes)
}

fn load_target_icc(recipe: &ConversionRecipe) -> Result<Vec<u8>, String> {
    let (path, hash, label) = match recipe.engine_mode {
        ConversionEngineMode::Icc => (
            recipe.target.output_profile_path.as_deref(),
            recipe
                .target
                .output_profile_identity
                .as_ref()
                .map(|identity| identity.sha256.as_str()),
            "Target ICC",
        ),
        ConversionEngineMode::DeviceLink => (
            recipe.target.device_link_path.as_deref(),
            recipe
                .target
                .device_link_identity
                .as_ref()
                .map(|identity| identity.sha256.as_str()),
            "DeviceLink ICC",
        ),
        ConversionEngineMode::CustomOptimizer => {
            return Err("Custom Optimizer has no ICC/DeviceLink candidate payload.".to_owned());
        }
    };
    let path = path.ok_or_else(|| format!("Candidate recipe has no {label} path."))?;
    let hash = hash.ok_or_else(|| format!("Candidate recipe has no {label} identity."))?;
    let bytes = fs::read(Path::new(path))
        .map_err(|error| format!("Cannot reopen {label} {path}: {error}"))?;
    verify_sha256(&bytes, hash, label)?;
    Ok(bytes)
}

fn verify_sha256(bytes: &[u8], expected: &str, label: &str) -> Result<(), String> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "{label} identity changed before candidate preview (expected {}, found {actual}).",
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
    use lcms2::{ColorSpaceSignature, Profile};

    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy,
    };
    use crate::model::IccProfileIdentity;

    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn unique_temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-candidate-{name}-{}-{}.icc",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn candidate_uses_production_devicelink_samples_and_builds_target_histograms() {
        let link_bytes = Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0)
            .unwrap()
            .icc()
            .unwrap();
        let link_path = unique_temp_file("link");
        fs::write(&link_path, &link_bytes).unwrap();
        let source_icc = Profile::new_srgb().icc().unwrap();
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::DeviceLink,
            source_profile_identity: IccProfileIdentity {
                description: "Source identity".to_owned(),
                sha256: hash(&source_icc),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "CMYK Link".to_owned(),
                channels: ["Cyan", "Magenta", "Yellow", "Black"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: (index == 0).then_some(0.5),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: Some(IccProfileIdentity {
                    description: "Link".to_owned(),
                    sha256: hash(&link_bytes),
                }),
                device_link_path: Some(link_path.to_string_lossy().into_owned()),
                characterization_id: None,
                total_ink_limit: Some(2.4),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        };
        let input = CandidatePreviewInput {
            width: 2,
            height: 1,
            source_model: IccSourceModel::Cmyk,
            source_planes: vec![
                vec![0, 52_000],
                vec![0, 41_000],
                vec![0, 30_000],
                vec![0, 22_000],
            ],
            source_profile: CapturedSourceProfile::Embedded,
            embedded_source_icc: Some(source_icc),
            recipe,
        };
        let result = render_candidate_preview(input, &ConversionCancellation::default()).unwrap();
        assert_eq!(result.channel_count(), 4);
        assert_eq!(result.planes[0][0], 0);
        assert_eq!(result.histograms[0].iter().sum::<u32>(), 2);
        assert_eq!(result.recipe_sha256.len(), 64);
        assert_eq!(result.usage.pixel_count, 2);
        assert_eq!(result.usage.channels.len(), 4);
        assert!(result.usage.channels[0].limit_hit_percent.is_some());
        assert!(result.usage.total_ink_limit_hit_percent.is_some());
        assert!(result.usage.neutral_black_share.is_none());
        let _ = fs::remove_file(link_path);
    }

    #[test]
    fn cancellation_invalidates_stale_candidate_before_profile_io() {
        let cancellation = ConversionCancellation::default();
        cancellation.request();
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "0".repeat(64),
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
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Target".to_owned(),
                    sha256: "1".repeat(64),
                }),
                output_profile_path: Some("missing.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: ConversionRenderingIntent::Perceptual,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        };
        let error = render_candidate_preview(
            CandidatePreviewInput {
                width: 1,
                height: 1,
                source_model: IccSourceModel::Rgb,
                source_planes: vec![vec![0], vec![0], vec![0]],
                source_profile: CapturedSourceProfile::Embedded,
                embedded_source_icc: None,
                recipe,
            },
            &cancellation,
        )
        .unwrap_err();
        assert!(error.contains("cancelled"));
    }
}
