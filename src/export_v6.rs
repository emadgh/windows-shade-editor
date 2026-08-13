use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use fontdue::{Font, FontSettings};
use tiff::encoder::{TiffEncoder, colortype};
use tiff::tags::{ExtraSamples, Tag};

use crate::dpi::{self, DpiInfo};
use crate::model::{ShadeProject, TestCodePosition, apply_curve, apply_levels};
use crate::tiff_io::{
    ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_strip, stream_info,
};

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
        ColorModel::Rgb | ColorModel::Cmyk
    ) {
        return Err(format!(
            "Export currently supports RGB and CMYK Photoshop TIFF; this file is {}.",
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
                        mixed
                    }
                    _ => prepared[out_channel],
                };
                output[base + out_channel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
            }
        }
        if y % progress_step == 0 {
            let fraction = y as f32 / height.max(1) as f32;
            progress(0.08 + fraction * 0.72, "Applying adjustments");
        }
    }

    if project.test_code.enabled {
        let text = project.effective_test_code_text();
        if !text.trim().is_empty() {
            if let Some(channel) = names
                .iter()
                .position(|name| name == &project.test_code.channel)
            {
                progress(0.82, "Rendering test code");
                let target_value = if channel >= base_channels {
                    0
                } else {
                    match decoded.metadata.color_model {
                        ColorModel::Cmyk => u16::MAX,
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
                    &text,
                    project.test_code.font_size_pt,
                    project.test_code.margin_cm,
                    project.test_code.position,
                    dpi_info,
                )?;
            }
        }
    }

    progress(0.88, "Writing TIFF");
    let file =
        File::create(destination).map_err(|err| format!("Cannot create export TIFF: {err}"))?;
    let writer = BufWriter::new(file);
    let mut encoder =
        TiffEncoder::new(writer).map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;

    match (decoded.metadata.color_model, decoded.metadata.bit_depth) {
        (ColorModel::Rgb, 8) => {
            let data = output
                .into_iter()
                .map(|value| (value >> 8) as u8)
                .collect::<Vec<_>>();
            let mut image = encoder
                .new_image::<colortype::RGB8>(decoded.metadata.width, decoded.metadata.height)
                .map_err(|err| format!("Cannot create RGB 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, &decoded.metadata, dpi_info)?;
            image
                .write_data(&data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Rgb, 16) => {
            let mut image = encoder
                .new_image::<colortype::RGB16>(decoded.metadata.width, decoded.metadata.height)
                .map_err(|err| format!("Cannot create RGB 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, &decoded.metadata, dpi_info)?;
            image
                .write_data(&output)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Cmyk, 8) => {
            let data = output
                .into_iter()
                .map(|value| (value >> 8) as u8)
                .collect::<Vec<_>>();
            let mut image = encoder
                .new_image::<colortype::CMYK8>(decoded.metadata.width, decoded.metadata.height)
                .map_err(|err| format!("Cannot create CMYK 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, &decoded.metadata, dpi_info)?;
            image
                .write_data(&data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Cmyk, 16) => {
            let mut image = encoder
                .new_image::<colortype::CMYK16>(decoded.metadata.width, decoded.metadata.height)
                .map_err(|err| format!("Cannot create CMYK 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, &decoded.metadata, dpi_info)?;
            image
                .write_data(&output)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (_, depth) => {
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
    stream: &StreamInfo,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    let metadata = &stream.metadata;
    if !matches!(metadata.color_model, ColorModel::Rgb | ColorModel::Cmyk) {
        return Err(format!(
            "Export currently supports RGB and CMYK Photoshop TIFF; this file is {}.",
            metadata.color_model.title()
        ));
    }
    let channels = metadata.samples_per_pixel;
    let base_channels = metadata.base_channel_count;
    if channels == 0 || channels < base_channels {
        return Err("Invalid TIFF channel layout.".to_owned());
    }
    let dpi_info = dpi::read_dpi(source, default_dpi);
    let overlay = build_project_test_code_overlay(
        metadata.width as usize,
        metadata.height as usize,
        metadata,
        project,
        dpi_info,
    )?;

    progress(0.05, "Preparing streaming TIFF");
    let file =
        File::create(destination).map_err(|err| format!("Cannot create export TIFF: {err}"))?;
    let writer = BufWriter::new(file);
    let mut encoder =
        TiffEncoder::new(writer).map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;

    match (metadata.color_model, metadata.bit_depth) {
        (ColorModel::Rgb, 8) => {
            let mut image = encoder
                .new_image::<colortype::RGB8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create RGB 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
            image
                .rows_per_strip(stream.rows_per_strip)
                .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            stream_write_u8(
                source,
                stream,
                project,
                overlay.as_ref(),
                &mut image,
                progress,
            )?;
            image
                .finish()
                .map_err(|err| format!("Cannot finalize TIFF: {err}"))?;
        }
        (ColorModel::Rgb, 16) => {
            let mut image = encoder
                .new_image::<colortype::RGB16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create RGB 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
            image
                .rows_per_strip(stream.rows_per_strip)
                .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            stream_write_u16(
                source,
                stream,
                project,
                overlay.as_ref(),
                &mut image,
                progress,
            )?;
            image
                .finish()
                .map_err(|err| format!("Cannot finalize TIFF: {err}"))?;
        }
        (ColorModel::Cmyk, 8) => {
            let mut image = encoder
                .new_image::<colortype::CMYK8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            image
                .rows_per_strip(stream.rows_per_strip)
                .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            stream_write_u8(
                source,
                stream,
                project,
                overlay.as_ref(),
                &mut image,
                progress,
            )?;
            image
                .finish()
                .map_err(|err| format!("Cannot finalize TIFF: {err}"))?;
        }
        (ColorModel::Cmyk, 16) => {
            let mut image = encoder
                .new_image::<colortype::CMYK16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            image
                .rows_per_strip(stream.rows_per_strip)
                .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            stream_write_u16(
                source,
                stream,
                project,
                overlay.as_ref(),
                &mut image,
                progress,
            )?;
            image
                .finish()
                .map_err(|err| format!("Cannot finalize TIFF: {err}"))?;
        }
        (_, depth) => {
            return Err(format!(
                "Unsupported export bit depth/color model: {depth}-bit."
            ));
        }
    }
    progress(1.0, "Export complete");
    Ok(())
}

fn adjusted_strip(
    input: &[u16],
    channels: usize,
    names: &[String],
    project: &ShadeProject,
) -> Vec<u16> {
    let pixel_count = input.len() / channels.max(1);
    let mut output = vec![0u16; pixel_count.saturating_mul(channels)];
    let mut prepared = vec![0.0f32; channels];
    for pixel in 0..pixel_count {
        let base = pixel * channels;
        for channel in 0..channels {
            let raw = input[base + channel] as f32 / 65535.0;
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
                    mixed
                }
                _ => prepared[out_channel],
            };
            output[base + out_channel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }
    output
}

fn stream_write_u8<W, C, K, F>(
    source: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    image: &mut tiff::encoder::ImageEncoder<'_, W, C, K>,
    progress: &mut F,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    C: tiff::encoder::colortype::ColorType<Inner = u8>,
    K: tiff::encoder::TiffKind,
    F: FnMut(f32, &str),
{
    let channels = stream.metadata.samples_per_pixel;
    let names = &stream.metadata.channel_names;
    for_each_decoded_strip(source, stream, |row_start, row_count, input| {
        let mut adjusted = adjusted_strip(input, channels, names, project);
        if let Some(overlay) = overlay {
            apply_text_overlay_to_rows(
                &mut adjusted,
                row_start as usize,
                row_count as usize,
                stream.metadata.width as usize,
                channels,
                overlay,
            );
        }
        let data = adjusted
            .into_iter()
            .map(|value| (value >> 8) as u8)
            .collect::<Vec<_>>();
        let expected = image.next_strip_sample_count() as usize;
        if data.len() != expected {
            return Err(format!(
                "Output strip sample mismatch: generated {}, encoder expects {expected}.",
                data.len()
            ));
        }
        image
            .write_strip(&data)
            .map_err(|err| format!("Cannot write TIFF strip: {err}"))?;
        let done =
            row_start.saturating_add(row_count) as f32 / stream.metadata.height.max(1) as f32;
        progress(0.06 + done * 0.90, "Streaming adjustments and TIFF strips");
        Ok(())
    })
}

fn stream_write_u16<W, C, K, F>(
    source: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    image: &mut tiff::encoder::ImageEncoder<'_, W, C, K>,
    progress: &mut F,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    C: tiff::encoder::colortype::ColorType<Inner = u16>,
    K: tiff::encoder::TiffKind,
    F: FnMut(f32, &str),
{
    let channels = stream.metadata.samples_per_pixel;
    let names = &stream.metadata.channel_names;
    for_each_decoded_strip(source, stream, |row_start, row_count, input| {
        let mut adjusted = adjusted_strip(input, channels, names, project);
        if let Some(overlay) = overlay {
            apply_text_overlay_to_rows(
                &mut adjusted,
                row_start as usize,
                row_count as usize,
                stream.metadata.width as usize,
                channels,
                overlay,
            );
        }
        let expected = image.next_strip_sample_count() as usize;
        if adjusted.len() != expected {
            return Err(format!(
                "Output strip sample mismatch: generated {}, encoder expects {expected}.",
                adjusted.len()
            ));
        }
        image
            .write_strip(&adjusted)
            .map_err(|err| format!("Cannot write TIFF strip: {err}"))?;
        let done =
            row_start.saturating_add(row_count) as f32 / stream.metadata.height.max(1) as f32;
        progress(0.06 + done * 0.90, "Streaming adjustments and TIFF strips");
        Ok(())
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
    target_channel: usize,
    target_value: u16,
    bitmap: TextBitmap,
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
    let Some(target_channel) = metadata
        .channel_names
        .iter()
        .position(|name| name == &project.test_code.channel)
    else {
        return Ok(None);
    };
    let target_value = if target_channel >= metadata.base_channel_count {
        0
    } else if metadata.color_model == ColorModel::Cmyk {
        u16::MAX
    } else {
        0
    };
    build_text_overlay(
        width,
        height,
        target_channel,
        target_value,
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
    target_channel: usize,
    target_value: u16,
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
        target_channel,
        target_value,
        bitmap,
    })
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
            let index = (local_y * width + x) * channels + overlay.target_channel;
            if index >= samples.len() {
                continue;
            }
            let a = f32::from(alpha) / 255.0;
            let current = samples[index] as f32;
            samples[index] = (current * (1.0 - a) + overlay.target_value as f32 * a).round() as u16;
        }
    }
}

fn draw_test_code(
    samples: &mut [u16],
    width: usize,
    height: usize,
    channels: usize,
    target_channel: usize,
    target_value: u16,
    text: &str,
    font_size_pt: f32,
    margin_cm: f32,
    position: TestCodePosition,
    dpi_info: DpiInfo,
) -> Result<(), String> {
    let overlay = build_text_overlay(
        width,
        height,
        target_channel,
        target_value,
        text,
        font_size_pt,
        margin_cm,
        position,
        dpi_info,
    )?;
    apply_text_overlay_to_rows(samples, 0, height, width, channels, &overlay);
    Ok(())
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
