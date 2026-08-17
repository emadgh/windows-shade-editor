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
use crate::devicelink_conversion::ProductionDeviceLinkTransform;
use crate::icc_conversion::{IccSourceModel, ProductionCmykTransform, RuntimeIccProfile};
use crate::model::ShadeProject;
use crate::nchannel_icc::ProductionNChannelTransform;
use crate::tiff_io::{self, ColorModel, StreamInfo};
use crate::{dpi, export};

static CONVERSION_SPOOL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
        if !self.replace_existing
            && (capture.output_tiff_path.exists() || capture.production_project_path.exists())
        {
            return Err(
                "Queued versioned conversion destination is no longer free; review and queue a new version."
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
            "Inspecting streamable source topology",
        ));
        let stream = tiff_io::stream_info(&capture.source_face_path)?;
        let source_model = source_model(&stream)?;
        if !stream.streamable {
            return Err(
                "Production conversion requires a strip/tile-streamable TIFF source; full-image fallback is disabled to preserve bounded memory."
                    .to_owned(),
            );
        }

        let profiles = load_verified_profiles(capture, &stream)?;
        let transform = RuntimeProductionTransform::new(
            source_model,
            &profiles.source_icc,
            &profiles.transform_icc,
            &capture.conversion_recipe,
        )?;
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
            &stream,
            profiles
                .embed_output_icc
                .then_some(profiles.transform_icc.as_slice()),
            &transform,
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

fn source_model(stream: &StreamInfo) -> Result<IccSourceModel, String> {
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

fn load_verified_profiles(
    capture: &ConversionJobCapture,
    stream: &StreamInfo,
) -> Result<VerifiedConversionProfiles, String> {
    let source_icc = match &capture.source_profile {
        CapturedSourceProfile::Embedded => {
            stream.metadata.icc_profile.clone().ok_or_else(|| {
                "Captured source expects an embedded ICC, but the TIFF has none.".to_owned()
            })?
        }
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
    stream: &StreamInfo,
    target_icc: Option<&[u8]>,
    transform: &RuntimeProductionTransform,
    default_dpi: f64,
) -> Result<(), String> {
    let spool_path = conversion_spool_path()?;
    let result = (|| {
        render_adjusted_source_spool(capture, cancellation, report, stream, &spool_path)?;
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
        let metadata = &stream.metadata;
        let dpi = dpi::read_dpi(&capture.source_face_path, default_dpi);
        let channel_names = capture
            .conversion_recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        let spec = ConversionTiffSpec {
            width: metadata.width,
            height: metadata.height,
            channel_names: &channel_names,
            target_icc,
            dpi_x: dpi.dpi_x,
            dpi_y: dpi.dpi_y,
            orientation: metadata.orientation,
            rows_per_strip: stream.rows_per_strip.max(1),
            force_bigtiff: false,
            replace_existing: capture.output_policy == CapturedOutputPolicy::TransactionalReplace,
        };
        let source_channels = metadata.samples_per_pixel;
        let width = metadata.width as usize;
        let height = metadata.height.max(1) as f32;

        match capture.conversion_recipe.target.bit_depth {
            16 => write_conversion_tiff_u16_atomic(
                &capture.output_tiff_path,
                &spec,
                |start_row, row_count, output| {
                    cancellation.check_before_commit()?;
                    let input =
                        source_rows(source_samples, start_row, row_count, width, source_channels)?;
                    transform.transform(input, output)?;
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
                    let mut converted = vec![0u16; output.len()];
                    transform.transform(input, &mut converted)?;
                    for (destination, source) in output.iter_mut().zip(converted) {
                        *destination = (source >> 8) as u8;
                    }
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
    file.sync_all()
        .map_err(|err| format!("Cannot sync conversion source spool: {err}"))?;
    Ok(())
}

enum RuntimeProductionTransform {
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
    fn runtime_dispatch_executes_captured_devicelink_without_source_icc_chain() {
        let link = Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0)
            .unwrap()
            .icc()
            .unwrap();
        let recipe = ConversionRecipe {
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
