use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use font8x8::{BASIC_FONTS, UnicodeFonts};
use tiff::encoder::{TiffEncoder, colortype};
use tiff::tags::{ExtraSamples, Tag};

use crate::model::{ShadeProject, apply_curve, apply_levels};
use crate::tiff_io::{ColorModel, TiffMetadata, decode_full};

pub fn export_face(source: &Path, destination: &Path, project: &ShadeProject) -> Result<(), String> {
    let decoded = decode_full(source)?;
    let channels = decoded.metadata.samples_per_pixel;
    let base_channels = decoded.metadata.base_channel_count;
    if channels == 0 || channels < base_channels {
        return Err("Invalid TIFF channel layout.".to_owned());
    }
    if !matches!(decoded.metadata.color_model, ColorModel::Rgb | ColorModel::Cmyk) {
        return Err(format!(
            "Export currently supports RGB and CMYK Photoshop TIFF; this file is {}.",
            decoded.metadata.color_model.title()
        ));
    }

    let width = decoded.metadata.width as usize;
    let height = decoded.metadata.height as usize;
    let pixel_count = width.checked_mul(height).ok_or_else(|| "Image is too large.".to_owned())?;
    let expected = pixel_count
        .checked_mul(channels)
        .ok_or_else(|| "Image sample count is too large.".to_owned())?;
    if decoded.samples.len() < expected {
        return Err("Decoded TIFF sample buffer is incomplete.".to_owned());
    }

    let names = &decoded.metadata.channel_names;
    let mut output = vec![0u16; expected];
    let mut prepared = vec![0.0f32; channels];

    for pixel in 0..pixel_count {
        let base = pixel * channels;
        for channel in 0..channels {
            let raw = decoded.samples[base + channel] as f32 / 65535.0;
            prepared[channel] = match project.adjustments.get(&names[channel]) {
                Some(adjustment) if adjustment.enabled => {
                    apply_curve(apply_levels(raw, adjustment.levels), adjustment.curve)
                }
                _ => raw,
            };
        }

        for out_channel in 0..channels {
            let value = match project.adjustments.get(&names[out_channel]) {
                Some(adjustment) if adjustment.enabled => {
                    let mut mixed = adjustment.mixer.constant;
                    for source_channel in 0..channels {
                        let coefficient = adjustment.mixer.coefficients
                            .get(&names[source_channel])
                            .copied()
                            .unwrap_or(if source_channel == out_channel { 1.0 } else { 0.0 });
                        mixed += prepared[source_channel] * coefficient;
                    }
                    mixed
                }
                _ => prepared[out_channel],
            };
            output[base + out_channel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }

    if project.test_code.enabled && !project.test_code.text.trim().is_empty() {
        if let Some(channel) = names.iter().position(|name| name == &project.test_code.channel) {
            let target_value = if channel >= base_channels {
                // Photoshop spot/alpha channel convention: dark pixels represent
                // the visible/selected separation.
                0
            } else {
                match decoded.metadata.color_model {
                    // Separated TIFF is ink coverage: max adds ink.
                    ColorModel::Cmyk => u16::MAX,
                    // RGB test text should be black in the selected component.
                    _ => 0,
                }
            };
            draw_test_code(
                &mut output,
                width,
                height,
                channels,
                channel,
                target_value,
                &project.test_code.text,
                project.test_code.scale.max(1) as usize,
                project.test_code.margin_px as usize,
            );
        }
    }

    let file = File::create(destination).map_err(|err| format!("Cannot create export TIFF: {err}"))?;
    let writer = BufWriter::new(file);
    let mut encoder = TiffEncoder::new(writer).map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;

    match (decoded.metadata.color_model, decoded.metadata.bit_depth) {
        (ColorModel::Rgb, 8) => {
            let data = output.into_iter().map(|value| (value >> 8) as u8).collect::<Vec<_>>();
            let mut image = encoder
                .new_image::<colortype::RGB8>(decoded.metadata.width, decoded.metadata.height)
                .map_err(|err| format!("Cannot create RGB 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, &decoded.metadata)?;
            image.write_data(&data).map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Rgb, 16) => {
            let mut image = encoder
                .new_image::<colortype::RGB16>(decoded.metadata.width, decoded.metadata.height)
                .map_err(|err| format!("Cannot create RGB 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, &decoded.metadata)?;
            image.write_data(&output).map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Cmyk, 8) => {
            let data = output.into_iter().map(|value| (value >> 8) as u8).collect::<Vec<_>>();
            let mut image = encoder
                .new_image::<colortype::CMYK8>(decoded.metadata.width, decoded.metadata.height)
                .map_err(|err| format!("Cannot create CMYK 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, &decoded.metadata)?;
            image.write_data(&data).map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Cmyk, 16) => {
            let mut image = encoder
                .new_image::<colortype::CMYK16>(decoded.metadata.width, decoded.metadata.height)
                .map_err(|err| format!("Cannot create CMYK 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, &decoded.metadata)?;
            image.write_data(&output).map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (_, depth) => return Err(format!("Unsupported export bit depth/color model: {depth}-bit.")),
    }

    Ok(())
}

fn configure_extras_and_metadata<W, C, K>(
    image: &mut tiff::encoder::ImageEncoder<'_, W, C, K>,
    channels: usize,
    base_channels: usize,
    metadata: &TiffMetadata,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    C: tiff::encoder::colortype::ColorType,
    K: tiff::encoder::TiffKind,
{
    let extra_count = channels.saturating_sub(base_channels);
    if extra_count > 0 {
        // Photoshop documents its TIFF reader as using ExtraSamples for the
        // count; spot/alpha identity, names and display information live in the
        // Photoshop Image Resources copied below.
        let extras = (0..extra_count).map(|_| ExtraSamples::Unspecified).collect::<Vec<_>>();
        image.extra_samples(&extras).map_err(|err| format!("Cannot configure extra/spot channels: {err}"))?;
    }
    if let Some(profile) = &metadata.icc_profile {
        image.encoder().write_tag(Tag::IccProfile, profile.as_slice())
            .map_err(|err| format!("Cannot preserve ICC profile: {err}"))?;
    }
    if let Some(resources) = &metadata.photoshop_resources {
        image.encoder().write_tag(Tag::Unknown(34377), resources.as_slice())
            .map_err(|err| format!("Cannot preserve Photoshop Image Resources: {err}"))?;
    }
    if let Some(source_data) = &metadata.photoshop_image_source_data {
        image.encoder().write_tag(Tag::Unknown(37724), source_data.as_slice())
            .map_err(|err| format!("Cannot preserve Photoshop ImageSourceData: {err}"))?;
    }
    image.encoder().write_tag(Tag::Software, "Shade Editor")
        .map_err(|err| format!("Cannot write TIFF software tag: {err}"))?;
    Ok(())
}

fn draw_test_code(
    samples: &mut [u16],
    width: usize,
    height: usize,
    channels: usize,
    target_channel: usize,
    target_value: u16,
    text: &str,
    scale: usize,
    margin: usize,
) {
    let chars = text.chars().map(|ch| if ch.is_ascii() { ch } else { '?' }).collect::<Vec<_>>();
    if chars.is_empty() { return; }
    let glyph_width = 8 * scale;
    let spacing = scale;
    let text_width = chars.len().saturating_mul(glyph_width + spacing).saturating_sub(spacing);
    let text_height = 8 * scale;
    let origin_x = width.saturating_sub(margin.saturating_add(text_width));
    let origin_y = height.saturating_sub(margin.saturating_add(text_height));

    for (char_index, ch) in chars.into_iter().enumerate() {
        let glyph = BASIC_FONTS.get(ch).or_else(|| BASIC_FONTS.get('?'));
        let Some(glyph) = glyph else { continue; };
        for (row, row_bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if row_bits & (1 << col) == 0 { continue; }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let x = origin_x + char_index * (glyph_width + spacing) + col * scale + sx;
                        let y = origin_y + row * scale + sy;
                        if x >= width || y >= height { continue; }
                        let index = (y * width + x) * channels + target_channel;
                        if index < samples.len() {
                            samples[index] = target_value;
                        }
                    }
                }
            }
        }
    }
}
