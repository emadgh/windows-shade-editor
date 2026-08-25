use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::{Duration, Instant};

use tiff::encoder::{colortype, Compression, DirectoryEncoder, Predictor, TiffEncoder, TiffKind};
use tiff::tags::{ExtraSamples, PhotometricInterpretation, SampleFormat, Tag};

use crate::conversion_tiff::lzw_strip_writer::LzwStripWriter;
use crate::dpi::{self, DpiInfo};
use crate::safe_fs::tiff_performance::{self, TiffPerfPhase};
use crate::tiff_io::{ColorModel, TiffMetadata};
use crate::tiff_output::{self, TiffLayout};

const TIFF_ENCODER_BUFFER_BYTES: usize = 1024 * 1024;
const TIFF_COMPRESSION_LZW: u16 = 5;
const TIFF_PREDICTOR_NONE: u16 = 1;
const TIFF_PREDICTOR_HORIZONTAL: u16 = 2;

/// Pixel storage supplied by a renderer to the shared RGB/CMYK/Gray writer.
#[derive(Clone, Copy)]
pub enum OutputPixels<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
}

/// Write a source-topology TIFF using the source container policy and metadata
/// conventions shared by normal Export and Test Stack.
pub fn write_tiff_pixels(
    source: &Path,
    destination: &Path,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    force_lzw: bool,
    rows_per_strip: Option<u32>,
    pixels: OutputPixels<'_>,
) -> Result<(), String> {
    let layout = TiffLayout {
        width: metadata.width,
        height: metadata.height,
        channels: metadata.samples_per_pixel,
        bit_depth: metadata.bit_depth,
    };
    let logical_bytes = tiff_output::raw_image_bytes(layout);
    let encode_started = Instant::now();

    let result = (|| {
        let file =
            File::create(destination).map_err(|err| format!("Cannot create export TIFF: {err}"))?;
        let writer = BufWriter::with_capacity(TIFF_ENCODER_BUFFER_BYTES, file);
        if should_write_bigtiff(source, metadata)? {
            let encoder = TiffEncoder::new_big(writer)
                .map_err(|err| format!("Cannot initialize BigTIFF encoder: {err}"))?;
            let mut encoder = configure_tiff_encoder(encoder, metadata, force_lzw);
            write_tiff_with_encoder(&mut encoder, metadata, dpi_info, rows_per_strip, pixels)
        } else {
            let encoder = TiffEncoder::new(writer)
                .map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;
            let mut encoder = configure_tiff_encoder(encoder, metadata, force_lzw);
            write_tiff_with_encoder(&mut encoder, metadata, dpi_info, rows_per_strip, pixels)
        }
    })();

    tiff_performance::emit_phase_if_enabled(
        "source_tiff_writer",
        TiffPerfPhase::CompressionEncode,
        encode_started.elapsed(),
        logical_bytes,
    );
    result
}

/// Directly publish adjusted row strips into an LZW TIFF without first materializing a full raw
/// raster. The producer must emit monotonically increasing, complete row ranges using the supplied
/// sink. This preserves the same source-topology metadata as `write_tiff_pixels` while bounding
/// intermediate memory to one decoded/adjusted strip plus the shared LZW scratch.
pub fn write_tiff_lzw_strips_u8<F>(
    source: &Path,
    destination: &Path,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    rows_per_strip: u32,
    produce: F,
) -> Result<(), String>
where
    F: FnOnce(&mut dyn FnMut(u32, u32, &[u8]) -> Result<(), String>) -> Result<(), String>,
{
    if metadata.bit_depth != 8 {
        return Err("8-bit streaming TIFF writer received non-8-bit metadata.".to_owned());
    }
    let logical_bytes = tiff_output::raw_image_bytes(TiffLayout {
        width: metadata.width,
        height: metadata.height,
        channels: metadata.samples_per_pixel,
        bit_depth: metadata.bit_depth,
    });
    let mut compression_elapsed = Duration::ZERO;
    let result = (|| {
        let file =
            File::create(destination).map_err(|err| format!("Cannot create export TIFF: {err}"))?;
        let writer = BufWriter::with_capacity(TIFF_ENCODER_BUFFER_BYTES, file);
        if should_write_bigtiff(source, metadata)? {
            let encoder = TiffEncoder::new_big(writer)
                .map_err(|err| format!("Cannot initialize BigTIFF encoder: {err}"))?;
            write_lzw_u8_with_encoder(
                encoder,
                metadata,
                dpi_info,
                rows_per_strip,
                produce,
                &mut compression_elapsed,
            )
        } else {
            let encoder = TiffEncoder::new(writer)
                .map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;
            write_lzw_u8_with_encoder(
                encoder,
                metadata,
                dpi_info,
                rows_per_strip,
                produce,
                &mut compression_elapsed,
            )
        }
    })();
    tiff_performance::emit_phase_if_enabled(
        "source_tiff_writer_streaming",
        TiffPerfPhase::CompressionEncode,
        compression_elapsed,
        logical_bytes,
    );
    result
}

pub fn write_tiff_lzw_strips_u16<F>(
    source: &Path,
    destination: &Path,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    rows_per_strip: u32,
    produce: F,
) -> Result<(), String>
where
    F: FnOnce(&mut dyn FnMut(u32, u32, &[u16]) -> Result<(), String>) -> Result<(), String>,
{
    if metadata.bit_depth != 16 {
        return Err("16-bit streaming TIFF writer received non-16-bit metadata.".to_owned());
    }
    let logical_bytes = tiff_output::raw_image_bytes(TiffLayout {
        width: metadata.width,
        height: metadata.height,
        channels: metadata.samples_per_pixel,
        bit_depth: metadata.bit_depth,
    });
    let mut compression_elapsed = Duration::ZERO;
    let result = (|| {
        let file =
            File::create(destination).map_err(|err| format!("Cannot create export TIFF: {err}"))?;
        let writer = BufWriter::with_capacity(TIFF_ENCODER_BUFFER_BYTES, file);
        if should_write_bigtiff(source, metadata)? {
            let encoder = TiffEncoder::new_big(writer)
                .map_err(|err| format!("Cannot initialize BigTIFF encoder: {err}"))?;
            write_lzw_u16_with_encoder(
                encoder,
                metadata,
                dpi_info,
                rows_per_strip,
                produce,
                &mut compression_elapsed,
            )
        } else {
            let encoder = TiffEncoder::new(writer)
                .map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;
            write_lzw_u16_with_encoder(
                encoder,
                metadata,
                dpi_info,
                rows_per_strip,
                produce,
                &mut compression_elapsed,
            )
        }
    })();
    tiff_performance::emit_phase_if_enabled(
        "source_tiff_writer_streaming",
        TiffPerfPhase::CompressionEncode,
        compression_elapsed,
        logical_bytes,
    );
    result
}

fn write_lzw_u8_with_encoder<W, K, F>(
    mut encoder: TiffEncoder<W, K>,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    rows_per_strip: u32,
    produce: F,
    compression_elapsed: &mut Duration,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    K: TiffKind,
    F: FnOnce(&mut dyn FnMut(u32, u32, &[u8]) -> Result<(), String>) -> Result<(), String>,
{
    let rows_per_strip = rows_per_strip.min(metadata.height).max(1);
    let mut directory = encoder
        .image_directory()
        .map_err(|err| format!("Cannot create streaming TIFF directory: {err}"))?;
    configure_lzw_streaming_directory(&mut directory, metadata, dpi_info, rows_per_strip)?;
    let mut writer = LzwStripWriter::new(directory);
    let row_samples = checked_row_samples(metadata)?;
    let horizontal = uses_horizontal_predictor(metadata);
    let channels = metadata.samples_per_pixel;
    let mut predicted = Vec::<u8>::new();
    let mut next_row = 0u32;

    let mut sink = |start_row: u32, row_count: u32, samples: &[u8]| -> Result<(), String> {
        validate_emitted_strip(
            metadata,
            rows_per_strip,
            row_samples,
            next_row,
            start_row,
            row_count,
            samples.len(),
        )?;
        if horizontal {
            horizontal_predict_u8(samples, row_samples, channels, &mut predicted)?;
            *compression_elapsed += writer.write_u8_strip(&predicted)?;
        } else {
            *compression_elapsed += writer.write_u8_strip(samples)?;
        }
        next_row = next_row
            .checked_add(row_count)
            .ok_or_else(|| "Streaming TIFF row cursor overflow.".to_owned())?;
        Ok(())
    };
    produce(&mut sink)?;
    if next_row != metadata.height {
        return Err(format!(
            "Streaming TIFF producer emitted {next_row} rows; expected {}.",
            metadata.height
        ));
    }
    writer.finish()
}

fn write_lzw_u16_with_encoder<W, K, F>(
    mut encoder: TiffEncoder<W, K>,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    rows_per_strip: u32,
    produce: F,
    compression_elapsed: &mut Duration,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    K: TiffKind,
    F: FnOnce(&mut dyn FnMut(u32, u32, &[u16]) -> Result<(), String>) -> Result<(), String>,
{
    let rows_per_strip = rows_per_strip.min(metadata.height).max(1);
    let mut directory = encoder
        .image_directory()
        .map_err(|err| format!("Cannot create streaming TIFF directory: {err}"))?;
    configure_lzw_streaming_directory(&mut directory, metadata, dpi_info, rows_per_strip)?;
    let mut writer = LzwStripWriter::new(directory);
    let row_samples = checked_row_samples(metadata)?;
    let horizontal = uses_horizontal_predictor(metadata);
    let channels = metadata.samples_per_pixel;
    let mut predicted = Vec::<u16>::new();
    let mut next_row = 0u32;

    let mut sink = |start_row: u32, row_count: u32, samples: &[u16]| -> Result<(), String> {
        validate_emitted_strip(
            metadata,
            rows_per_strip,
            row_samples,
            next_row,
            start_row,
            row_count,
            samples.len(),
        )?;
        if horizontal {
            horizontal_predict_u16(samples, row_samples, channels, &mut predicted)?;
            *compression_elapsed += writer.write_u16_strip(&predicted)?;
        } else {
            *compression_elapsed += writer.write_u16_strip(samples)?;
        }
        next_row = next_row
            .checked_add(row_count)
            .ok_or_else(|| "Streaming TIFF row cursor overflow.".to_owned())?;
        Ok(())
    };
    produce(&mut sink)?;
    if next_row != metadata.height {
        return Err(format!(
            "Streaming TIFF producer emitted {next_row} rows; expected {}.",
            metadata.height
        ));
    }
    writer.finish()
}

fn configure_lzw_streaming_directory<W, K>(
    directory: &mut DirectoryEncoder<'_, W, K>,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    rows_per_strip: u32,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    K: TiffKind,
{
    let channels = metadata.samples_per_pixel;
    let base_channels = base_channel_count_for_model(metadata.color_model)?;
    if channels < base_channels || metadata.base_channel_count != base_channels {
        return Err("Invalid source-topology TIFF channel layout.".to_owned());
    }
    let photometric = match metadata.color_model {
        ColorModel::Rgb => PhotometricInterpretation::RGB,
        ColorModel::Cmyk => PhotometricInterpretation::CMYK,
        ColorModel::Gray => PhotometricInterpretation::BlackIsZero,
        _ => return Err("Streaming source TIFF supports RGB, CMYK and Gray only.".to_owned()),
    };
    let bits_per_sample = vec![u16::from(metadata.bit_depth); channels];
    let sample_format = vec![SampleFormat::Uint; channels];
    let predictor = if uses_horizontal_predictor(metadata) {
        TIFF_PREDICTOR_HORIZONTAL
    } else {
        TIFF_PREDICTOR_NONE
    };

    directory
        .write_tag(Tag::ImageWidth, metadata.width)
        .map_err(|err| format!("Cannot write streaming ImageWidth: {err}"))?;
    directory
        .write_tag(Tag::ImageLength, metadata.height)
        .map_err(|err| format!("Cannot write streaming ImageLength: {err}"))?;
    directory
        .write_tag(Tag::Compression, TIFF_COMPRESSION_LZW)
        .map_err(|err| format!("Cannot write streaming Compression: {err}"))?;
    directory
        .write_tag(Tag::Predictor, predictor)
        .map_err(|err| format!("Cannot write streaming Predictor: {err}"))?;
    directory
        .write_tag(Tag::PhotometricInterpretation, photometric)
        .map_err(|err| format!("Cannot write streaming photometric interpretation: {err}"))?;
    directory
        .write_tag(Tag::RowsPerStrip, rows_per_strip)
        .map_err(|err| format!("Cannot write streaming RowsPerStrip: {err}"))?;
    directory
        .write_tag(Tag::SamplesPerPixel, channels as u16)
        .map_err(|err| format!("Cannot write streaming SamplesPerPixel: {err}"))?;
    directory
        .write_tag(Tag::BitsPerSample, bits_per_sample.as_slice())
        .map_err(|err| format!("Cannot write streaming BitsPerSample: {err}"))?;
    directory
        .write_tag(Tag::SampleFormat, sample_format.as_slice())
        .map_err(|err| format!("Cannot write streaming SampleFormat: {err}"))?;

    let extra_count = channels.saturating_sub(base_channels);
    if extra_count > 0 {
        let extras = (0..extra_count)
            .map(|_| ExtraSamples::Unspecified)
            .collect::<Vec<_>>();
        directory
            .write_tag(Tag::ExtraSamples, extras.as_slice())
            .map_err(|err| format!("Cannot configure streaming extra/spot channels: {err}"))?;
    }

    let (resolution_x, resolution_y, resolution_unit) = dpi_info.effective_tiff_resolution();
    directory
        .write_tag(Tag::XResolution, dpi::rational(resolution_x))
        .map_err(|err| format!("Cannot write streaming XResolution: {err}"))?;
    directory
        .write_tag(Tag::YResolution, dpi::rational(resolution_y))
        .map_err(|err| format!("Cannot write streaming YResolution: {err}"))?;
    directory
        .write_tag(Tag::ResolutionUnit, resolution_unit)
        .map_err(|err| format!("Cannot preserve/write streaming resolution unit: {err}"))?;

    if let Some(orientation) = metadata.orientation {
        directory
            .write_tag(Tag::Orientation, orientation)
            .map_err(|err| format!("Cannot preserve streaming TIFF orientation: {err}"))?;
    }
    if let Some(profile) = &metadata.icc_profile {
        directory
            .write_tag(Tag::IccProfile, profile.as_slice())
            .map_err(|err| format!("Cannot preserve streaming ICC profile: {err}"))?;
    }
    if let Some(resources) = &metadata.photoshop_resources {
        directory
            .write_tag(Tag::Unknown(34377), resources.as_slice())
            .map_err(|err| format!("Cannot preserve streaming Photoshop Image Resources: {err}"))?;
    }
    if let Some(source_data) = &metadata.photoshop_image_source_data {
        directory
            .write_tag(Tag::Unknown(37724), source_data.as_slice())
            .map_err(|err| format!("Cannot preserve streaming Photoshop ImageSourceData: {err}"))?;
    }
    directory
        .write_tag(Tag::Software, "Shade Editor")
        .map_err(|err| format!("Cannot write streaming TIFF software tag: {err}"))?;
    Ok(())
}

fn validate_emitted_strip(
    metadata: &TiffMetadata,
    rows_per_strip: u32,
    row_samples: usize,
    expected_start_row: u32,
    start_row: u32,
    row_count: u32,
    sample_count: usize,
) -> Result<(), String> {
    if start_row != expected_start_row {
        return Err(format!(
            "Streaming TIFF producer emitted row {start_row}; expected {expected_start_row}."
        ));
    }
    if row_count == 0 || start_row >= metadata.height {
        return Err("Streaming TIFF producer emitted an invalid row range.".to_owned());
    }
    let remaining = metadata.height - start_row;
    let expected_rows = rows_per_strip.min(remaining);
    if row_count != expected_rows {
        return Err(format!(
            "Streaming TIFF producer emitted {row_count} rows at {start_row}; expected {expected_rows}."
        ));
    }
    let expected_samples = usize::try_from(row_count)
        .ok()
        .and_then(|rows| rows.checked_mul(row_samples))
        .ok_or_else(|| "Streaming TIFF strip sample count overflow.".to_owned())?;
    if sample_count != expected_samples {
        return Err(format!(
            "Streaming TIFF strip contains {sample_count} samples; expected {expected_samples}."
        ));
    }
    Ok(())
}

fn horizontal_predict_u8(
    samples: &[u8],
    row_samples: usize,
    channels: usize,
    output: &mut Vec<u8>,
) -> Result<(), String> {
    if row_samples == 0 || channels == 0 || row_samples < channels || samples.len() % row_samples != 0
    {
        return Err("Invalid 8-bit horizontal predictor row layout.".to_owned());
    }
    output.clear();
    output.reserve(samples.len());
    for row in samples.chunks_exact(row_samples) {
        output.extend_from_slice(&row[..channels]);
        output.extend(
            row.iter()
                .copied()
                .zip(row[channels..].iter().copied())
                .map(|(previous, current)| current.wrapping_sub(previous)),
        );
    }
    Ok(())
}

fn horizontal_predict_u16(
    samples: &[u16],
    row_samples: usize,
    channels: usize,
    output: &mut Vec<u16>,
) -> Result<(), String> {
    if row_samples == 0 || channels == 0 || row_samples < channels || samples.len() % row_samples != 0
    {
        return Err("Invalid 16-bit horizontal predictor row layout.".to_owned());
    }
    output.clear();
    output.reserve(samples.len());
    for row in samples.chunks_exact(row_samples) {
        output.extend_from_slice(&row[..channels]);
        output.extend(
            row.iter()
                .copied()
                .zip(row[channels..].iter().copied())
                .map(|(previous, current)| current.wrapping_sub(previous)),
        );
    }
    Ok(())
}

fn uses_horizontal_predictor(metadata: &TiffMetadata) -> bool {
    metadata.predictor == Some(TIFF_PREDICTOR_HORIZONTAL)
        && metadata.samples_per_pixel == metadata.base_channel_count
}

fn checked_row_samples(metadata: &TiffMetadata) -> Result<usize, String> {
    usize::try_from(metadata.width)
        .ok()
        .and_then(|width| width.checked_mul(metadata.samples_per_pixel))
        .ok_or_else(|| "Streaming TIFF row sample count overflow.".to_owned())
}

fn base_channel_count_for_model(model: ColorModel) -> Result<usize, String> {
    match model {
        ColorModel::Rgb => Ok(3),
        ColorModel::Cmyk => Ok(4),
        ColorModel::Gray => Ok(1),
        _ => Err("Streaming source TIFF supports RGB, CMYK and Gray only.".to_owned()),
    }
}

fn should_write_bigtiff(source: &Path, metadata: &TiffMetadata) -> Result<bool, String> {
    tiff_output::preserve_source_or_layout_requires_bigtiff(
        source,
        TiffLayout {
            width: metadata.width,
            height: metadata.height,
            channels: metadata.samples_per_pixel,
            bit_depth: metadata.bit_depth,
        },
    )
}

fn configure_tiff_encoder<W, K>(
    mut encoder: TiffEncoder<W, K>,
    metadata: &TiffMetadata,
    force_lzw: bool,
) -> TiffEncoder<W, K>
where
    W: std::io::Write + std::io::Seek,
    K: tiff::encoder::TiffKind,
{
    let compression = if force_lzw {
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
