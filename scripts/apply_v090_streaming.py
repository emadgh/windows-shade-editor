from pathlib import Path

TIFF = Path('src/tiff_io.rs')
EXPORT = Path('src/export_v6.rs')

tiff = TIFF.read_text(encoding='utf-8')
export = EXPORT.read_text(encoding='utf-8')


def once(text, old, new, label):
    if old not in text:
        raise SystemExit(f'anchor not found: {label}')
    return text.replace(old, new, 1)

# --- tiff_io incremental strip decode ---
insert_after_preview = '''#[derive(Clone, Debug)]
pub struct PreviewFace {
    pub metadata: TiffMetadata,
    pub width: usize,
    pub height: usize,
    /// One downsampled 16-bit plane per TIFF sample/channel.
    pub channels: Vec<Vec<u16>>,
    pub histograms: Vec<[u32; 256]>,
}
'''
stream_struct = insert_after_preview + '''
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub metadata: TiffMetadata,
    pub rows_per_strip: u32,
    pub strip_count: u32,
    /// True when the source is chunky/interleaved and strip-based, allowing
    /// bounded-memory incremental decoding. Planar/tiled files use the proven
    /// full-image compatibility path.
    pub streamable: bool,
}
'''
tiff = once(tiff, insert_after_preview, stream_struct, 'StreamInfo struct')

# Replace old full-image preview with strip-streamed downsampling and add public stream helpers.
preview_start = tiff.index('pub fn load_preview(path: &Path, max_dimension: u32) -> Result<PreviewFace, String> {')
preview_end = tiff.index('fn read_metadata<R: Read + Seek>(', preview_start)
new_preview = r'''pub fn stream_info(path: &Path) -> Result<StreamInfo, String> {
    let mut decoder = open_decoder(path)?;
    let (metadata, planar_configuration) = read_metadata(&mut decoder)?;
    let rows_per_strip = decoder
        .find_tag_unsigned::<u32>(Tag::RowsPerStrip)
        .ok()
        .flatten()
        .unwrap_or(metadata.height)
        .max(1)
        .min(metadata.height.max(1));
    let strip_count = decoder.strip_count().ok();
    Ok(StreamInfo {
        metadata,
        rows_per_strip,
        strip_count: strip_count.unwrap_or(1),
        streamable: planar_configuration == 1 && strip_count.is_some(),
    })
}

pub fn for_each_decoded_strip<F>(
    path: &Path,
    info: &StreamInfo,
    mut callback: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, &[u16]) -> Result<(), String>,
{
    if !info.streamable {
        let decoded = decode_full(path)?;
        callback(0, decoded.metadata.height, &decoded.samples)?;
        return Ok(());
    }

    let needs_multiband_workaround = info.metadata.samples_per_pixel > info.metadata.base_channel_count
        && matches!(info.metadata.color_model, ColorModel::Rgb | ColorModel::Cmyk);
    if needs_multiband_workaround {
        let decoder = open_multiband_decoder(path)?;
        stream_decoder_strips(decoder, info, &mut callback)
    } else {
        let decoder = open_decoder(path)?;
        stream_decoder_strips(decoder, info, &mut callback)
    }
}

fn stream_decoder_strips<R, F>(
    mut decoder: Decoder<R>,
    info: &StreamInfo,
    callback: &mut F,
) -> Result<(), String>
where
    R: Read + Seek,
    F: FnMut(u32, u32, &[u16]) -> Result<(), String>,
{
    let strip_count = decoder
        .strip_count()
        .map_err(|err| format!("Cannot read TIFF strip count: {err}"))?;
    let width = info.metadata.width as usize;
    let channels = info.metadata.samples_per_pixel;
    let mut row_start = 0u32;

    for strip_index in 0..strip_count {
        let (chunk_width, row_count) = decoder.chunk_data_dimensions(strip_index);
        if chunk_width != info.metadata.width {
            return Err(format!(
                "Unexpected TIFF strip width {chunk_width}; expected {}.",
                info.metadata.width
            ));
        }
        let decoded = decoder
            .read_chunk(strip_index)
            .map_err(|err| format!("Cannot decode TIFF strip {strip_index}: {err}"))?;
        let mut samples = decoding_result_to_u16(decoded, info.metadata.bit_depth)?;
        let expected = width
            .checked_mul(row_count as usize)
            .and_then(|pixels| pixels.checked_mul(channels))
            .ok_or_else(|| "TIFF strip sample count is too large.".to_owned())?;
        if samples.len() < expected {
            return Err(format!(
                "Decoded TIFF strip {strip_index} is incomplete ({} of {expected} samples).",
                samples.len()
            ));
        }
        samples.truncate(expected);
        callback(row_start, row_count, &samples)?;
        row_start = row_start.saturating_add(row_count);
    }

    if row_start < info.metadata.height {
        return Err(format!(
            "TIFF strip stream ended at row {row_start} of {}.",
            info.metadata.height
        ));
    }
    Ok(())
}

pub fn load_preview(path: &Path, max_dimension: u32) -> Result<PreviewFace, String> {
    let info = stream_info(path)?;
    let source_width = info.metadata.width as usize;
    let source_height = info.metadata.height as usize;
    let max_source = source_width.max(source_height).max(1);
    let max_dimension = max_dimension.max(256) as usize;
    let scale = (max_source as f64 / max_dimension as f64).max(1.0);
    let width = ((source_width as f64 / scale).round() as usize).max(1);
    let height = ((source_height as f64 / scale).round() as usize).max(1);
    let channel_count = info.metadata.samples_per_pixel;
    let preview_pixels = width
        .checked_mul(height)
        .ok_or_else(|| "Preview dimensions are too large.".to_owned())?;
    let mut channels = (0..channel_count)
        .map(|_| vec![0u16; preview_pixels])
        .collect::<Vec<_>>();

    let source_x = (0..width)
        .map(|x| {
            ((x as f64 * source_width as f64 / width as f64).floor() as usize)
                .min(source_width.saturating_sub(1))
        })
        .collect::<Vec<_>>();
    let source_y = (0..height)
        .map(|y| {
            ((y as f64 * source_height as f64 / height as f64).floor() as usize)
                .min(source_height.saturating_sub(1))
        })
        .collect::<Vec<_>>();
    let mut next_preview_y = 0usize;

    for_each_decoded_strip(path, &info, |row_start, row_count, samples| {
        let row_end = row_start.saturating_add(row_count) as usize;
        while next_preview_y < height && source_y[next_preview_y] < row_end {
            let src_y = source_y[next_preview_y];
            if src_y < row_start as usize {
                next_preview_y += 1;
                continue;
            }
            let local_y = src_y - row_start as usize;
            for (preview_x, &src_x) in source_x.iter().enumerate() {
                let source_base = (local_y * source_width + src_x) * channel_count;
                let destination = next_preview_y * width + preview_x;
                for channel in 0..channel_count {
                    channels[channel][destination] = samples[source_base + channel];
                }
            }
            next_preview_y += 1;
        }
        Ok(())
    })?;

    if next_preview_y != height {
        return Err(format!(
            "Preview stream filled {next_preview_y} of {height} rows."
        ));
    }
    let histograms = channels.iter().map(|plane| histogram(plane)).collect();
    Ok(PreviewFace {
        metadata: info.metadata,
        width,
        height,
        channels,
        histograms,
    })
}

'''
tiff = tiff[:preview_start] + new_preview + tiff[preview_end:]

# --- export bounded-memory strip pipeline ---
export = once(
    export,
    'use crate::tiff_io::{ColorModel, TiffMetadata, decode_full};',
    'use crate::tiff_io::{ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_strip, stream_info};',
    'export stream imports',
)

start_anchor = '''{
    progress(0.02, "Decoding TIFF");
    let decoded = decode_full(source)?;'''
start_new = '''{
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
    let decoded = decode_full(source)?;'''
export = once(export, start_anchor, start_new, 'streaming export entry')

# Insert streaming export helpers before configure_extras_and_metadata.
helper_anchor = 'fn configure_extras_and_metadata<W, C, K>('
helpers = r'''fn export_face_streaming<F>(
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
    let file = File::create(destination)
        .map_err(|err| format!("Cannot create export TIFF: {err}"))?;
    let writer = BufWriter::new(file);
    let mut encoder = TiffEncoder::new(writer)
        .map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;

    match (metadata.color_model, metadata.bit_depth) {
        (ColorModel::Rgb, 8) => {
            let mut image = encoder
                .new_image::<colortype::RGB8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create RGB 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
            image
                .rows_per_strip(stream.rows_per_strip)
                .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            stream_write_u8(source, stream, project, overlay.as_ref(), &mut image, progress)?;
            image.finish().map_err(|err| format!("Cannot finalize TIFF: {err}"))?;
        }
        (ColorModel::Rgb, 16) => {
            let mut image = encoder
                .new_image::<colortype::RGB16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create RGB 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
            image
                .rows_per_strip(stream.rows_per_strip)
                .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            stream_write_u16(source, stream, project, overlay.as_ref(), &mut image, progress)?;
            image.finish().map_err(|err| format!("Cannot finalize TIFF: {err}"))?;
        }
        (ColorModel::Cmyk, 8) => {
            let mut image = encoder
                .new_image::<colortype::CMYK8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            image
                .rows_per_strip(stream.rows_per_strip)
                .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            stream_write_u8(source, stream, project, overlay.as_ref(), &mut image, progress)?;
            image.finish().map_err(|err| format!("Cannot finalize TIFF: {err}"))?;
        }
        (ColorModel::Cmyk, 16) => {
            let mut image = encoder
                .new_image::<colortype::CMYK16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            image
                .rows_per_strip(stream.rows_per_strip)
                .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            stream_write_u16(source, stream, project, overlay.as_ref(), &mut image, progress)?;
            image.finish().map_err(|err| format!("Cannot finalize TIFF: {err}"))?;
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
                            .unwrap_or(if source_channel == out_channel { 1.0 } else { 0.0 });
                        mixed += prepared[source_channel] * coefficient;
                    }
                    mixed
                }
                _ => prepared[out_channel],
            };
            output[base + out_channel] =
                (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
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
        let done = row_start.saturating_add(row_count) as f32
            / stream.metadata.height.max(1) as f32;
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
        let done = row_start.saturating_add(row_count) as f32
            / stream.metadata.height.max(1) as f32;
        progress(0.06 + done * 0.90, "Streaming adjustments and TIFF strips");
        Ok(())
    })
}

'''
if helper_anchor not in export:
    raise SystemExit('configure metadata anchor missing')
export = export.replace(helper_anchor, helpers + helper_anchor, 1)

# Refactor test-code rendering so streaming strips can overlay only intersecting rows.
old_draw_start = export.index('fn draw_test_code(')
old_draw_end = export.index('struct TextBitmap {', old_draw_start)
old_draw = export[old_draw_start:old_draw_end]
new_draw = r'''struct TextOverlay {
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
            samples[index] =
                (current * (1.0 - a) + overlay.target_value as f32 * a).round() as u16;
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

'''
export = export[:old_draw_start] + new_draw + export[old_draw_end:]

TIFF.write_text(tiff, encoding='utf-8')
EXPORT.write_text(export, encoding='utf-8')
