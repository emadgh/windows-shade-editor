use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use fontdue::{Font, FontSettings};
use memmap2::MmapOptions;
use tiff::encoder::{Compression, Predictor, TiffEncoder, colortype};
use tiff::tags::{ExtraSamples, Tag};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

use crate::dpi::{self, DpiInfo};
use crate::model::{
    ShadeProject, TEST_CODE_ALL_CHANNELS, TestCodePosition, apply_curve, apply_levels,
};
use crate::tiff_io::{
    ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_region,
    for_each_decoded_strip, stream_info, tiff_sample_from_working, working_sample_from_tiff,
};

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    pub force_lzw: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { force_lzw: true }
    }
}

pub fn export_face(
    source: &Path,
    destination: &Path,
    project: &ShadeProject,
    default_dpi: f64,
) -> Result<(), String> {
    export_face_with_progress(source, destination, project, default_dpi, |_, _| {})
}

pub fn export_face_with_progress<F>(
    source: &Path,
    destination: &Path,
    project: &ShadeProject,
    default_dpi: f64,
    progress: F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    export_face_with_progress_options(
        source,
        destination,
        project,
        default_dpi,
        ExportOptions::default(),
        progress,
    )
}

pub fn export_face_with_progress_options<F>(
    source: &Path,
    destination: &Path,
    project: &ShadeProject,
    default_dpi: f64,
    options: ExportOptions,
    mut progress: F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    let temporary = temporary_export_path(destination)?;
    let result = export_face_direct_with_progress(
        source,
        &temporary,
        project,
        default_dpi,
        options,
        |fraction, detail| progress((fraction * 0.98).clamp(0.0, 0.98), detail),
    );
    if let Err(err) = result {
        let _ = fs::remove_file(&temporary);
        return Err(err);
    }
    progress(0.99, "Committing TIFF atomically");
    if let Err(err) = atomic_replace(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(err);
    }
    progress(1.0, "Export complete");
    Ok(())
}

fn export_face_direct_with_progress<F>(
    source: &Path,
    destination: &Path,
    project: &ShadeProject,
    default_dpi: f64,
    options: ExportOptions,
    mut progress: F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    progress(0.02, "Inspecting TIFF");
    let stream = stream_info(source)?;
    if stream.streamable {
        return export_face_streaming(
            source,
            destination,
            project,
            default_dpi,
            options,
            &stream,
            &mut progress,
        );
    }
    progress(0.02, "Compatibility decode");
    let decoded = decode_full(source)?;
    let dpi_info = dpi::read_dpi(source, default_dpi);
    let channels = decoded.metadata.samples_per_pixel;
    let base_channels = decoded.metadata.base_channel_count;
    if channels == 0 || channels < base_channels {
        return Err("Invalid TIFF channel layout.".to_owned());
    }
    if !matches!(
        decoded.metadata.color_model,
        ColorModel::Rgb | ColorModel::Cmyk | ColorModel::Gray
    ) {
        return Err(format!(
            "Export currently supports RGB, CMYK and Gray TIFF; this file is {}.",
            decoded.metadata.color_model.title()
        ));
    }

    let width = decoded.metadata.width as usize;
    let height = decoded.metadata.height as usize;
    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| "Image is too large.".to_owned())?;
    let expected = pixel_count
        .checked_mul(channels)
        .ok_or_else(|| "Image sample count is too large.".to_owned())?;
    if decoded.samples.len() < expected {
        return Err("Decoded TIFF sample buffer is incomplete.".to_owned());
    }

    let names = &decoded.metadata.channel_names;
    let mut output = vec![0u16; expected];
    let mut prepared = vec![0.0f32; channels];

    progress(0.08, "Applying adjustments");
    let progress_step = (height / 100).max(1);
    for y in 0..height {
        for x in 0..width {
            let pixel = y * width + x;
            let base = pixel * channels;
            for channel in 0..channels {
                let raw = working_sample_from_tiff(
                    &decoded.metadata,
                    channel,
                    decoded.samples[base + channel],
                ) as f32
                    / 65535.0;
                prepared[channel] = match project.adjustments.get(&names[channel]) {
                    Some(adjustment) if adjustment.enabled => apply_levels(raw, adjustment.levels),
                    _ => raw,
                };
            }

            for out_channel in 0..channels {
                let value = match project.adjustments.get(&names[out_channel]) {
                    Some(adjustment) if adjustment.enabled => {
                        let mut mixed = adjustment.mixer.constant;
                        for source_channel in 0..channels {
                            let coefficient = adjustment
                                .mixer
                                .coefficients
                                .get(&names[source_channel])
                                .copied()
                                .unwrap_or(if source_channel == out_channel {
                                    1.0
                                } else {
                                    0.0
                                });
                            mixed += prepared[source_channel] * coefficient;
                        }
                        apply_curve(mixed, adjustment.curve)
                    }
                    _ => prepared[out_channel],
                };
                let working = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
                output[base + out_channel] =
                    tiff_sample_from_working(&decoded.metadata, out_channel, working);
            }
        }
        if y % progress_step == 0 {
            let fraction = y as f32 / height.max(1) as f32;
            progress(0.08 + fraction * 0.72, "Applying adjustments");
        }
    }

    if let Some(overlay) =
        build_project_test_code_overlay(width, height, &decoded.metadata, project, dpi_info)?
    {
        progress(0.82, "Rendering test code");
        apply_text_overlay_to_rows(&mut output, 0, height, width, channels, &overlay);
    }

    progress(0.88, "Writing TIFF");
    match decoded.metadata.bit_depth {
        8 => {
            let data = output
                .into_iter()
                .map(|value| (value >> 8) as u8)
                .collect::<Vec<_>>();
            write_tiff_pixels(
                source,
                destination,
                &decoded.metadata,
                dpi_info,
                options,
                None,
                OutputPixels::U8(&data),
            )?;
        }
        16 => {
            write_tiff_pixels(
                source,
                destination,
                &decoded.metadata,
                dpi_info,
                options,
                None,
                OutputPixels::U16(&output),
            )?;
        }
        depth => {
            return Err(format!(
                "Unsupported export bit depth/color model: {depth}-bit."
            ));
        }
    }

    progress(1.0, "Export complete");
    Ok(())
}

fn export_face_streaming<F>(
    source: &Path,
    destination: &Path,
    project: &ShadeProject,
    default_dpi: f64,
    options: ExportOptions,
    stream: &StreamInfo,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    let metadata = &stream.metadata;
    if !matches!(
        metadata.color_model,
        ColorModel::Rgb | ColorModel::Cmyk | ColorModel::Gray
    ) {
        return Err(format!(
            "Export currently supports RGB, CMYK and Gray TIFF; this file is {}.",
            metadata.color_model.title()
        ));
    }
    let channels = metadata.samples_per_pixel;
    let base_channels = metadata.base_channel_count;
    if channels == 0 || channels < base_channels {
        return Err("Invalid TIFF channel layout.".to_owned());
    }
    if !matches!(metadata.bit_depth, 8 | 16) {
        return Err(format!(
            "Unsupported export bit depth/color model: {}-bit.",
            metadata.bit_depth
        ));
    }

    let dpi_info = dpi::read_dpi(source, default_dpi);
    let overlay = build_project_test_code_overlay(
        metadata.width as usize,
        metadata.height as usize,
        metadata,
        project,
        dpi_info,
    )?;
    let spool_path = temporary_spool_path(destination)?;

    let result = (|| -> Result<(), String> {
        progress(0.05, "Streaming adjustments to disk spool");
        if stream.row_streamable {
            let spool_file = File::create(&spool_path)
                .map_err(|err| format!("Cannot create export spool: {err}"))?;
            let mut spool = BufWriter::new(spool_file);
            match metadata.bit_depth {
                8 => stream_spool_u8(
                    source,
                    stream,
                    project,
                    overlay.as_ref(),
                    &mut spool,
                    progress,
                )?,
                16 => stream_spool_u16(
                    source,
                    stream,
                    project,
                    overlay.as_ref(),
                    &mut spool,
                    progress,
                )?,
                _ => unreachable!(),
            }
            spool
                .flush()
                .map_err(|err| format!("Cannot flush export spool: {err}"))?;
        } else {
            stream_spool_regions(
                source,
                stream,
                project,
                overlay.as_ref(),
                &spool_path,
                progress,
            )?;
        }

        let bytes_per_sample = u64::from(metadata.bit_depth / 8);
        let expected_bytes = u64::from(metadata.width)
            .checked_mul(u64::from(metadata.height))
            .and_then(|value| value.checked_mul(channels as u64))
            .and_then(|value| value.checked_mul(bytes_per_sample))
            .ok_or_else(|| "Export spool size overflow.".to_owned())?;
        let actual_bytes = fs::metadata(&spool_path)
            .map_err(|err| format!("Cannot inspect export spool: {err}"))?
            .len();
        if actual_bytes != expected_bytes {
            return Err(format!(
                "Export spool size mismatch: wrote {actual_bytes} bytes, expected {expected_bytes}."
            ));
        }

        // image-tiff 0.11.x only activates LZW/Deflate/PackBits in
        // ImageEncoder::write_data(). Direct write_strip() calls do not turn
        // the compressor on even though the Compression TIFF tag is present.
        // Keep adjustment processing strip-streamed into a disk-backed spool,
        // then memory-map that spool and let write_data() perform the final
        // correctly compressed strip encoding without allocating the full
        // image in RAM.
        progress(0.72, "Compressing TIFF from disk-backed spool");
        let spool_file =
            File::open(&spool_path).map_err(|err| format!("Cannot reopen export spool: {err}"))?;
        // SAFETY: this mapping is read-only, the spool file is no longer
        // written after the map is created, and it stays open for the map's
        // lifetime inside this closure.
        let mmap = unsafe {
            MmapOptions::new()
                .map(&spool_file)
                .map_err(|err| format!("Cannot map export spool: {err}"))?
        };

        match metadata.bit_depth {
            8 => {
                write_tiff_pixels(
                    source,
                    destination,
                    metadata,
                    dpi_info,
                    options,
                    Some(stream.rows_per_strip),
                    OutputPixels::U8(&mmap[..]),
                )?;
            }
            16 => {
                let data = mmap_as_u16(&mmap)?;
                write_tiff_pixels(
                    source,
                    destination,
                    metadata,
                    dpi_info,
                    options,
                    Some(stream.rows_per_strip),
                    OutputPixels::U16(data),
                )?;
            }
            depth => {
                return Err(format!(
                    "Unsupported export bit depth/color model: {depth}-bit."
                ));
            }
        }

        progress(1.0, "Export complete");
        Ok(())
    })();

    let _ = fs::remove_file(&spool_path);
    result
}

fn adjusted_strip(input: &[u16], metadata: &TiffMetadata, project: &ShadeProject) -> Vec<u16> {
    let channels = metadata.samples_per_pixel;
    let names = &metadata.channel_names;
    let pixel_count = input.len() / channels.max(1);
    let mut output = vec![0u16; pixel_count.saturating_mul(channels)];
    let mut prepared = vec![0.0f32; channels];
    for pixel in 0..pixel_count {
        let base = pixel * channels;
        for channel in 0..channels {
            let raw =
                working_sample_from_tiff(metadata, channel, input[base + channel]) as f32 / 65535.0;
            prepared[channel] = match project.adjustments.get(&names[channel]) {
                Some(adjustment) if adjustment.enabled => apply_levels(raw, adjustment.levels),
                _ => raw,
            };
        }
        for out_channel in 0..channels {
            let value = match project.adjustments.get(&names[out_channel]) {
                Some(adjustment) if adjustment.enabled => {
                    let mut mixed = adjustment.mixer.constant;
                    for source_channel in 0..channels {
                        let coefficient = adjustment
                            .mixer
                            .coefficients
                            .get(&names[source_channel])
                            .copied()
                            .unwrap_or(if source_channel == out_channel {
                                1.0
                            } else {
                                0.0
                            });
                        mixed += prepared[source_channel] * coefficient;
                    }
                    apply_curve(mixed, adjustment.curve)
                }
                _ => prepared[out_channel],
            };
            let working = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
            output[base + out_channel] = tiff_sample_from_working(metadata, out_channel, working);
        }
    }
    output
}

fn stream_spool_u8<W, F>(
    source: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    writer: &mut W,
    progress: &mut F,
) -> Result<(), String>
where
    W: Write,
    F: FnMut(f32, &str),
{
    let channels = stream.metadata.samples_per_pixel;
    let width = stream.metadata.width as usize;
    for_each_decoded_strip(source, stream, |row_start, row_count, input| {
        let mut adjusted = adjusted_strip(input, &stream.metadata, project);
        if let Some(overlay) = overlay {
            apply_text_overlay_to_rows(
                &mut adjusted,
                row_start as usize,
                row_count as usize,
                width,
                channels,
                overlay,
            );
        }
        let data = adjusted
            .into_iter()
            .map(|value| (value >> 8) as u8)
            .collect::<Vec<_>>();
        let expected = row_count as usize * width * channels;
        if data.len() != expected {
            return Err(format!(
                "Output strip sample mismatch: generated {}, expected {expected}.",
                data.len()
            ));
        }
        writer
            .write_all(&data)
            .map_err(|err| format!("Cannot write export spool: {err}"))?;
        let done =
            row_start.saturating_add(row_count) as f32 / stream.metadata.height.max(1) as f32;
        progress(0.06 + done * 0.60, "Streaming adjustments to disk spool");
        Ok(())
    })
}

fn stream_spool_u16<W, F>(
    source: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    writer: &mut W,
    progress: &mut F,
) -> Result<(), String>
where
    W: Write,
    F: FnMut(f32, &str),
{
    let channels = stream.metadata.samples_per_pixel;
    let width = stream.metadata.width as usize;
    for_each_decoded_strip(source, stream, |row_start, row_count, input| {
        let mut adjusted = adjusted_strip(input, &stream.metadata, project);
        if let Some(overlay) = overlay {
            apply_text_overlay_to_rows(
                &mut adjusted,
                row_start as usize,
                row_count as usize,
                width,
                channels,
                overlay,
            );
        }
        let expected = row_count as usize * width * channels;
        if adjusted.len() != expected {
            return Err(format!(
                "Output strip sample mismatch: generated {}, expected {expected}.",
                adjusted.len()
            ));
        }
        let mut bytes = Vec::with_capacity(adjusted.len().saturating_mul(2));
        for value in adjusted {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        writer
            .write_all(&bytes)
            .map_err(|err| format!("Cannot write export spool: {err}"))?;
        let done =
            row_start.saturating_add(row_count) as f32 / stream.metadata.height.max(1) as f32;
        progress(0.06 + done * 0.60, "Streaming adjustments to disk spool");
        Ok(())
    })
}

fn stream_spool_regions<F>(
    source: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    spool_path: &Path,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    let metadata = &stream.metadata;
    let channels = metadata.samples_per_pixel;
    let full_width = metadata.width as usize;
    let bytes_per_sample = usize::from(metadata.bit_depth / 8);
    let total_samples = (metadata.width as usize)
        .checked_mul(metadata.height as usize)
        .and_then(|value| value.checked_mul(channels))
        .ok_or_else(|| "Export spool sample count overflow.".to_owned())?;
    let total_bytes = total_samples
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| "Export spool size overflow.".to_owned())?;

    let spool_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(spool_path)
        .map_err(|err| format!("Cannot create random-access export spool: {err}"))?;
    spool_file
        .set_len(total_bytes as u64)
        .map_err(|err| format!("Cannot size export spool: {err}"))?;
    let mut mmap = unsafe {
        MmapOptions::new()
            .map_mut(&spool_file)
            .map_err(|err| format!("Cannot map export spool for writing: {err}"))?
    };
    let total_pixels = u64::from(metadata.width).saturating_mul(u64::from(metadata.height));
    let mut processed_pixels = 0u64;

    for_each_decoded_region(source, stream, |x, y, width, height, input| {
        let region_width = width as usize;
        let region_height = height as usize;
        let mut adjusted = adjusted_strip(input, metadata, project);
        if let Some(overlay) = overlay {
            apply_text_overlay_to_region(
                &mut adjusted,
                x as usize,
                y as usize,
                region_width,
                region_height,
                channels,
                overlay,
            );
        }
        let expected = region_width
            .checked_mul(region_height)
            .and_then(|value| value.checked_mul(channels))
            .ok_or_else(|| "Output region sample count overflow.".to_owned())?;
        if adjusted.len() != expected {
            return Err(format!(
                "Output region sample mismatch: generated {}, expected {expected}.",
                adjusted.len()
            ));
        }

        for local_y in 0..region_height {
            let source_sample = local_y * region_width * channels;
            let destination_sample = ((y as usize + local_y) * full_width + x as usize) * channels;
            let row_samples = region_width * channels;
            match metadata.bit_depth {
                8 => {
                    let destination = destination_sample;
                    for offset in 0..row_samples {
                        mmap[destination + offset] = (adjusted[source_sample + offset] >> 8) as u8;
                    }
                }
                16 => {
                    let destination = destination_sample * 2;
                    for offset in 0..row_samples {
                        let bytes = adjusted[source_sample + offset].to_ne_bytes();
                        let index = destination + offset * 2;
                        mmap[index] = bytes[0];
                        mmap[index + 1] = bytes[1];
                    }
                }
                depth => {
                    return Err(format!(
                        "Unsupported streaming spool bit depth: {depth}-bit."
                    ));
                }
            }
        }

        processed_pixels =
            processed_pixels.saturating_add(u64::from(width).saturating_mul(u64::from(height)));
        let done = processed_pixels as f32 / total_pixels.max(1) as f32;
        progress(
            0.06 + done.min(1.0) * 0.60,
            "Streaming TIFF regions to disk spool",
        );
        Ok(())
    })?;
    mmap.flush()
        .map_err(|err| format!("Cannot flush random-access export spool: {err}"))?;
    Ok(())
}

fn mmap_as_u16(mmap: &memmap2::Mmap) -> Result<&[u16], String> {
    if mmap.len() % std::mem::size_of::<u16>() != 0 {
        return Err("16-bit export spool has an odd byte length.".to_owned());
    }
    if (mmap.as_ptr() as usize) % std::mem::align_of::<u16>() != 0 {
        return Err("16-bit export spool is not aligned for u16 samples.".to_owned());
    }
    // SAFETY: length and alignment are checked above. The read-only mmap stays
    // alive for the returned slice lifetime and is never mutated concurrently.
    Ok(unsafe {
        std::slice::from_raw_parts(
            mmap.as_ptr().cast::<u16>(),
            mmap.len() / std::mem::size_of::<u16>(),
        )
    })
}

fn configure_extras_and_metadata<W, C, K>(
    image: &mut tiff::encoder::ImageEncoder<'_, W, C, K>,
    channels: usize,
    base_channels: usize,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    C: tiff::encoder::colortype::ColorType,
    K: tiff::encoder::TiffKind,
{
    let extra_count = channels.saturating_sub(base_channels);
    if extra_count > 0 {
        let extras = (0..extra_count)
            .map(|_| ExtraSamples::Unspecified)
            .collect::<Vec<_>>();
        image
            .extra_samples(&extras)
            .map_err(|err| format!("Cannot configure extra/spot channels: {err}"))?;
    }

    let (resolution_x, resolution_y, resolution_unit) = dpi_info.effective_tiff_resolution();
    image.x_resolution(dpi::rational(resolution_x));
    image.y_resolution(dpi::rational(resolution_y));
    image
        .encoder()
        .write_tag(Tag::ResolutionUnit, resolution_unit)
        .map_err(|err| format!("Cannot preserve/write TIFF resolution unit: {err}"))?;

    if let Some(orientation) = metadata.orientation {
        image
            .encoder()
            .write_tag(Tag::Orientation, orientation)
            .map_err(|err| format!("Cannot preserve TIFF orientation: {err}"))?;
    }

    if let Some(profile) = &metadata.icc_profile {
        image
            .encoder()
            .write_tag(Tag::IccProfile, profile.as_slice())
            .map_err(|err| format!("Cannot preserve ICC profile: {err}"))?;
    }
    if let Some(resources) = &metadata.photoshop_resources {
        image
            .encoder()
            .write_tag(Tag::Unknown(34377), resources.as_slice())
            .map_err(|err| format!("Cannot preserve Photoshop Image Resources: {err}"))?;
    }
    if let Some(source_data) = &metadata.photoshop_image_source_data {
        image
            .encoder()
            .write_tag(Tag::Unknown(37724), source_data.as_slice())
            .map_err(|err| format!("Cannot preserve Photoshop ImageSourceData: {err}"))?;
    }
    image
        .encoder()
        .write_tag(Tag::Software, "Shade Editor")
        .map_err(|err| format!("Cannot write TIFF software tag: {err}"))?;
    Ok(())
}

#[derive(Clone, Copy)]
enum OutputPixels<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
}

// Classic TIFF uses 32-bit offsets. Keep a conservative margin below the
// absolute 4 GiB address limit so strip tables, metadata, ICC/Photoshop
// resources and encoder overhead cannot push a nominally-safe image over it.
const CLASSIC_TIFF_SAFE_RAW_BYTES: u64 = 4_000_000_000;

fn source_is_bigtiff(source: &Path) -> Result<bool, String> {
    let mut file =
        File::open(source).map_err(|err| format!("Cannot inspect source TIFF header: {err}"))?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)
        .map_err(|err| format!("Cannot read source TIFF header: {err}"))?;
    match header {
        [b'I', b'I', 43, 0] | [b'M', b'M', 0, 43] => Ok(true),
        [b'I', b'I', 42, 0] | [b'M', b'M', 0, 42] => Ok(false),
        _ => Err("Source does not have a valid TIFF/BigTIFF header.".to_owned()),
    }
}

fn raw_image_bytes(width: u32, height: u32, channels: usize, bit_depth: u8) -> Option<u64> {
    let bytes_per_sample = u64::from(bit_depth / 8);
    u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(channels as u64)?
        .checked_mul(bytes_per_sample)
}

fn layout_requires_bigtiff_values(width: u32, height: u32, channels: usize, bit_depth: u8) -> bool {
    raw_image_bytes(width, height, channels, bit_depth)
        .map(|bytes| bytes >= CLASSIC_TIFF_SAFE_RAW_BYTES)
        .unwrap_or(true)
}

fn should_write_bigtiff(source: &Path, metadata: &TiffMetadata) -> Result<bool, String> {
    Ok(source_is_bigtiff(source)?
        || layout_requires_bigtiff_values(
            metadata.width,
            metadata.height,
            metadata.samples_per_pixel,
            metadata.bit_depth,
        ))
}

fn configure_tiff_encoder<W, K>(
    mut encoder: TiffEncoder<W, K>,
    metadata: &TiffMetadata,
    options: ExportOptions,
) -> TiffEncoder<W, K>
where
    W: std::io::Write + std::io::Seek,
    K: tiff::encoder::TiffKind,
{
    let compression = if options.force_lzw {
        Compression::Lzw
    } else {
        match metadata.compression {
            Some(1) => Compression::Uncompressed,
            Some(5) => Compression::Lzw,
            Some(8 | 32946) => Compression::Deflate(tiff::encoder::DeflateLevel::Balanced),
            Some(32773) => Compression::Packbits,
            _ => Compression::Lzw,
        }
    };
    encoder = encoder.with_compression(compression);
    if metadata.predictor == Some(2) && metadata.samples_per_pixel == metadata.base_channel_count {
        encoder = encoder.with_predictor(Predictor::Horizontal);
    }
    encoder
}

fn write_tiff_pixels(
    source: &Path,
    destination: &Path,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    options: ExportOptions,
    rows_per_strip: Option<u32>,
    pixels: OutputPixels<'_>,
) -> Result<(), String> {
    let file =
        File::create(destination).map_err(|err| format!("Cannot create export TIFF: {err}"))?;
    let writer = BufWriter::new(file);
    if should_write_bigtiff(source, metadata)? {
        let encoder = TiffEncoder::new_big(writer)
            .map_err(|err| format!("Cannot initialize BigTIFF encoder: {err}"))?;
        let mut encoder = configure_tiff_encoder(encoder, metadata, options);
        write_tiff_with_encoder(&mut encoder, metadata, dpi_info, rows_per_strip, pixels)
    } else {
        let encoder = TiffEncoder::new(writer)
            .map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;
        let mut encoder = configure_tiff_encoder(encoder, metadata, options);
        write_tiff_with_encoder(&mut encoder, metadata, dpi_info, rows_per_strip, pixels)
    }
}

fn write_tiff_with_encoder<W, K>(
    encoder: &mut TiffEncoder<W, K>,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    rows_per_strip: Option<u32>,
    pixels: OutputPixels<'_>,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    K: tiff::encoder::TiffKind,
{
    let channels = metadata.samples_per_pixel;
    match (metadata.color_model, metadata.bit_depth, pixels) {
        (ColorModel::Rgb, 8, OutputPixels::U8(data)) => {
            let mut image = encoder
                .new_image::<colortype::RGB8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create RGB 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Rgb, 16, OutputPixels::U16(data)) => {
            let mut image = encoder
                .new_image::<colortype::RGB16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create RGB 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Cmyk, 8, OutputPixels::U8(data)) => {
            let mut image = encoder
                .new_image::<colortype::CMYK8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Cmyk, 16, OutputPixels::U16(data)) => {
            let mut image = encoder
                .new_image::<colortype::CMYK16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Gray, 8, OutputPixels::U8(data)) => {
            let mut image = encoder
                .new_image::<colortype::Gray8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create Gray 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 1, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Gray, 16, OutputPixels::U16(data)) => {
            let mut image = encoder
                .new_image::<colortype::Gray16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create Gray 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 1, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (_, depth, _) => {
            return Err(format!(
                "Unsupported export bit depth/color model: {depth}-bit."
            ));
        }
    }
    Ok(())
}

fn temporary_spool_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export.tif");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(
            ".{file_name}.shade-editor-spool-{}-{stamp}-{attempt}.raw",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Cannot allocate a temporary export spool beside the destination.".to_owned())
}

fn temporary_export_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export.tif");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(
            ".{file_name}.shade-editor-{}-{stamp}-{attempt}.tmp",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Cannot allocate a temporary export file beside the destination.".to_owned())
}

fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
    let moved = unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), flags) };
    if moved == 0 {
        return Err(format!(
            "Cannot atomically replace {}: {}",
            destination.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn find_windows_font(family: &str) -> Result<PathBuf, String> {
    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let fonts = windows.join("Fonts");
    let mut candidates = Vec::new();
    if family.eq_ignore_ascii_case("Tahoma") {
        candidates.push(fonts.join("tahoma.ttf"));
        candidates.push(fonts.join("tahomabd.ttf"));
    }
    candidates.push(fonts.join("segoeui.ttf"));
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| format!("Cannot find Tahoma/Segoe UI in {}", fonts.display()))
}

struct TextOverlay {
    x0: usize,
    y0: usize,
    targets: Vec<(usize, u16)>,
    bitmap: TextBitmap,
}

fn test_code_target_value(metadata: &TiffMetadata, channel: usize) -> u16 {
    if channel >= metadata.base_channel_count {
        0
    } else if metadata.color_model == ColorModel::Cmyk {
        u16::MAX
    } else {
        0
    }
}

fn test_code_targets(metadata: &TiffMetadata, project: &ShadeProject) -> Vec<(usize, u16)> {
    if project.test_code.channel == TEST_CODE_ALL_CHANNELS {
        return (0..metadata.samples_per_pixel)
            .map(|channel| (channel, test_code_target_value(metadata, channel)))
            .collect();
    }
    metadata
        .channel_names
        .iter()
        .position(|name| name == &project.test_code.channel)
        .map(|channel| vec![(channel, test_code_target_value(metadata, channel))])
        .unwrap_or_default()
}

fn build_project_test_code_overlay(
    width: usize,
    height: usize,
    metadata: &TiffMetadata,
    project: &ShadeProject,
    dpi_info: DpiInfo,
) -> Result<Option<TextOverlay>, String> {
    if !project.test_code.enabled {
        return Ok(None);
    }
    let text = project.effective_test_code_text();
    if text.trim().is_empty() {
        return Ok(None);
    }
    let targets = test_code_targets(metadata, project);
    if targets.is_empty() {
        return Ok(None);
    }
    build_text_overlay(
        width,
        height,
        targets,
        &text,
        project.test_code.font_size_pt,
        project.test_code.margin_cm,
        project.test_code.position,
        dpi_info,
    )
    .map(Some)
}

fn build_text_overlay(
    width: usize,
    height: usize,
    targets: Vec<(usize, u16)>,
    text: &str,
    font_size_pt: f32,
    margin_cm: f32,
    position: TestCodePosition,
    dpi_info: DpiInfo,
) -> Result<TextOverlay, String> {
    let font_path = find_windows_font("Tahoma")?;
    let bytes = fs::read(&font_path)
        .map_err(|err| format!("Cannot read {}: {err}", font_path.display()))?;
    let font = Font::from_bytes(bytes, FontSettings::default())
        .map_err(|err| format!("Cannot parse Tahoma font: {err}"))?;
    let px = dpi::pixels_for_points(font_size_pt, dpi_info.dpi_y).max(4.0);
    let bitmap = rasterize_text(&font, text, px);
    let margin_x = dpi::pixels_for_cm(margin_cm, dpi_info.dpi_x);
    let margin_y = dpi::pixels_for_cm(margin_cm, dpi_info.dpi_y);
    let x0 = match position {
        TestCodePosition::TopLeft | TestCodePosition::BottomLeft => margin_x,
        TestCodePosition::TopRight | TestCodePosition::BottomRight => {
            width.saturating_sub(margin_x.saturating_add(bitmap.width))
        }
    };
    let y0 = match position {
        TestCodePosition::TopLeft | TestCodePosition::TopRight => margin_y,
        TestCodePosition::BottomLeft | TestCodePosition::BottomRight => {
            height.saturating_sub(margin_y.saturating_add(bitmap.height))
        }
    };
    Ok(TextOverlay {
        x0,
        y0,
        targets,
        bitmap,
    })
}

fn apply_text_overlay_to_region(
    samples: &mut [u16],
    region_x: usize,
    region_y: usize,
    region_width: usize,
    region_height: usize,
    channels: usize,
    overlay: &TextOverlay,
) {
    if overlay.bitmap.width == 0 || overlay.bitmap.height == 0 {
        return;
    }
    let region_x1 = region_x.saturating_add(region_width);
    let region_y1 = region_y.saturating_add(region_height);
    let text_x1 = overlay.x0.saturating_add(overlay.bitmap.width);
    let text_y1 = overlay.y0.saturating_add(overlay.bitmap.height);
    let x_begin = region_x.max(overlay.x0);
    let x_end = region_x1.min(text_x1);
    let y_begin = region_y.max(overlay.y0);
    let y_end = region_y1.min(text_y1);
    if x_begin >= x_end || y_begin >= y_end {
        return;
    }

    for image_y in y_begin..y_end {
        let bitmap_y = image_y - overlay.y0;
        let local_y = image_y - region_y;
        for image_x in x_begin..x_end {
            let bitmap_x = image_x - overlay.x0;
            let alpha = overlay.bitmap.alpha[bitmap_y * overlay.bitmap.width + bitmap_x];
            if alpha == 0 {
                continue;
            }
            let local_x = image_x - region_x;
            for &(target_channel, target_value) in &overlay.targets {
                let index = (local_y * region_width + local_x) * channels + target_channel;
                if index >= samples.len() {
                    continue;
                }
                let a = f32::from(alpha) / 255.0;
                let current = samples[index] as f32;
                samples[index] = (current * (1.0 - a) + target_value as f32 * a).round() as u16;
            }
        }
    }
}

fn apply_text_overlay_to_rows(
    samples: &mut [u16],
    row_start: usize,
    row_count: usize,
    width: usize,
    channels: usize,
    overlay: &TextOverlay,
) {
    if overlay.bitmap.width == 0 || overlay.bitmap.height == 0 {
        return;
    }
    let row_end = row_start.saturating_add(row_count);
    let text_end = overlay.y0.saturating_add(overlay.bitmap.height);
    let y_begin = row_start.max(overlay.y0);
    let y_end = row_end.min(text_end);
    if y_begin >= y_end {
        return;
    }
    for image_y in y_begin..y_end {
        let bitmap_y = image_y - overlay.y0;
        let local_y = image_y - row_start;
        for bx in 0..overlay.bitmap.width {
            let alpha = overlay.bitmap.alpha[bitmap_y * overlay.bitmap.width + bx];
            if alpha == 0 {
                continue;
            }
            let x = overlay.x0 + bx;
            if x >= width {
                continue;
            }
            for &(target_channel, target_value) in &overlay.targets {
                let index = (local_y * width + x) * channels + target_channel;
                if index >= samples.len() {
                    continue;
                }
                let a = f32::from(alpha) / 255.0;
                let current = samples[index] as f32;
                samples[index] = (current * (1.0 - a) + target_value as f32 * a).round() as u16;
            }
        }
    }
}

struct TextBitmap {
    width: usize,
    height: usize,
    alpha: Vec<u8>,
}

fn rasterize_text(font: &Font, text: &str, px: f32) -> TextBitmap {
    let lines = text.split('\n').collect::<Vec<_>>();
    let line_height = (px * 1.28).ceil().max(1.0) as usize;
    let mut widths = Vec::with_capacity(lines.len());
    for line in &lines {
        let mut pen = 0.0f32;
        let mut previous = None;
        for ch in line.chars() {
            if let Some(prev) = previous {
                pen += font.horizontal_kern(prev, ch, px).unwrap_or(0.0);
            }
            pen += font.metrics(ch, px).advance_width;
            previous = Some(ch);
        }
        widths.push(pen.ceil().max(0.0) as usize);
    }
    let width = widths.into_iter().max().unwrap_or(0).saturating_add(4);
    let height = lines
        .len()
        .max(1)
        .saturating_mul(line_height)
        .saturating_add(4);
    let mut alpha = vec![0u8; width.saturating_mul(height)];

    for (line_index, line) in lines.iter().enumerate() {
        let mut pen = 2.0f32;
        let mut previous = None;
        for ch in line.chars() {
            if let Some(prev) = previous {
                pen += font.horizontal_kern(prev, ch, px).unwrap_or(0.0);
            }
            let (metrics, glyph) = font.rasterize(ch, px);
            let gx = (pen + metrics.xmin as f32).round() as isize;
            let baseline_bottom =
                (line_index * line_height + line_height).saturating_sub(2) as isize;
            let gy = baseline_bottom - metrics.height as isize;
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let x = gx + col as isize;
                    let y = gy + row as isize;
                    if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
                        continue;
                    }
                    let source = glyph[row * metrics.width + col];
                    let index = y as usize * width + x as usize;
                    alpha[index] = alpha[index].max(source);
                }
            }
            pen += metrics.advance_width;
            previous = Some(ch);
        }
    }

    TextBitmap {
        width,
        height,
        alpha,
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use tiff::encoder::{Compression, TiffEncoder, colortype};
    use tiff::tags::ExtraSamples;

    fn test_metadata(
        names: &[String],
        base_channel_count: usize,
        channel_display_info: Vec<Option<crate::tiff_io::PhotoshopChannelDisplay>>,
    ) -> TiffMetadata {
        TiffMetadata {
            width: 1,
            height: 1,
            bit_depth: 16,
            samples_per_pixel: names.len(),
            base_channel_count,
            color_model: ColorModel::Cmyk,
            channel_names: names.to_vec(),
            channel_display_info,
            compression: None,
            predictor: None,
            orientation: None,
            icc_profile: None,
            photoshop_resources: None,
            photoshop_image_source_data: None,
        }
    }

    #[test]
    fn adjustment_pipeline_is_levels_then_mixer_then_curve() {
        let names = vec!["A".to_owned(), "B".to_owned()];
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);

        {
            let adjustment = project.adjustments.get_mut("A").unwrap();
            adjustment.levels.output_white = 0.5;
            adjustment.mixer.constant = 0.0;
            adjustment.mixer.coefficients.insert("A".to_owned(), 0.5);
            adjustment.mixer.coefficients.insert("B".to_owned(), 0.5);
            adjustment.curve.midpoint_enabled = true;
            adjustment.curve.midpoint_input = 0.5;
            adjustment.curve.midpoint = 0.1;
        }

        let input = [13_107u16, 52_428u16];
        let metadata = test_metadata(&names, 2, vec![None; 2]);
        let output = adjusted_strip(&input, &metadata, &project);

        let raw_a = input[0] as f32 / 65_535.0;
        let raw_b = input[1] as f32 / 65_535.0;
        let a = project.adjustments.get("A").unwrap();
        let b = project.adjustments.get("B").unwrap();
        let leveled_a = apply_levels(raw_a, a.levels);
        let leveled_b = apply_levels(raw_b, b.levels);
        let mixed = a.mixer.constant + leveled_a * 0.5 + leveled_b * 0.5;
        let expected = apply_curve(mixed, a.curve);
        let actual = output[0] as f32 / 65_535.0;
        assert!((actual - expected).abs() <= 2.0 / 65_535.0);

        let legacy = apply_curve(leveled_a, a.curve) * 0.5 + leveled_b * 0.5;
        assert!((actual - legacy).abs() > 0.20);
    }

    #[test]
    fn gray_adjustment_pipeline_preserves_single_channel_semantics() {
        let names = vec!["Gray".to_owned()];
        let mut metadata = test_metadata(&names, 1, vec![None]);
        metadata.color_model = ColorModel::Gray;
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);
        project
            .adjustments
            .get_mut("Gray")
            .unwrap()
            .levels
            .output_white = 0.5;
        let input = [32_768u16];
        let output = adjusted_strip(&input, &metadata, &project);
        assert_eq!(output.len(), 1);
        assert!(output[0] < input[0]);
    }

    #[test]
    fn spot_zero_working_coverage_exports_as_no_ink_with_photoshop_polarity() {
        let names = vec![
            "Cyan".to_owned(),
            "Magenta".to_owned(),
            "Yellow".to_owned(),
            "Black".to_owned(),
            "Spot Red".to_owned(),
        ];
        let mut display = vec![None; 5];
        display[4] = Some(crate::tiff_io::PhotoshopChannelDisplay {
            rgb: Some([0.9, 0.2, 0.1]),
            solidity: 1.0,
            kind: 2,
        });
        let metadata = test_metadata(&names, 4, display);
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);
        project
            .adjustments
            .get_mut("Yellow")
            .unwrap()
            .levels
            .output_white = 0.0;
        project
            .adjustments
            .get_mut("Spot Red")
            .unwrap()
            .levels
            .output_white = 0.0;

        // Equivalent 50% ink: CMYK raw uses direct coverage; Photoshop Spot raw is inverted.
        let input = [0u16, 0, 32_768, 0, u16::MAX - 32_768];
        let output = adjusted_strip(&input, &metadata, &project);

        assert_eq!(
            output[2], 0,
            "Yellow 0 working coverage must export as no ink"
        );
        if crate::tiff_io::NORMALIZE_PHOTOSHOP_SPOT_POLARITY {
            assert_eq!(
                output[4],
                u16::MAX,
                "Spot 0 working coverage must be restored to Photoshop's no-ink raw value"
            );
        } else {
            assert_eq!(
                output[4], 0,
                "legacy Spot polarity must remain available when disabled"
            );
        }
    }

    fn apply_dynamic_u8_predictor(data: &mut [u8], width: usize, height: usize, channels: usize) {
        let row_samples = width * channels;
        assert_eq!(data.len(), row_samples * height);
        for row in 0..height {
            let start = row * row_samples;
            for x in (1..width).rev() {
                for channel in 0..channels {
                    let index = start + x * channels + channel;
                    let previous = start + (x - 1) * channels + channel;
                    data[index] = data[index].wrapping_sub(data[previous]);
                }
            }
        }
    }

    #[test]
    fn streaming_identity_export_preserves_six_channels() {
        let unique = format!(
            "shade-stream-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let source = std::env::temp_dir().join(format!("{unique}-source.tif"));
        let destination = std::env::temp_dir().join(format!("{unique}-export.tif"));
        let pixels = vec![
            1u8, 2, 3, 4, 5, 6, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150,
            160, 170, 180,
        ];

        {
            let file = File::create(&source).unwrap();
            let mut tiff = TiffEncoder::new(BufWriter::new(file))
                .unwrap()
                .with_compression(Compression::Lzw);
            let mut image = tiff.new_image::<colortype::CMYK8>(2, 2).unwrap();
            image
                .extra_samples(&[ExtraSamples::Unspecified, ExtraSamples::Unspecified])
                .unwrap();
            // Build a valid six-channel Predictor=2 source without relying on
            // image-tiff's base-CMYK predictor stride. The decoder must restore
            // the original samples using SamplesPerPixel=6.
            image.encoder().write_tag(Tag::Predictor, 2u16).unwrap();
            image.rows_per_strip(1).unwrap();
            let mut predicted = pixels.clone();
            apply_dynamic_u8_predictor(&mut predicted, 2, 2, 6);
            image.write_data(&predicted).unwrap();
        }

        let info = stream_info(&source).unwrap();
        assert!(info.streamable);
        assert_eq!(info.metadata.samples_per_pixel, 6);
        assert_eq!(info.metadata.compression, Some(5));
        assert_eq!(info.metadata.predictor, Some(2));
        let decoded_source = decode_full(&source).unwrap();
        let mut project = ShadeProject::default();
        project.ensure_channels(&decoded_source.metadata.channel_names);

        std::fs::write(&destination, b"stale partial export").unwrap();
        export_face_with_progress(&source, &destination, &project, 220.0, |_, _| {}).unwrap();

        let decoded_output = decode_full(&destination).unwrap();
        assert_eq!(decoded_output.metadata.samples_per_pixel, 6);
        assert_eq!(decoded_output.metadata.color_model, ColorModel::Cmyk);
        assert_eq!(decoded_output.metadata.compression, Some(5));
        // Predictor is intentionally normalized off for extra-channel TIFFs;
        // decoded pixel/separation data must still be exactly identical.
        assert_ne!(decoded_output.metadata.predictor, Some(2));
        assert_eq!(decoded_output.samples, decoded_source.samples);

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(destination);
    }

    #[test]
    fn identity_export_preserves_bigtiff_container() {
        let unique = format!(
            "shade-bigtiff-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let source = std::env::temp_dir().join(format!("{unique}-source.tif"));
        let destination = std::env::temp_dir().join(format!("{unique}-export.tif"));
        let pixels = vec![
            1u8, 2, 3, 4, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120,
        ];
        {
            let file = File::create(&source).unwrap();
            let mut tiff = TiffEncoder::new_big(BufWriter::new(file))
                .unwrap()
                .with_compression(Compression::Lzw);
            let mut image = tiff.new_image::<colortype::CMYK8>(2, 2).unwrap();
            image.write_data(&pixels).unwrap();
        }
        assert!(source_is_bigtiff(&source).unwrap());
        let decoded_source = decode_full(&source).unwrap();
        let mut project = ShadeProject::default();
        project.ensure_channels(&decoded_source.metadata.channel_names);
        export_face_with_progress(&source, &destination, &project, 220.0, |_, _| {}).unwrap();
        assert!(source_is_bigtiff(&destination).unwrap());
        let decoded_output = decode_full(&destination).unwrap();
        assert_eq!(decoded_output.samples, decoded_source.samples);
        assert_eq!(decoded_output.metadata.color_model, ColorModel::Cmyk);
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(destination);
    }

    #[test]
    fn large_layout_selects_bigtiff_without_allocating_pixels() {
        assert!(!layout_requires_bigtiff_values(720, 1280, 6, 8));
        assert!(!layout_requires_bigtiff_values(20_000, 20_000, 4, 8));
        assert!(layout_requires_bigtiff_values(40_000, 40_000, 4, 8));
        assert!(layout_requires_bigtiff_values(30_000, 30_000, 4, 16));
    }
}
