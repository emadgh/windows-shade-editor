use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::Instant;

use tiff::encoder::{colortype, Compression, Predictor, TiffEncoder};
use tiff::tags::{ExtraSamples, Tag};

use crate::dpi::{self, DpiInfo};
use crate::tiff_io::{ColorModel, TiffMetadata};
use crate::tiff_output::{self, TiffLayout};
use crate::tiff_performance::{self, TiffPerfPhase};

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
        let writer = BufWriter::new(file);
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
