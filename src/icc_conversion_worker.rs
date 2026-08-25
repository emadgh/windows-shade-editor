use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use memmap2::MmapOptions;
use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_tiff::{
    ConversionTiffSpec, write_conversion_tiff_u8_atomic, write_conversion_tiff_u16_atomic,
};
use crate::conversion_transaction::{
    CapturedOutputPolicy, CapturedSourceProfile, CommittedConversionOutput, ConversionCancellation,
    ConversionJobCapture, ConversionPhase, ConversionProgress, ConversionTransactionBackend,
};
use crate::custom_optimizer_evidence::load_and_authorize_custom_optimizer_evidence;
use crate::custom_optimizer_raster_transform::{
    MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS, ProductionCustomOptimizerRasterTransform,
};
use crate::devicelink_conversion::ProductionDeviceLinkTransform;
use crate::icc_conversion::{IccSourceModel, ProductionCmykTransform, RuntimeIccProfile};
use crate::jpeg_source::{DecodedJpegSource, JpegSourceModel, decode_jpeg_source};
use crate::model::{MASTER_ADJUSTMENT_KEY, ShadeProject, apply_curve, apply_levels};
use crate::nchannel_icc::ProductionNChannelTransform;
use crate::png_source::{DecodedPngSource, PngSourceModel, decode_png_source};
use crate::source_profile_fallback::{is_srgb_fallback_identity, srgb_fallback_icc};
use crate::source_transparency::{SourceTransparencyPolicy, composite_rgb_u16};
use crate::tiff_io::{self, ColorModel, StreamInfo};
use crate::{dpi, export};

static CONVERSION_SPOOL_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const DESIGN_SOURCE_ROWS_PER_STRIP: u32 = 64;

pub struct FilesystemIccConversionBackend {
    default_dpi: f64,
    replace_existing: bool,
}

impl FilesystemIccConversionBackend {
    pub fn new(default_dpi: f64) -> Result<Self, String> {
        if !default_dpi.is_finite() || default_dpi <= 0.0 {
            return Err("Conversion fallback DPI must be finite and positive.".to_owned());
        }
        Ok(Self {
            default_dpi,
            replace_existing: false,
        })
    }
}

impl ConversionTransactionBackend for FilesystemIccConversionBackend {
    fn render_convert_and_commit(
        &mut self,
        capture: &ConversionJobCapture,
        cancellation: &ConversionCancellation,
        report: &mut dyn FnMut(ConversionProgress),
    ) -> Result<CommittedConversionOutput, String> {
        cancellation.check_before_commit()?;
        self.replace_existing = capture.output_policy == CapturedOutputPolicy::TransactionalReplace;
        if !self.replace_existing && capture.output_tiff_path.exists() {
            return Err(
                "Queued conversion TIFF destination is no longer free; review route ownership and queue again."
                    .to_owned(),
            );
        }
        report(ConversionProgress::new(
            ConversionPhase::CaptureValidation,
            0.02,
            "Revalidating source file identity",
        ));
        verify_file_sha256(
            &capture.source_face_path,
            &capture.source_file_sha256,
            "Source Face",
        )?;

        report(ConversionProgress::new(
            ConversionPhase::Decode,
            0.04,
            "Inspecting production source topology",
        ));
        let source = ProductionSourceRaster::load(&capture.source_face_path)?;
        let source_model = source.source_model()?;
        source.validate_transparency_policy(&capture.conversion_recipe)?;

        let (output_icc, mut transform) = match capture.conversion_recipe.engine_mode {
            ConversionEngineMode::CustomOptimizer => {
                report(ConversionProgress::new(
                    ConversionPhase::CaptureValidation,
                    0.05,
                    "Reopening and authorizing Custom Optimizer production evidence",
                ));
                let source_icc = load_verified_source_icc(capture, source.embedded_icc())?;
                let evidence = capture.custom_optimizer_evidence.as_ref().ok_or_else(|| {
                    "Custom Optimizer conversion capture is missing immutable production evidence."
                        .to_owned()
                })?;
                let loaded = load_and_authorize_custom_optimizer_evidence(
                    evidence,
                    &capture.conversion_recipe,
                )
                .map_err(|error| {
                    format!("Custom Optimizer production evidence rejected: {error:?}")
                })?;
                let transform = ProductionCustomOptimizerRasterTransform::authorize(
                    source_model,
                    &source_icc,
                    &loaded.lut,
                    &loaded.validation,
                    &evidence.threshold_set,
                    &evidence.calibration_manifest,
                    &evidence.calibration_approval,
                    &loaded.pcs_compatibility,
                    &capture.conversion_recipe,
                    &loaded.model,
                )
                .map_err(|error| {
                    format!("Cannot authorize Custom Optimizer raster transform: {error:?}")
                })?;
                if transform.eligibility() != &loaded.eligibility {
                    return Err(
                        "Custom Optimizer raster authorization identity changed after evidence reload."
                            .to_owned(),
                    );
                }
                (None, RuntimeProductionTransform::Custom(transform))
            }
            ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink => {
                let VerifiedConversionProfiles {
                    source_icc,
                    transform_icc,
                    embed_output_icc,
                } = load_verified_profiles(capture, source.embedded_icc())?;
                let transform = RuntimeProductionTransform::new(
                    source_model,
                    &source_icc,
                    &transform_icc,
                    &capture.conversion_recipe,
                )?;
                let output_icc = embed_output_icc.then_some(transform_icc);
                (output_icc, transform)
            }
        };
        if transform.output_channels() != capture.conversion_recipe.target.channels.len() {
            return Err(
                "Runtime production transform topology does not match the captured target."
                    .to_owned(),
            );
        }

        render_convert_and_commit(
            capture,
            cancellation,
            report,
            &source,
            output_icc.as_deref(),
            &mut transform,
            self.default_dpi,
        )?;

        Ok(CommittedConversionOutput {
            path: capture.output_tiff_path.clone(),
            sha256: sha256_file(&capture.output_tiff_path)?,
            converted_at_unix_ms: unix_time_ms()?,
        })
    }

    fn save_production_project(
        &mut self,
        path: &Path,
        project: &ShadeProject,
    ) -> Result<(), String> {
        if project.faces.len() != 1 {
            return Err(
                "A new Production project must contain exactly one converted Face.".to_owned(),
            );
        }
        let output = PathBuf::from(&project.faces[0].path);
        if self.replace_existing {
            project.save(path, &[output])
        } else {
            project.save_new(path, &[output])
        }
    }
}

enum ProductionSourceRaster {
    Tiff(StreamInfo),
    Png(DecodedPngSource),
    Jpeg(DecodedJpegSource),
}

impl ProductionSourceRaster {
    fn load(path: &Path) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "tif" | "tiff" => {
                let stream = tiff_io::stream_info(path)?;
                if !stream.streamable {
                    return Err(
                        "Production conversion requires a strip/tile-streamable TIFF source; full-image TIFF fallback is disabled to preserve bounded memory."
                            .to_owned(),
                    );
                }
                Ok(Self::Tiff(stream))
            }
            "png" => decode_png_source(path).map(Self::Png),
            "jpg" | "jpeg" => decode_jpeg_source(path).map(Self::Jpeg),
            _ => Err(format!(
                "Unsupported production source format for {}. Use RGB TIFF, PNG or JPEG, or CMYK TIFF.",
                path.display()
            )),
        }
    }

    fn source_model(&self) -> Result<IccSourceModel, String> {
        match self {
            Self::Tiff(stream) => tiff_source_model(stream),
            Self::Png(source) => match source.model {
                PngSourceModel::Rgb => Ok(IccSourceModel::Rgb),
                PngSourceModel::Gray => Err(
                    "Production color conversion currently requires RGB PNG artwork; Gray PNG is not enabled for the production transform path."
                        .to_owned(),
                ),
            },
            Self::Jpeg(source) => match source.model {
                JpegSourceModel::Rgb => Ok(IccSourceModel::Rgb),
                JpegSourceModel::Gray => Err(
                    "Production color conversion currently requires RGB JPEG artwork; Gray JPEG is not enabled for the production transform path."
                        .to_owned(),
                ),
            },
        }
    }

    fn embedded_icc(&self) -> Option<&[u8]> {
        match self {
            Self::Tiff(stream) => stream.metadata.icc_profile.as_deref(),
            Self::Png(source) => source.icc_profile.as_deref(),
            Self::Jpeg(source) => source.icc_profile.as_deref(),
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Tiff(stream) => (stream.metadata.width, stream.metadata.height),
            Self::Png(source) => (source.width, source.height),
            Self::Jpeg(source) => (source.width, source.height),
        }
    }

    fn source_channels(&self) -> usize {
        match self {
            Self::Tiff(stream) => stream.metadata.samples_per_pixel,
            Self::Png(source) => match source.model {
                PngSourceModel::Gray => 1,
                PngSourceModel::Rgb => 3,
            },
            Self::Jpeg(source) => match source.model {
                JpegSourceModel::Gray => 1,
                JpegSourceModel::Rgb => 3,
            },
        }
    }

    fn rows_per_strip(&self) -> u32 {
        match self {
            Self::Tiff(stream) => stream.rows_per_strip.max(1),
            Self::Png(source) => source.height.min(DESIGN_SOURCE_ROWS_PER_STRIP).max(1),
            Self::Jpeg(source) => source.height.min(DESIGN_SOURCE_ROWS_PER_STRIP).max(1),
        }
    }

    fn orientation(&self) -> Option<u16> {
        match self {
            Self::Tiff(stream) => stream.metadata.orientation,
            Self::Png(_) | Self::Jpeg(_) => None,
        }
    }

    fn dpi(&self, path: &Path, default_dpi: f64) -> dpi::DpiInfo {
        match self {
            Self::Tiff(_) => dpi::read_dpi(path, default_dpi),
            Self::Png(_) | Self::Jpeg(_) => dpi::DpiInfo::with_default(default_dpi),
        }
    }

    fn validate_transparency_policy(&self, recipe: &ConversionRecipe) -> Result<(), String> {
        let policy = recipe.source_transparency_policy;
        match self {
            Self::Png(source) if source.alpha.is_some() => {
                if policy.is_none() {
                    Err(
                        "Captured PNG contains alpha but the conversion recipe has no explicit flatten background policy."
                            .to_owned(),
                    )
                } else {
                    Ok(())
                }
            }
            Self::Png(_) => {
                if policy.is_some() {
                    Err(
                        "Captured conversion recipe contains an alpha-flatten policy, but this PNG has no alpha plane."
                            .to_owned(),
                    )
                } else {
                    Ok(())
                }
            }
            Self::Jpeg(_) => {
                if policy.is_some() {
                    Err(
                        "Captured conversion recipe contains an alpha-flatten policy, but JPEG has no alpha plane."
                            .to_owned(),
                    )
                } else {
                    Ok(())
                }
            }
            Self::Tiff(_) => {
                if policy.is_some() {
                    Err(
                        "Captured conversion recipe contains a PNG alpha-flatten policy for a TIFF source."
                            .to_owned(),
                    )
                } else {
                    Ok(())
                }
            }
        }
    }
}

fn tiff_source_model(stream: &StreamInfo) -> Result<IccSourceModel, String> {
    let metadata = &stream.metadata;
    if metadata.samples_per_pixel != metadata.base_channel_count {
        return Err(
            "ICC production conversion currently requires a pure RGB or CMYK source without extra/Spot samples."
                .to_owned(),
        );
    }
    match (metadata.color_model, metadata.samples_per_pixel) {
        (ColorModel::Rgb, 3) => Ok(IccSourceModel::Rgb),
        (ColorModel::Cmyk, 4) => Ok(IccSourceModel::Cmyk),
        _ => Err(format!(
            "ICC production conversion requires 3-channel RGB or 4-channel CMYK source data; found {} with {} samples.",
            metadata.color_model.title(),
            metadata.samples_per_pixel
        )),
    }
}

struct VerifiedConversionProfiles {
    source_icc: Vec<u8>,
    transform_icc: Vec<u8>,
    embed_output_icc: bool,
}

fn load_verified_source_icc(
    capture: &ConversionJobCapture,
    embedded_icc: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let source_icc = match &capture.source_profile {
        CapturedSourceProfile::Embedded => match embedded_icc {
            Some(bytes) => bytes.to_vec(),
            None if is_srgb_fallback_identity(&capture.conversion_recipe.source_profile_identity) => {
                srgb_fallback_icc()?
            }
            None => {
                return Err(
                    "Captured source expects an embedded ICC, but the decoded source has none."
                        .to_owned(),
                );
            }
        },
        CapturedSourceProfile::External { path } => fs::read(path).map_err(|err| {
            format!(
                "Cannot reopen assigned production Source ICC {}: {err}",
                path.display()
            )
        })?,
    };
    verify_bytes_sha256(
        &source_icc,
        &capture.conversion_recipe.source_profile_identity.sha256,
        "Source ICC",
    )?;
    Ok(source_icc)
}

fn load_verified_profiles(
    capture: &ConversionJobCapture,
    embedded_icc: Option<&[u8]>,
) -> Result<VerifiedConversionProfiles, String> {
    let source_icc = load_verified_source_icc(capture, embedded_icc)?;

    let (target_path, target_identity, label, embed_as_output) =
        match capture.conversion_recipe.engine_mode {
            ConversionEngineMode::Icc => (
                capture
                    .conversion_recipe
                    .target
                    .output_profile_path
                    .as_deref(),
                capture
                    .conversion_recipe
                    .target
                    .output_profile_identity
                    .as_ref(),
                "Target ICC",
                true,
            ),
            ConversionEngineMode::DeviceLink => (
                capture.conversion_recipe.target.device_link_path.as_deref(),
                capture
                    .conversion_recipe
                    .target
                    .device_link_identity
                    .as_ref(),
                "DeviceLink ICC",
                false,
            ),
            ConversionEngineMode::CustomOptimizer => {
                return Err("Custom Optimizer requires its dedicated production engine.".to_owned());
            }
        };
    let target_path = target_path
        .map(Path::new)
        .ok_or_else(|| format!("Captured recipe has no {label} path."))?;
    let target_identity =
        target_identity.ok_or_else(|| format!("Captured recipe has no {label} identity."))?;
    let transform_icc = fs::read(target_path)
        .map_err(|err| format!("Cannot reopen {label} {}: {err}", target_path.display()))?;
    verify_bytes_sha256(&transform_icc, &target_identity.sha256, label)?;
    Ok(VerifiedConversionProfiles {
        source_icc,
        transform_icc,
        embed_output_icc: embed_as_output,
    })
}

fn render_convert_and_commit(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
    source: &ProductionSourceRaster,
    target_icc: Option<&[u8]>,
    transform: &mut RuntimeProductionTransform,
    default_dpi: f64,
) -> Result<(), String> {
    let spool_path = conversion_spool_path()?;
    let result = (|| {
        render_adjusted_source_spool(capture, cancellation, report, source, &spool_path)?;
        cancellation.check_before_commit()?;
        report(ConversionProgress::new(
            ConversionPhase::ColorConversion,
            0.52,
            "Opening bounded adjusted-source spool",
        ));
        let spool_file = File::open(&spool_path)
            .map_err(|err| format!("Cannot reopen conversion source spool: {err}"))?;
        // SAFETY: the source spool is complete and no longer mutable while this
        // read-only mapping is alive.
        let mmap = unsafe {
            MmapOptions::new()
                .map(&spool_file)
                .map_err(|err| format!("Cannot map conversion source spool: {err}"))?
        };
        let source_samples = mmap_as_u16(&mmap)?;
        let (source_width, source_height) = source.dimensions();
        let dpi = source.dpi(&capture.source_face_path, default_dpi);
        let channel_names = capture
            .conversion_recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        let spec = ConversionTiffSpec {
            width: source_width,
            height: source_height,
            channel_names: &channel_names,
            target_icc,
            dpi_x: dpi.dpi_x,
            dpi_y: dpi.dpi_y,
            orientation: source.orientation(),
            rows_per_strip: source.rows_per_strip(),
            force_bigtiff: false,
            replace_existing: capture.output_policy == CapturedOutputPolicy::TransactionalReplace,
        };
        let source_channels = source.source_channels();
        let width = source_width as usize;
        let height = source_height.max(1) as f32;

        match capture.conversion_recipe.target.bit_depth {
            16 => write_conversion_tiff_u16_atomic(
                &capture.output_tiff_path,
                &spec,
                |start_row, row_count, output| {
                    cancellation.check_before_commit()?;
                    let input =
                        source_rows(source_samples, start_row, row_count, width, source_channels)?;
                    transform.transform_u16_bounded(input, output, source_channels)?;
                    report(ConversionProgress::new(
                        ConversionPhase::ColorConversion,
                        0.52 + 0.34 * (start_row.saturating_add(row_count) as f32 / height),
                        format!("Converted rows {}–{}", start_row + 1, start_row + row_count),
                    ));
                    Ok(())
                },
            ),
            8 => write_conversion_tiff_u8_atomic(
                &capture.output_tiff_path,
                &spec,
                |start_row, row_count, output| {
                    cancellation.check_before_commit()?;
                    let input =
                        source_rows(source_samples, start_row, row_count, width, source_channels)?;
                    transform.transform_u8_bounded(input, output, source_channels)?;
                    report(ConversionProgress::new(
                        ConversionPhase::ColorConversion,
                        0.52 + 0.34 * (start_row.saturating_add(row_count) as f32 / height),
                        format!("Converted rows {}–{}", start_row + 1, start_row + row_count),
                    ));
                    Ok(())
                },
            ),
            depth => Err(format!(
                "Unsupported captured conversion precision: {depth}-bit."
            )),
        }?;
        report(ConversionProgress::new(
            ConversionPhase::OutputValidation,
            0.90,
            "Conversion TIFF validated and committed",
        ));
        Ok(())
    })();
    let _ = fs::remove_file(&spool_path);
    result
}

fn render_adjusted_source_spool(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
    source: &ProductionSourceRaster,
    spool_path: &Path,
) -> Result<(), String> {
    match source {
        ProductionSourceRaster::Tiff(stream) => {
            render_adjusted_tiff_source_spool(capture, cancellation, report, stream, spool_path)
        }
        ProductionSourceRaster::Png(source) => render_adjusted_rgb_source_spool(
            capture,
            cancellation,
            report,
            source.width,
            source.height,
            &source.samples,
            source.alpha.as_deref(),
            spool_path,
        ),
        ProductionSourceRaster::Jpeg(source) => render_adjusted_rgb_source_spool(
            capture,
            cancellation,
            report,
            source.width,
            source.height,
            &source.samples,
            None,
            spool_path,
        ),
    }
}

fn render_adjusted_rgb_source_spool(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
    width: u32,
    height: u32,
    samples: &[u16],
    alpha: Option<&[u16]>,
    spool_path: &Path,
) -> Result<(), String> {
    const CHANNELS: usize = 3;
    cancellation.check_before_commit()?;
    report(ConversionProgress::new(
        ConversionPhase::SourceAdjustments,
        0.08,
        "Rendering saved RGB source adjustments to local spool",
    ));
    if width == 0 || height == 0 {
        return Err("Decoded RGB design source dimensions must be non-zero.".to_owned());
    }
    let width_usize = width as usize;
    let height_usize = height as usize;
    let pixel_count = width_usize
        .checked_mul(height_usize)
        .ok_or_else(|| "RGB production source pixel count overflow.".to_owned())?;
    let expected_samples = pixel_count
        .checked_mul(CHANNELS)
        .ok_or_else(|| "RGB production source sample count overflow.".to_owned())?;
    if samples.len() != expected_samples {
        return Err(format!(
            "Decoded RGB production source contains {} samples; expected {expected_samples}.",
            samples.len()
        ));
    }
    if let Some(alpha) = alpha {
        if alpha.len() != pixel_count {
            return Err(format!(
                "Decoded PNG alpha contains {} samples; expected {pixel_count}.",
                alpha.len()
            ));
        }
    }
    let policy = match (alpha.is_some(), capture.conversion_recipe.source_transparency_policy) {
        (true, Some(policy)) => Some(policy),
        (true, None) => {
            return Err(
                "PNG alpha requires the explicit flatten policy captured in the conversion recipe."
                    .to_owned(),
            );
        }
        (false, Some(_)) => {
            return Err(
                "Conversion recipe contains an alpha-flatten policy for a source without alpha."
                    .to_owned(),
            );
        }
        (false, None) => None,
    };

    let total_bytes = expected_samples
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| "RGB conversion source spool byte count overflow.".to_owned())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(spool_path)
        .map_err(|err| format!("Cannot create local RGB conversion source spool: {err}"))?;
    file.set_len(
        u64::try_from(total_bytes)
            .map_err(|_| "RGB conversion source spool exceeds supported file size.".to_owned())?,
    )
    .map_err(|err| format!("Cannot size RGB conversion source spool: {err}"))?;
    // SAFETY: this uniquely created file is exclusively owned for the mutable
    // mapping's lifetime and is sized before mapping.
    let mut mmap = unsafe {
        MmapOptions::new()
            .map_mut(&file)
            .map_err(|err| format!("Cannot map RGB conversion source spool: {err}"))?
    };
    let project = capture.source_recipe.materialize_project();
    let chunk_rows = DESIGN_SOURCE_ROWS_PER_STRIP as usize;
    let mut processed_pixels = 0usize;

    for start_row in (0..height_usize).step_by(chunk_rows) {
        cancellation.check_before_commit()?;
        let row_count = chunk_rows.min(height_usize - start_row);
        let start_pixel = start_row
            .checked_mul(width_usize)
            .ok_or_else(|| "RGB production source row offset overflow.".to_owned())?;
        let chunk_pixels = row_count
            .checked_mul(width_usize)
            .ok_or_else(|| "RGB production source chunk pixel count overflow.".to_owned())?;
        let start_sample = start_pixel
            .checked_mul(CHANNELS)
            .ok_or_else(|| "RGB production source sample offset overflow.".to_owned())?;
        let chunk_samples = chunk_pixels
            .checked_mul(CHANNELS)
            .ok_or_else(|| "RGB production source chunk sample count overflow.".to_owned())?;
        let end_sample = start_sample
            .checked_add(chunk_samples)
            .ok_or_else(|| "RGB production source sample end overflow.".to_owned())?;
        let input = samples
            .get(start_sample..end_sample)
            .ok_or_else(|| "Decoded RGB source does not contain the requested rows.".to_owned())?;
        let mut adjusted = adjust_working_rgb(input, &project)?;
        if let (Some(alpha), Some(policy)) = (alpha, policy) {
            let alpha_end = start_pixel
                .checked_add(chunk_pixels)
                .ok_or_else(|| "PNG alpha row range overflow.".to_owned())?;
            flatten_adjusted_rgb_in_place(
                &mut adjusted,
                alpha
                    .get(start_pixel..alpha_end)
                    .ok_or_else(|| "PNG alpha does not contain the requested rows.".to_owned())?,
                policy,
            )?;
        }

        let destination_start = start_sample
            .checked_mul(2)
            .ok_or_else(|| "RGB conversion spool byte offset overflow.".to_owned())?;
        let destination_len = chunk_samples
            .checked_mul(2)
            .ok_or_else(|| "RGB conversion spool byte length overflow.".to_owned())?;
        let destination_end = destination_start
            .checked_add(destination_len)
            .ok_or_else(|| "RGB conversion spool byte end overflow.".to_owned())?;
        let destination = mmap
            .get_mut(destination_start..destination_end)
            .ok_or_else(|| "RGB conversion spool does not contain the requested byte range.".to_owned())?;
        if destination.len() != adjusted.len().saturating_mul(2) {
            return Err("RGB conversion spool sample/byte topology mismatch.".to_owned());
        }
        for (bytes, value) in destination.chunks_exact_mut(2).zip(adjusted.into_iter()) {
            bytes.copy_from_slice(&value.to_ne_bytes());
        }
        processed_pixels = processed_pixels.saturating_add(chunk_pixels);
        report(ConversionProgress::new(
            ConversionPhase::SourceAdjustments,
            0.08 + 0.40 * (processed_pixels as f32 / pixel_count.max(1) as f32),
            format!("Rendered {processed_pixels}/{pixel_count} RGB source pixels"),
        ));
    }

    if processed_pixels != pixel_count {
        return Err(format!(
            "Rendered {processed_pixels} RGB source pixels; expected {pixel_count}."
        ));
    }
    mmap.flush()
        .map_err(|err| format!("Cannot flush RGB conversion source spool: {err}"))?;
    // This is same-process scratch data. The read-only conversion remap only needs completed bytes;
    // crash durability belongs to the later staged/final TIFF boundary, not this disposable spool.
    drop(mmap);
    drop(file);
    Ok(())
}

fn adjust_working_rgb(input: &[u16], project: &ShadeProject) -> Result<Vec<u16>, String> {
    const NAMES: [&str; 3] = ["Red", "Green", "Blue"];
    if input.len() % 3 != 0 {
        return Err(format!(
            "RGB adjustment input contains {} samples, not divisible by 3 channels.",
            input.len()
        ));
    }
    let pixel_count = input.len() / 3;
    let mut output = vec![0u16; input.len()];
    let master = project
        .adjustments
        .get(MASTER_ADJUSTMENT_KEY)
        .filter(|adjustment| adjustment.enabled);
    let mut prepared = [0.0f32; 3];

    for pixel in 0..pixel_count {
        let base = pixel * 3;
        for channel in 0..3 {
            let adjustment = project
                .adjustments
                .get(NAMES[channel])
                .filter(|adjustment| adjustment.enabled);
            let mut value = input[base + channel] as f32 / 65535.0;
            if let Some(adjustment) = adjustment {
                value = apply_levels(value, adjustment.levels);
            }
            if let Some(master) = master {
                value = apply_levels(value, master.levels);
            }
            prepared[channel] = value;
        }
        for out_channel in 0..3 {
            let adjustment = project
                .adjustments
                .get(NAMES[out_channel])
                .filter(|adjustment| adjustment.enabled);
            let mut value = if let Some(adjustment) = adjustment {
                let mut mixed = adjustment.mixer.constant;
                for source_channel in 0..3 {
                    let coefficient = adjustment
                        .mixer
                        .coefficients
                        .get(NAMES[source_channel])
                        .copied()
                        .unwrap_or(if source_channel == out_channel { 1.0 } else { 0.0 });
                    mixed += prepared[source_channel] * coefficient;
                }
                mixed
            } else {
                prepared[out_channel]
            };
            if let Some(adjustment) = adjustment {
                value = apply_curve(value, adjustment.curve);
            }
            if let Some(master) = master {
                value = apply_curve(value, master.curve);
            }
            output[base + out_channel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }
    Ok(output)
}

fn flatten_adjusted_rgb_in_place(
    samples: &mut [u16],
    alpha: &[u16],
    policy: SourceTransparencyPolicy,
) -> Result<(), String> {
    if samples.len() % 3 != 0 {
        return Err("Flatten RGB sample count is not divisible by three.".to_owned());
    }
    let pixel_count = samples.len() / 3;
    if alpha.len() != pixel_count {
        return Err(format!(
            "Flatten alpha contains {} samples; expected {pixel_count}.",
            alpha.len()
        ));
    }
    for (pixel, alpha) in samples.chunks_exact_mut(3).zip(alpha.iter().copied()) {
        let flattened = composite_rgb_u16([pixel[0], pixel[1], pixel[2]], alpha, policy);
        pixel.copy_from_slice(&flattened);
    }
    Ok(())
}

fn render_adjusted_tiff_source_spool(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
    stream: &StreamInfo,
    spool_path: &Path,
) -> Result<(), String> {
    cancellation.check_before_commit()?;
    report(ConversionProgress::new(
        ConversionPhase::SourceAdjustments,
        0.08,
        "Rendering saved source adjustments to local spool",
    ));
    let metadata = &stream.metadata;
    let channels = metadata.samples_per_pixel;
    let width = metadata.width as usize;
    let total_samples = width
        .checked_mul(metadata.height as usize)
        .and_then(|value| value.checked_mul(channels))
        .ok_or_else(|| "Conversion source spool sample count overflow.".to_owned())?;
    let total_bytes = total_samples
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| "Conversion source spool byte count overflow.".to_owned())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(spool_path)
        .map_err(|err| format!("Cannot create local conversion source spool: {err}"))?;
    file.set_len(total_bytes as u64)
        .map_err(|err| format!("Cannot size conversion source spool: {err}"))?;
    // SAFETY: this uniquely created file is exclusively owned for the mutable
    // mapping's lifetime and is sized before mapping.
    let mut mmap = unsafe {
        MmapOptions::new()
            .map_mut(&file)
            .map_err(|err| format!("Cannot map conversion source spool: {err}"))?
    };
    let project = capture.source_recipe.materialize_project();
    let total_pixels = u64::from(metadata.width).saturating_mul(u64::from(metadata.height));
    let mut processed_pixels = 0u64;
    tiff_io::for_each_decoded_region(
        &capture.source_face_path,
        stream,
        |x, y, region_width, region_height, input| {
            cancellation.check_before_commit()?;
            validate_region_bounds(
                x,
                y,
                region_width,
                region_height,
                metadata.width,
                metadata.height,
            )?;
            let adjusted = export::adjusted_strip(input, metadata, &project);
            let region_width_usize = region_width as usize;
            let region_height_usize = region_height as usize;
            let expected = region_width_usize
                .checked_mul(region_height_usize)
                .and_then(|value| value.checked_mul(channels))
                .ok_or_else(|| "Adjusted source region sample count overflow.".to_owned())?;
            if adjusted.len() != expected {
                return Err(format!(
                    "Adjusted source region contains {} samples; expected {expected}.",
                    adjusted.len()
                ));
            }
            for local_y in 0..region_height_usize {
                let source_start = local_y * region_width_usize * channels;
                let destination_sample = (((y as usize + local_y) * width) + x as usize) * channels;
                for offset in 0..region_width_usize * channels {
                    let bytes = adjusted[source_start + offset].to_ne_bytes();
                    let destination = (destination_sample + offset) * 2;
                    mmap[destination] = bytes[0];
                    mmap[destination + 1] = bytes[1];
                }
            }
            processed_pixels = processed_pixels
                .saturating_add(u64::from(region_width).saturating_mul(u64::from(region_height)));
            report(ConversionProgress::new(
                ConversionPhase::SourceAdjustments,
                0.08 + 0.40 * (processed_pixels as f32 / total_pixels.max(1) as f32),
                format!("Rendered {processed_pixels}/{total_pixels} source pixels"),
            ));
            Ok(())
        },
    )?;
    if processed_pixels != total_pixels {
        return Err(format!(
            "Decoded source regions covered {processed_pixels} pixels; expected {total_pixels}."
        ));
    }
    mmap.flush()
        .map_err(|err| format!("Cannot flush conversion source spool: {err}"))?;
    // The adjusted TIFF source spool is transient and is immediately reopened read-only by this
    // process. Do not pay durable-file sync cost before the real TIFF output transaction starts.
    drop(mmap);
    drop(file);
    Ok(())
}

enum RuntimeProductionTransform {
    Custom(ProductionCustomOptimizerRasterTransform),
    Cmyk(ProductionCmykTransform),
    N5(ProductionNChannelTransform<5>),
    N6(ProductionNChannelTransform<6>),
    N7(ProductionNChannelTransform<7>),
    N8(ProductionNChannelTransform<8>),
    N9(ProductionNChannelTransform<9>),
    N10(ProductionNChannelTransform<10>),
    N11(ProductionNChannelTransform<11>),
    N12(ProductionNChannelTransform<12>),
    LinkCmyk(ProductionDeviceLinkTransform<4>),
    LinkN5(ProductionDeviceLinkTransform<5>),
    LinkN6(ProductionDeviceLinkTransform<6>),
    LinkN7(ProductionDeviceLinkTransform<7>),
    LinkN8(ProductionDeviceLinkTransform<8>),
    LinkN9(ProductionDeviceLinkTransform<9>),
    LinkN10(ProductionDeviceLinkTransform<10>),
    LinkN11(ProductionDeviceLinkTransform<11>),
    LinkN12(ProductionDeviceLinkTransform<12>),
}

impl RuntimeProductionTransform {
    fn new(
        source_model: IccSourceModel,
        source_icc: &[u8],
        target_icc: &[u8],
        recipe: &ConversionRecipe,
    ) -> Result<Self, String> {
        if recipe.engine_mode == ConversionEngineMode::DeviceLink {
            macro_rules! create_link {
                ($n:literal, $variant:ident) => {
                    Ok(Self::$variant(ProductionDeviceLinkTransform::<$n>::new(
                        source_model,
                        RuntimeIccProfile::Embedded(target_icc),
                    )?))
                };
            }
            return match recipe.target.channels.len() {
                4 => create_link!(4, LinkCmyk),
                5 => create_link!(5, LinkN5),
                6 => create_link!(6, LinkN6),
                7 => create_link!(7, LinkN7),
                8 => create_link!(8, LinkN8),
                9 => create_link!(9, LinkN9),
                10 => create_link!(10, LinkN10),
                11 => create_link!(11, LinkN11),
                12 => create_link!(12, LinkN12),
                count => Err(format!(
                    "Unsupported DeviceLink output channel count: {count}."
                )),
            };
        }
        if recipe.engine_mode != ConversionEngineMode::Icc {
            return Err("Unsupported production conversion engine.".to_owned());
        }
        let create_n = |count: usize| -> Result<Self, String> {
            macro_rules! create {
                ($n:literal, $variant:ident) => {
                    Ok(Self::$variant(ProductionNChannelTransform::<$n>::new(
                        source_model,
                        RuntimeIccProfile::Embedded(source_icc),
                        RuntimeIccProfile::Embedded(target_icc),
                        recipe.rendering_intent,
                        recipe.black_point_compensation,
                    )?))
                };
            }
            match count {
                5 => create!(5, N5),
                6 => create!(6, N6),
                7 => create!(7, N7),
                8 => create!(8, N8),
                9 => create!(9, N9),
                10 => create!(10, N10),
                11 => create!(11, N11),
                12 => create!(12, N12),
                _ => Err(format!("Unsupported ICC output channel count: {count}.")),
            }
        };
        match recipe.target.channels.len() {
            4 => Ok(Self::Cmyk(ProductionCmykTransform::new(
                source_model,
                RuntimeIccProfile::Embedded(source_icc),
                RuntimeIccProfile::Embedded(target_icc),
                recipe.rendering_intent,
                recipe.black_point_compensation,
            )?)),
            count => create_n(count),
        }
    }

    fn output_channels(&self) -> usize {
        match self {
            Self::Custom(transform) => transform.output_channels(),
            Self::Cmyk(_) => 4,
            Self::N5(_) => 5,
            Self::N6(_) => 6,
            Self::N7(_) => 7,
            Self::N8(_) => 8,
            Self::N9(_) => 9,
            Self::N10(_) => 10,
            Self::N11(_) => 11,
            Self::N12(_) => 12,
            Self::LinkCmyk(_) => 4,
            Self::LinkN5(_) => 5,
            Self::LinkN6(_) => 6,
            Self::LinkN7(_) => 7,
            Self::LinkN8(_) => 8,
            Self::LinkN9(_) => 9,
            Self::LinkN10(_) => 10,
            Self::LinkN11(_) => 11,
            Self::LinkN12(_) => 12,
        }
    }

    fn transform(&self, source: &[u16], destination: &mut [u16]) -> Result<(), String> {
        match self {
            Self::Custom(_) => Err(
                "Custom Optimizer requires bit-depth-specific bounded raster dispatch.".to_owned(),
            ),
            Self::Cmyk(transform) => transform_cmyk(transform, source, destination),
            Self::N5(transform) => transform_n(transform, source, destination),
            Self::N6(transform) => transform_n(transform, source, destination),
            Self::N7(transform) => transform_n(transform, source, destination),
            Self::N8(transform) => transform_n(transform, source, destination),
            Self::N9(transform) => transform_n(transform, source, destination),
            Self::N10(transform) => transform_n(transform, source, destination),
            Self::N11(transform) => transform_n(transform, source, destination),
            Self::N12(transform) => transform_n(transform, source, destination),
            Self::LinkCmyk(transform) => transform_link(transform, source, destination),
            Self::LinkN5(transform) => transform_link(transform, source, destination),
            Self::LinkN6(transform) => transform_link(transform, source, destination),
            Self::LinkN7(transform) => transform_link(transform, source, destination),
            Self::LinkN8(transform) => transform_link(transform, source, destination),
            Self::LinkN9(transform) => transform_link(transform, source, destination),
            Self::LinkN10(transform) => transform_link(transform, source, destination),
            Self::LinkN11(transform) => transform_link(transform, source, destination),
            Self::LinkN12(transform) => transform_link(transform, source, destination),
        }
    }

    fn transform_u16_bounded(
        &mut self,
        source: &[u16],
        destination: &mut [u16],
        source_channels: usize,
    ) -> Result<(), String> {
        if let Self::Custom(transform) = self {
            let target_channels = transform.output_channels();
            transform_custom_optimizer_bounded(
                source,
                destination,
                source_channels,
                target_channels,
                |source_chunk, destination_chunk| {
                    transform
                        .transform_u16_chunk(source_chunk, destination_chunk)
                        .map_err(|error| {
                            format!("Custom Optimizer u16 raster chunk failed: {error:?}")
                        })
                },
            )
        } else {
            self.transform(source, destination)
        }
    }

    fn transform_u8_bounded(
        &mut self,
        source: &[u16],
        destination: &mut [u8],
        source_channels: usize,
    ) -> Result<(), String> {
        if let Self::Custom(transform) = self {
            let target_channels = transform.output_channels();
            transform_custom_optimizer_bounded(
                source,
                destination,
                source_channels,
                target_channels,
                |source_chunk, destination_chunk| {
                    transform
                        .transform_u8_chunk(source_chunk, destination_chunk)
                        .map_err(|error| {
                            format!("Custom Optimizer u8 raster chunk failed: {error:?}")
                        })
                },
            )
        } else {
            let mut converted = vec![0u16; destination.len()];
            self.transform(source, &mut converted)?;
            for (destination, source) in destination.iter_mut().zip(converted) {
                *destination = (source >> 8) as u8;
            }
            Ok(())
        }
    }
}

fn transform_custom_optimizer_bounded<T, F>(
    source: &[u16],
    destination: &mut [T],
    source_channels: usize,
    target_channels: usize,
    mut transform_chunk: F,
) -> Result<(), String>
where
    F: FnMut(&[u16], &mut [T]) -> Result<(), String>,
{
    if source_channels == 0 || target_channels == 0 {
        return Err("Custom Optimizer chunk topology cannot contain zero channels.".to_owned());
    }
    if source.len() % source_channels != 0 {
        return Err(format!(
            "Custom Optimizer source window has {} samples, not divisible by {} source channels.",
            source.len(),
            source_channels
        ));
    }
    let pixels = source.len() / source_channels;
    let expected_destination = pixels
        .checked_mul(target_channels)
        .ok_or_else(|| "Custom Optimizer destination sample count overflow.".to_owned())?;
    if destination.len() != expected_destination {
        return Err(format!(
            "Custom Optimizer destination window has {} samples; expected {expected_destination}.",
            destination.len()
        ));
    }

    let mut start_pixel = 0usize;
    while start_pixel < pixels {
        let end_pixel = start_pixel
            .saturating_add(MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS)
            .min(pixels);
        let source_start = start_pixel
            .checked_mul(source_channels)
            .ok_or_else(|| "Custom Optimizer source chunk offset overflow.".to_owned())?;
        let source_end = end_pixel
            .checked_mul(source_channels)
            .ok_or_else(|| "Custom Optimizer source chunk end overflow.".to_owned())?;
        let destination_start = start_pixel
            .checked_mul(target_channels)
            .ok_or_else(|| "Custom Optimizer destination chunk offset overflow.".to_owned())?;
        let destination_end = end_pixel
            .checked_mul(target_channels)
            .ok_or_else(|| "Custom Optimizer destination chunk end overflow.".to_owned())?;
        transform_chunk(
            &source[source_start..source_end],
            &mut destination[destination_start..destination_end],
        )?;
        start_pixel = end_pixel;
    }
    Ok(())
}

fn transform_cmyk(
    transform: &ProductionCmykTransform,
    source: &[u16],
    destination: &mut [u16],
) -> Result<(), String> {
    match transform.source_model() {
        IccSourceModel::Rgb => transform.transform_rgb_chunk(
            samples_as_arrays::<3>(source)?,
            samples_as_arrays_mut::<4>(destination)?,
        ),
        IccSourceModel::Cmyk => transform.transform_cmyk_chunk(
            samples_as_arrays::<4>(source)?,
            samples_as_arrays_mut::<4>(destination)?,
        ),
    }
}

fn transform_n<const N: usize>(
    transform: &ProductionNChannelTransform<N>,
    source: &[u16],
    destination: &mut [u16],
) -> Result<(), String> {
    match transform.source_model() {
        IccSourceModel::Rgb => transform.transform_rgb_chunk(
            samples_as_arrays::<3>(source)?,
            samples_as_arrays_mut::<N>(destination)?,
        ),
        IccSourceModel::Cmyk => transform.transform_cmyk_chunk(
            samples_as_arrays::<4>(source)?,
            samples_as_arrays_mut::<N>(destination)?,
        ),
    }
}

fn transform_link<const N: usize>(
    transform: &ProductionDeviceLinkTransform<N>,
    source: &[u16],
    destination: &mut [u16],
) -> Result<(), String> {
    match transform.source_model() {
        IccSourceModel::Rgb => transform.transform_rgb_chunk(
            samples_as_arrays::<3>(source)?,
            samples_as_arrays_mut::<N>(destination)?,
        ),
        IccSourceModel::Cmyk => transform.transform_cmyk_chunk(
            samples_as_arrays::<4>(source)?,
            samples_as_arrays_mut::<N>(destination)?,
        ),
    }
}

fn samples_as_arrays<const N: usize>(samples: &[u16]) -> Result<&[[u16; N]], String> {
    if samples.len() % N != 0 {
        return Err(format!(
            "Source transform buffer is not divisible by {N} channels."
        ));
    }
    // SAFETY: [u16; N] has u16 alignment and the length divisibility is checked.
    Ok(unsafe {
        std::slice::from_raw_parts(samples.as_ptr().cast::<[u16; N]>(), samples.len() / N)
    })
}

fn samples_as_arrays_mut<const N: usize>(samples: &mut [u16]) -> Result<&mut [[u16; N]], String> {
    if samples.len() % N != 0 {
        return Err(format!(
            "Target transform buffer is not divisible by {N} channels."
        ));
    }
    // SAFETY: [u16; N] has u16 alignment, the slice is uniquely borrowed, and
    // the length divisibility is checked.
    Ok(unsafe {
        std::slice::from_raw_parts_mut(samples.as_mut_ptr().cast::<[u16; N]>(), samples.len() / N)
    })
}

fn source_rows<'a>(
    samples: &'a [u16],
    start_row: u32,
    row_count: u32,
    width: usize,
    channels: usize,
) -> Result<&'a [u16], String> {
    let start = (start_row as usize)
        .checked_mul(width)
        .and_then(|value| value.checked_mul(channels))
        .ok_or_else(|| "Conversion source row offset overflow.".to_owned())?;
    let len = (row_count as usize)
        .checked_mul(width)
        .and_then(|value| value.checked_mul(channels))
        .ok_or_else(|| "Conversion source row length overflow.".to_owned())?;
    samples
        .get(start..start.saturating_add(len))
        .ok_or_else(|| "Conversion source spool does not contain requested rows.".to_owned())
}

fn validate_region_bounds(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    image_width: u32,
    image_height: u32,
) -> Result<(), String> {
    let x_end = x
        .checked_add(width)
        .ok_or_else(|| "Decoded source region x range overflow.".to_owned())?;
    let y_end = y
        .checked_add(height)
        .ok_or_else(|| "Decoded source region y range overflow.".to_owned())?;
    if width == 0 || height == 0 || x_end > image_width || y_end > image_height {
        return Err(format!(
            "Decoded source region ({x}, {y}, {width}, {height}) exceeds image bounds {image_width}x{image_height}."
        ));
    }
    Ok(())
}

fn conversion_spool_path() -> Result<PathBuf, String> {
    let root = std::env::temp_dir()
        .join("ShadeEditor")
        .join("conversion-spool");
    fs::create_dir_all(&root)
        .map_err(|err| format!("Cannot create local conversion spool folder: {err}"))?;
    let sequence = CONVERSION_SPOOL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(root.join(format!(
        "conversion-{}-{sequence}.u16.spool.tmp",
        std::process::id()
    )))
}

fn mmap_as_u16(mmap: &memmap2::Mmap) -> Result<&[u16], String> {
    if mmap.len() % 2 != 0 || (mmap.as_ptr() as usize) % std::mem::align_of::<u16>() != 0 {
        return Err("Conversion source spool is not a valid aligned u16 buffer.".to_owned());
    }
    // SAFETY: byte length and alignment are checked; the read-only mapping
    // remains alive for the returned slice lifetime.
    Ok(unsafe { std::slice::from_raw_parts(mmap.as_ptr().cast(), mmap.len() / 2) })
}

fn verify_file_sha256(path: &Path, expected: &str, label: &str) -> Result<(), String> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "{label} SHA-256 changed after the conversion job was captured."
        ))
    }
}

fn verify_bytes_sha256(bytes: &[u8], expected: &str, label: &str) -> Result<(), String> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "{label} SHA-256 no longer matches the captured recipe."
        ))
    }
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|err| format!("Cannot open file for SHA-256 {}: {err}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|err| format!("Cannot hash file {}: {err}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn unix_time_ms() -> Result<i64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("System clock is before Unix epoch: {err}"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| "Conversion timestamp exceeds i64 range.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::IccProfileIdentity;
    use lcms2::{ColorSpaceSignature, Profile};

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-icc-worker-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn minimal_recipe(policy: Option<SourceTransparencyPolicy>) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: policy,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "a".repeat(64),
            },
            target: ConversionTargetDefinition {
                name: "Target".to_owned(),
                channels: Vec::new(),
                bit_depth: 16,
                output_profile_identity: None,
                output_profile_path: None,
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

    #[test]
    fn full_file_sha256_detects_any_payload_change() {
        let path = temp_path("hash");
        fs::write(&path, b"production source bytes").unwrap();
        let first = sha256_file(&path).unwrap();
        verify_file_sha256(&path, &first, "Source Face").unwrap();
        fs::write(&path, b"production source byteS").unwrap();
        assert!(verify_file_sha256(&path, &first, "Source Face").is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn typed_sample_views_require_exact_channel_divisibility() {
        assert_eq!(
            samples_as_arrays::<3>(&[1, 2, 3, 4, 5, 6]).unwrap().len(),
            2
        );
        assert!(samples_as_arrays::<4>(&[1, 2, 3]).is_err());
        let mut output = [0u16; 14];
        assert_eq!(samples_as_arrays_mut::<7>(&mut output).unwrap().len(), 2);
    }

    #[test]
    fn source_row_window_is_overflow_and_bounds_checked() {
        let samples = (0..48).collect::<Vec<u16>>();
        assert_eq!(source_rows(&samples, 1, 1, 4, 3).unwrap(), &samples[12..24]);
        assert!(source_rows(&samples, 4, 1, 4, 3).is_err());
    }

    #[test]
    fn decoded_region_bounds_reject_overflow_and_out_of_image_tiles() {
        validate_region_bounds(3, 2, 4, 5, 10, 10).unwrap();
        assert!(validate_region_bounds(9, 0, 2, 1, 10, 10).is_err());
        assert!(validate_region_bounds(u32::MAX, 0, 2, 1, u32::MAX, 10).is_err());
        assert!(validate_region_bounds(0, 0, 0, 1, 10, 10).is_err());
    }

    #[test]
    fn invalid_default_dpi_is_rejected() {
        assert!(FilesystemIccConversionBackend::new(f64::NAN).is_err());
        assert!(FilesystemIccConversionBackend::new(0.0).is_err());
        assert!(FilesystemIccConversionBackend::new(220.0).is_ok());
    }

    #[test]
    fn rgb_working_adjustments_preserve_identity_without_project_edits() {
        let project = ShadeProject::default();
        let input = [0u16, 12345, u16::MAX, 32768, 22222, 11111];
        assert_eq!(adjust_working_rgb(&input, &project).unwrap(), input);
    }

    #[test]
    fn alpha_flatten_is_applied_after_saved_rgb_adjustments() {
        let mut project = ShadeProject::default();
        let mut red = crate::model::ChannelAdjustment::default();
        red.levels.output_black = 0.5;
        project.adjustments.insert("Red".to_owned(), red);
        let mut adjusted = adjust_working_rgb(&[0, 0, 0], &project).unwrap();
        assert!((i32::from(adjusted[0]) - 32768).abs() <= 1);
        let policy = SourceTransparencyPolicy::FlattenSolidRgb16 {
            background_rgb: [0, 0, 0],
        };
        flatten_adjusted_rgb_in_place(&mut adjusted, &[32768], policy).unwrap();
        assert!((i32::from(adjusted[0]) - 16384).abs() <= 1);
        assert_eq!(&adjusted[1..], &[0, 0]);
    }

    #[test]
    fn production_source_policy_fails_closed_for_missing_or_stale_alpha_policy() {
        let alpha_png = ProductionSourceRaster::Png(DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![1, 2, 3],
            alpha: Some(vec![32768]),
            icc_profile: Some(vec![1, 2, 3]),
            declares_srgb: false,
        });
        assert!(
            alpha_png
                .validate_transparency_policy(&minimal_recipe(None))
                .is_err()
        );
        let policy = SourceTransparencyPolicy::FlattenSolidRgb16 {
            background_rgb: [1000, 2000, 3000],
        };
        alpha_png
            .validate_transparency_policy(&minimal_recipe(Some(policy)))
            .unwrap();
        assert_eq!(alpha_png.embedded_icc(), Some(&[1, 2, 3][..]));

        let opaque_png = ProductionSourceRaster::Png(DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 8,
            model: PngSourceModel::Rgb,
            samples: vec![1, 2, 3],
            alpha: None,
            icc_profile: None,
            declares_srgb: false,
        });
        assert!(
            opaque_png
                .validate_transparency_policy(&minimal_recipe(Some(policy)))
                .is_err()
        );
    }

    #[test]
    fn gray_non_tiff_sources_fail_before_production_transform_dispatch() {
        let gray_png = ProductionSourceRaster::Png(DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 8,
            model: PngSourceModel::Gray,
            samples: vec![12345],
            alpha: None,
            icc_profile: None,
            declares_srgb: false,
        });
        let error = gray_png.source_model().expect_err("Gray PNG must fail closed");
        assert!(error.contains("RGB PNG"), "{error}");
    }

    #[test]
    fn custom_optimizer_bounded_adapter_splits_oversized_seven_channel_window() {
        let pixels = MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS * 2 + 17;
        let source_channels = 3usize;
        let target_channels = 7usize;
        let source = vec![0u16; pixels * source_channels];
        let mut destination = vec![0u8; pixels * target_channels];
        let mut chunk_pixels = Vec::new();

        transform_custom_optimizer_bounded(
            &source,
            &mut destination,
            source_channels,
            target_channels,
            |source_chunk, destination_chunk| {
                let chunk = source_chunk.len() / source_channels;
                assert!(chunk <= MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS);
                assert_eq!(destination_chunk.len(), chunk * target_channels);
                chunk_pixels.push(chunk);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            chunk_pixels,
            vec![
                MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS,
                MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS,
                17,
            ]
        );
    }

    #[test]
    fn custom_optimizer_bounded_adapter_rejects_mismatched_topology() {
        let source = [0u16; 7];
        let mut destination = [0u16; 14];
        assert!(
            transform_custom_optimizer_bounded(
                &source,
                &mut destination,
                3,
                7,
                |_source, _destination| Ok(()),
            )
            .is_err()
        );
    }

    #[test]
    fn runtime_dispatch_executes_captured_devicelink_without_source_icc_chain() {
        let link = Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0)
            .unwrap()
            .icc()
            .unwrap();
        let recipe = ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::DeviceLink,
            source_profile_identity: IccProfileIdentity {
                description: "Captured source".to_owned(),
                sha256: "a".repeat(64),
            },
            target: ConversionTargetDefinition {
                name: "Direct CMYK link".to_owned(),
                channels: ["Cyan", "Magenta", "Yellow", "Black"]
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: None,
                    })
                    .to_vec(),
                bit_depth: 16,
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: Some(IccProfileIdentity {
                    description: "240% ink limit".to_owned(),
                    sha256: "b".repeat(64),
                }),
                device_link_path: Some("fixture.icc".to_owned()),
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: ConversionRenderingIntent::AbsoluteColorimetric,
            black_point_compensation: true,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        };
        let transform = RuntimeProductionTransform::new(
            IccSourceModel::Cmyk,
            &Profile::new_srgb().icc().unwrap(),
            &link,
            &recipe,
        )
        .unwrap();
        let source = [60_000u16, 50_000, 40_000, 30_000];
        let mut output = [0u16; 4];
        transform.transform(&source, &mut output).unwrap();

        assert_eq!(transform.output_channels(), 4);
        assert!(output.into_iter().map(u64::from).sum::<u64>() <= 157_290);
    }
}
