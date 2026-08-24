use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::color_conversion::{
    ConversionEngineMode, ConversionRecipe, TargetChannelDefinition,
};
use crate::conversion_recipe::recipe_sha256;
use crate::conversion_transaction::{CapturedSourceProfile, ConversionCancellation};
use crate::devicelink_conversion::ProductionDeviceLinkTransform;
use crate::icc_conversion::{IccSourceModel, ProductionCmykTransform, RuntimeIccProfile};
use crate::nchannel_icc::ProductionNChannelTransform;

const PREVIEW_CHUNK_PIXELS: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct CandidatePreviewInput {
    pub width: usize,
    pub height: usize,
    pub source_model: IccSourceModel,
    /// Downsampled, source-adjusted planes in production working polarity.
    /// RGB inputs require 3 planes; CMYK inputs require 4.
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
}

impl CandidatePreviewResult {
    pub fn channel_names(&self) -> Vec<String> {
        self.channels.iter().map(|channel| channel.name.clone()).collect()
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
}

/// Convert a bounded preview raster with the same production transform primitives
/// used by the final conversion worker. This function performs no output write,
/// project mutation, or Production-project creation.
pub fn render_candidate_preview(
    input: CandidatePreviewInput,
    cancellation: &ConversionCancellation,
) -> Result<CandidatePreviewResult, String> {
    input.recipe.validate().map_err(|errors| {
        format!("Candidate preview recipe is invalid: {}", errors.join(" "))
    })?;
    cancellation_check(cancellation)?;

    let expected_source_channels = match input.source_model {
        IccSourceModel::Rgb => 3,
        IccSourceModel::Cmyk => 4,
    };
    if input.source_planes.len() != expected_source_channels {
        return Err(format!(
            "Candidate preview {:?} source requires {expected_source_channels} adjusted planes; found {}.",
            input.source_model,
            input.source_planes.len()
        ));
    }
    let pixel_count = input
        .width
        .checked_mul(input.height)
        .ok_or_else(|| "Candidate preview dimensions overflow the pixel count.".to_owned())?;
    if pixel_count == 0 {
        return Err("Candidate preview requires a non-empty raster.".to_owned());
    }
    for (index, plane) in input.source_planes.iter().enumerate() {
        if plane.len() != pixel_count {
            return Err(format!(
                "Candidate preview source plane {} has {} samples; expected {pixel_count}.",
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
    cancellation_check(cancellation)?;

    let channel_count = input.recipe.target.channels.len();
    let planes = match channel_count {
        4 => transform_4(
            input.source_model,
            &input.source_planes,
            &source_icc,
            &target_icc,
            &input.recipe,
            cancellation,
        )?,
        5 => transform_n::<5>(input.source_model, &input.source_planes, &source_icc, &target_icc, &input.recipe, cancellation)?,
        6 => transform_n::<6>(input.source_model, &input.source_planes, &source_icc, &target_icc, &input.recipe, cancellation)?,
        7 => transform_n::<7>(input.source_model, &input.source_planes, &source_icc, &target_icc, &input.recipe, cancellation)?,
        8 => transform_n::<8>(input.source_model, &input.source_planes, &source_icc, &target_icc, &input.recipe, cancellation)?,
        9 => transform_n::<9>(input.source_model, &input.source_planes, &source_icc, &target_icc, &input.recipe, cancellation)?,
        10 => transform_n::<10>(input.source_model, &input.source_planes, &source_icc, &target_icc, &input.recipe, cancellation)?,
        11 => transform_n::<11>(input.source_model, &input.source_planes, &source_icc, &target_icc, &input.recipe, cancellation)?,
        12 => transform_n::<12>(input.source_model, &input.source_planes, &source_icc, &target_icc, &input.recipe, cancellation)?,
        other => {
            return Err(format!(
                "Candidate preview supports production target topology 4..=12 channels; found {other}."
            ));
        }
    };
    let histograms = histograms(&planes);
    Ok(CandidatePreviewResult {
        width: input.width,
        height: input.height,
        recipe_sha256: recipe_sha256(&input.recipe)?,
        channels: input.recipe.target.channels,
        planes,
        histograms,
    })
}

fn transform_4(
    source_model: IccSourceModel,
    source_planes: &[Vec<u16>],
    source_icc: &[u8],
    target_icc: &[u8],
    recipe: &ConversionRecipe,
    cancellation: &ConversionCancellation,
) -> Result<Vec<Vec<u16>>, String> {
    match recipe.engine_mode {
        ConversionEngineMode::Icc => {
            let transform = ProductionCmykTransform::new(
                source_model,
                RuntimeIccProfile::Embedded(source_icc),
                RuntimeIccProfile::Embedded(target_icc),
                recipe.rendering_intent,
                recipe.black_point_compensation,
            )?;
            match source_model {
                IccSourceModel::Rgb => {
                    transform_rgb_to::<4, _>(source_planes, cancellation, |src, dst| {
                        transform.transform_rgb_chunk(src, dst)
                    })
                }
                IccSourceModel::Cmyk => {
                    transform_cmyk_to::<4, _>(source_planes, cancellation, |src, dst| {
                        transform.transform_cmyk_chunk(src, dst)
                    })
                }
            }
        }
        ConversionEngineMode::DeviceLink => {
            let transform = ProductionDeviceLinkTransform::<4>::new(
                source_model,
                RuntimeIccProfile::Embedded(target_icc),
            )?;
            match source_model {
                IccSourceModel::Rgb => {
                    transform_rgb_to::<4, _>(source_planes, cancellation, |src, dst| {
                        transform.transform_rgb_chunk(src, dst)
                    })
                }
                IccSourceModel::Cmyk => {
                    transform_cmyk_to::<4, _>(source_planes, cancellation, |src, dst| {
                        transform.transform_cmyk_chunk(src, dst)
                    })
                }
            }
        }
        ConversionEngineMode::CustomOptimizer => Err(
            "Candidate preview for Custom Optimizer requires its authorized characterized-target runtime and is not available through the ICC/DeviceLink preview path."
                .to_owned(),
        ),
    }
}

fn transform_n<const N: usize>(
    source_model: IccSourceModel,
    source_planes: &[Vec<u16>],
    source_icc: &[u8],
    target_icc: &[u8],
    recipe: &ConversionRecipe,
    cancellation: &ConversionCancellation,
) -> Result<Vec<Vec<u16>>, String> {
    match recipe.engine_mode {
        ConversionEngineMode::Icc => {
            let transform = ProductionNChannelTransform::<N>::new(
                source_model,
                RuntimeIccProfile::Embedded(source_icc),
                RuntimeIccProfile::Embedded(target_icc),
                recipe.rendering_intent,
                recipe.black_point_compensation,
            )?;
            match source_model {
                IccSourceModel::Rgb => transform_rgb_to::<N, _>(
                    source_planes,
                    cancellation,
                    |src, dst| transform.transform_rgb_chunk(src, dst),
                ),
                IccSourceModel::Cmyk => transform_cmyk_to::<N, _>(
                    source_planes,
                    cancellation,
                    |src, dst| transform.transform_cmyk_chunk(src, dst),
                ),
            }
        }
        ConversionEngineMode::DeviceLink => {
            let transform = ProductionDeviceLinkTransform::<N>::new(
                source_model,
                RuntimeIccProfile::Embedded(target_icc),
            )?;
            match source_model {
                IccSourceModel::Rgb => transform_rgb_to::<N, _>(
                    source_planes,
                    cancellation,
                    |src, dst| transform.transform_rgb_chunk(src, dst),
                ),
                IccSourceModel::Cmyk => transform_cmyk_to::<N, _>(
                    source_planes,
                    cancellation,
                    |src, dst| transform.transform_cmyk_chunk(src, dst),
                ),
            }
        }
        ConversionEngineMode::CustomOptimizer => Err(
            "Candidate preview for Custom Optimizer requires its authorized characterized-target runtime and is not available through the ICC/DeviceLink preview path."
                .to_owned(),
        ),
    }
}

fn transform_rgb_to<const N: usize, F>(
    planes: &[Vec<u16>],
    cancellation: &ConversionCancellation,
    mut transform: F,
) -> Result<Vec<Vec<u16>>, String>
where
    F: FnMut(&[[u16; 3]], &mut [[u16; N]]) -> Result<(), String>,
{
    let pixel_count = planes[0].len();
    let mut source = Vec::with_capacity(pixel_count);
    for pixel in 0..pixel_count {
        source.push([planes[0][pixel], planes[1][pixel], planes[2][pixel]]);
    }
    let mut destination = vec![[0u16; N]; pixel_count];
    for start in (0..pixel_count).step_by(PREVIEW_CHUNK_PIXELS) {
        cancellation_check(cancellation)?;
        let end = (start + PREVIEW_CHUNK_PIXELS).min(pixel_count);
        transform(&source[start..end], &mut destination[start..end])?;
    }
    Ok(deinterleave::<N>(&destination))
}

fn transform_cmyk_to<const N: usize, F>(
    planes: &[Vec<u16>],
    cancellation: &ConversionCancellation,
    mut transform: F,
) -> Result<Vec<Vec<u16>>, String>
where
    F: FnMut(&[[u16; 4]], &mut [[u16; N]]) -> Result<(), String>,
{
    let pixel_count = planes[0].len();
    let mut source = Vec::with_capacity(pixel_count);
    for pixel in 0..pixel_count {
        source.push([
            planes[0][pixel],
            planes[1][pixel],
            planes[2][pixel],
            planes[3][pixel],
        ]);
    }
    let mut destination = vec![[0u16; N]; pixel_count];
    for start in (0..pixel_count).step_by(PREVIEW_CHUNK_PIXELS) {
        cancellation_check(cancellation)?;
        let end = (start + PREVIEW_CHUNK_PIXELS).min(pixel_count);
        transform(&source[start..end], &mut destination[start..end])?;
    }
    Ok(deinterleave::<N>(&destination))
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

fn histograms(planes: &[Vec<u16>]) -> Vec<[u32; 256]> {
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
        CapturedSourceProfile::Embedded => embedded
            .map(ToOwned::to_owned)
            .ok_or_else(|| "Candidate preview expects an embedded Source ICC, but the preview source has none.".to_owned())?,
        CapturedSourceProfile::External { path } => fs::read(path).map_err(|error| {
            format!("Cannot reopen assigned Source ICC {} for candidate preview: {error}", path.display())
        })?,
    };
    verify_sha256(&bytes, expected_sha256, "Source ICC")?;
    Ok(bytes)
}

fn load_target_icc(recipe: &ConversionRecipe) -> Result<Vec<u8>, String> {
    let (path, expected_sha256, label) = match recipe.engine_mode {
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
            return Err(
                "Custom Optimizer candidate preview does not use an ICC/DeviceLink payload."
                    .to_owned(),
            );
        }
    };
    let path = path.ok_or_else(|| format!("Candidate preview recipe has no {label} path."))?;
    let expected_sha256 =
        expected_sha256.ok_or_else(|| format!("Candidate preview recipe has no {label} identity."))?;
    let bytes = fs::read(Path::new(path))
        .map_err(|error| format!("Cannot reopen {label} {path}: {error}"))?;
    verify_sha256(&bytes, expected_sha256, label)?;
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

fn cancellation_check(cancellation: &ConversionCancellation) -> Result<(), String> {
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
    use tempfile::tempdir;

    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy,
    };
    use crate::model::IccProfileIdentity;

    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn cmyk_link_recipe(path: &Path, link_bytes: &[u8], source_icc: &[u8]) -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::DeviceLink,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash(source_icc),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "CMYK Link".to_owned(),
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
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: Some(IccProfileIdentity {
                    description: "Link".to_owned(),
                    sha256: hash(link_bytes),
                }),
                device_link_path: Some(path.to_string_lossy().into_owned()),
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        }
    }

    #[test]
    fn candidate_preview_uses_direct_production_devicelink_samples_and_histograms() {
        let dir = tempdir().unwrap();
        let link_bytes = Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0)
            .unwrap()
            .icc()
            .unwrap();
        let link_path = dir.path().join("link.icc");
        fs::write(&link_path, &link_bytes).unwrap();
        let source_icc = Profile::new_srgb().icc().unwrap();
        let recipe = cmyk_link_recipe(&link_path, &link_bytes, &source_icc);
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
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 1);
        assert_eq!(result.planes[0][0], 0);
        assert_eq!(result.histograms[0].iter().sum::<u32>(), 2);
        assert_eq!(result.recipe_sha256.len(), 64);
    }

    #[test]
    fn cancellation_blocks_stale_preview_before_transform() {
        let cancellation = ConversionCancellation::default();
        cancellation.request();
        let input = CandidatePreviewInput {
            width: 1,
            height: 1,
            source_model: IccSourceModel::Rgb,
            source_planes: vec![vec![0], vec![0], vec![0]],
            source_profile: CapturedSourceProfile::Embedded,
            embedded_source_icc: None,
            recipe: ConversionRecipe {
                schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
                engine_mode: ConversionEngineMode::Icc,
                source_profile_identity: IccProfileIdentity {
                    description: "Source".to_owned(),
                    sha256: "0".repeat(64),
                },
                source_transparency_policy: None,
                target: ConversionTargetDefinition {
                    name: "CMYK".to_owned(),
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
            },
        };
        assert!(render_candidate_preview(input, &cancellation)
            .unwrap_err()
            .contains("cancelled"));
    }
}
