use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorModel {
    Gray,
    Rgb,
    Cmyk,
    Other,
}

impl ColorModel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Gray => "Gray",
            Self::Rgb => "RGB",
            Self::Cmyk => "CMYK",
            Self::Other => "Multichannel",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TiffMetadata {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    /// Actual TIFF SamplesPerPixel. Do not infer this from ColorType: Photoshop
    /// may store additional alpha/spot separations as extra samples.
    pub samples_per_pixel: usize,
    pub base_channel_count: usize,
    pub color_model: ColorModel,
    pub channel_names: Vec<String>,
    pub icc_profile: Option<Vec<u8>>,
    pub photoshop_resources: Option<Vec<u8>>,
    pub photoshop_image_source_data: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub metadata: TiffMetadata,
    /// Chunky/interleaved samples normalized to 0..=65535 regardless of the
    /// original 8/16-bit sample depth.
    pub samples: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct PreviewFace {
    pub metadata: TiffMetadata,
    pub width: usize,
    pub height: usize,
    /// One downsampled 16-bit plane per TIFF sample/channel.
    pub channels: Vec<Vec<u16>>,
    pub histograms: Vec<[u32; 256]>,
}

fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>, String> {
    let file = File::open(path).map_err(|err| format!("Cannot open TIFF: {err}"))?;
    let reader = BufReader::new(file);
    let decoder = Decoder::new(reader)
        .map_err(|err| format!("Invalid/unsupported TIFF: {err}"))?
        // The crate's default whole-image decode cap is intentionally modest.
        // Production ceramic artwork routinely exceeds it. We still validate
        // dimensions/sample counts below before allocating application buffers.
        .with_limits(Limits::unlimited());
    Ok(decoder)
}

pub fn decode_full(path: &Path) -> Result<DecodedImage, String> {
    let mut decoder = open_decoder(path)?;
    let (metadata, planar_configuration) = read_metadata(&mut decoder)?;

    let pixel_count = (metadata.width as usize)
        .checked_mul(metadata.height as usize)
        .ok_or_else(|| "TIFF dimensions are too large.".to_owned())?;
    let expected = pixel_count
        .checked_mul(metadata.samples_per_pixel)
        .ok_or_else(|| "TIFF sample count is too large.".to_owned())?;
    // Refuse obviously unreasonable/corrupt declarations before a second large
    // application allocation. This is ~64 GiB at u16 and far above intended use.
    if expected > 32_000_000_000usize {
        return Err("TIFF declares an unreasonable number of samples.".to_owned());
    }

    let mut decoded = DecodingResult::U8(Vec::new());
    let layout = decoder
        .read_image_to_buffer(&mut decoded)
        .map_err(|err| format!("Cannot decode TIFF pixels: {err}"))?;

    let mut samples = decoding_result_to_u16(decoded, metadata.bit_depth)?;
    if samples.len() < expected {
        return Err(format!(
            "Decoded TIFF data is incomplete ({} of {} samples).",
            samples.len(), expected
        ));
    }
    if samples.len() > expected {
        samples.truncate(expected);
    }

    // read_image_to_buffer returns all planar planes when the configured limits
    // permit it. Limits are unlimited above, so plane-consecutive data must be
    // converted once into the application's canonical chunky representation.
    if planar_configuration == 2 || layout.planes > 1 {
        samples = planar_to_interleaved(&samples, pixel_count, metadata.samples_per_pixel);
    }

    Ok(DecodedImage { metadata, samples })
}

pub fn load_preview(path: &Path, max_dimension: u32) -> Result<PreviewFace, String> {
    // v0.2 removes the decoder's 256 MiB default ceiling. The decoder is also
    // switched from deprecated read_image() to read_image_to_buffer(), which is
    // required for correct multi-plane TIFFs. A future optimization can sample
    // strips/tiles directly without changing this PreviewFace API.
    let decoded = decode_full(path)?;
    let source_width = decoded.metadata.width as usize;
    let source_height = decoded.metadata.height as usize;
    let max_source = source_width.max(source_height).max(1);
    let max_dimension = max_dimension.max(256) as usize;
    let scale = (max_source as f64 / max_dimension as f64).max(1.0);
    let width = ((source_width as f64 / scale).round() as usize).max(1);
    let height = ((source_height as f64 / scale).round() as usize).max(1);
    let channel_count = decoded.metadata.samples_per_pixel;

    let preview_pixels = width
        .checked_mul(height)
        .ok_or_else(|| "Preview dimensions are too large.".to_owned())?;
    let mut channels = (0..channel_count)
        .map(|_| Vec::with_capacity(preview_pixels))
        .collect::<Vec<_>>();

    for y in 0..height {
        let src_y = ((y as f64 * source_height as f64 / height as f64).floor() as usize)
            .min(source_height - 1);
        for x in 0..width {
            let src_x = ((x as f64 * source_width as f64 / width as f64).floor() as usize)
                .min(source_width - 1);
            let base = (src_y * source_width + src_x) * channel_count;
            for channel in 0..channel_count {
                channels[channel].push(decoded.samples[base + channel]);
            }
        }
    }

    let histograms = channels.iter().map(|plane| histogram(plane)).collect();
    Ok(PreviewFace {
        metadata: decoded.metadata,
        width,
        height,
        channels,
        histograms,
    })
}

fn read_metadata<R: std::io::Read + std::io::Seek>(
    decoder: &mut Decoder<R>,
) -> Result<(TiffMetadata, u16), String> {
    let (width, height) = decoder
        .dimensions()
        .map_err(|err| format!("Cannot read TIFF dimensions: {err}"))?;
    let color_type = decoder
        .colortype()
        .map_err(|err| format!("Cannot read TIFF color type: {err}"))?;
    let bit_depth = color_type.bit_depth();
    if !matches!(bit_depth, 8 | 16) {
        return Err(format!(
            "Shade Editor currently supports 8-bit and 16-bit TIFF only; file is {bit_depth}-bit."
        ));
    }

    // This is the critical Photoshop/spot-channel invariant: TIFF tag 277 is
    // authoritative. ColorType may describe the base photometric model while
    // SamplesPerPixel includes additional unspecified samples.
    let samples_per_pixel = decoder
        .find_tag_unsigned::<u16>(Tag::SamplesPerPixel)
        .map_err(|err| format!("Cannot read TIFF SamplesPerPixel: {err}"))?
        .map(usize::from)
        .unwrap_or_else(|| usize::from(color_type.num_samples()));
    if samples_per_pixel == 0 || samples_per_pixel > 56 {
        return Err(format!("Unsupported TIFF channel count: {samples_per_pixel}."));
    }

    let photometric = decoder
        .find_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
        .ok()
        .flatten();
    let (color_model, base_channel_count) = infer_color_model(&color_type, photometric);
    if samples_per_pixel < base_channel_count {
        return Err(format!(
            "TIFF SamplesPerPixel ({samples_per_pixel}) is smaller than its {} base channel count ({base_channel_count}).",
            color_model.title()
        ));
    }

    let planar_configuration = decoder
        .find_tag_unsigned::<u16>(Tag::PlanarConfiguration)
        .ok()
        .flatten()
        .unwrap_or(1);
    if !matches!(planar_configuration, 1 | 2) {
        return Err(format!("Unsupported TIFF PlanarConfiguration: {planar_configuration}."));
    }

    let icc_profile = decoder.get_tag_u8_vec(Tag::IccProfile).ok();
    let photoshop_resources = decoder.get_tag_u8_vec(Tag::Unknown(34377)).ok();
    let photoshop_image_source_data = decoder.get_tag_u8_vec(Tag::Unknown(37724)).ok();
    let channel_names = channel_names(
        color_model,
        base_channel_count,
        samples_per_pixel,
        photoshop_resources.as_deref(),
    );

    Ok((
        TiffMetadata {
            width,
            height,
            bit_depth,
            samples_per_pixel,
            base_channel_count,
            color_model,
            channel_names,
            icc_profile,
            photoshop_resources,
            photoshop_image_source_data,
        },
        planar_configuration,
    ))
}

fn infer_color_model(color_type: &ColorType, photometric: Option<u16>) -> (ColorModel, usize) {
    // TIFF 6.0 PhotometricInterpretation: 0/1 gray, 2 RGB, 5 separated/CMYK.
    match photometric {
        Some(2) => return (ColorModel::Rgb, 3),
        Some(5) => return (ColorModel::Cmyk, 4),
        Some(0 | 1) => return (ColorModel::Gray, 1),
        _ => {}
    }
    match color_type {
        ColorType::RGB(_) | ColorType::RGBA(_) => (ColorModel::Rgb, 3),
        ColorType::CMYK(_) | ColorType::CMYKA(_) => (ColorModel::Cmyk, 4),
        ColorType::Gray(_) | ColorType::GrayA(_) => (ColorModel::Gray, 1),
        _ => (ColorModel::Other, usize::from(color_type.num_samples()).max(1)),
    }
}

fn decoding_result_to_u16(decoded: DecodingResult, bit_depth: u8) -> Result<Vec<u16>, String> {
    match decoded {
        DecodingResult::U8(values) if bit_depth == 8 => {
            Ok(values.into_iter().map(|value| u16::from(value) * 257).collect())
        }
        DecodingResult::U16(values) if bit_depth == 16 => Ok(values),
        DecodingResult::U8(values) => {
            Ok(values.into_iter().map(|value| u16::from(value) * 257).collect())
        }
        DecodingResult::U16(values) => Ok(values),
        _ => Err("This TIFF sample type is not supported by Shade Editor.".to_owned()),
    }
}

fn planar_to_interleaved(planar: &[u16], pixel_count: usize, channels: usize) -> Vec<u16> {
    let mut interleaved = vec![0u16; pixel_count.saturating_mul(channels)];
    for channel in 0..channels {
        let plane_start = channel.saturating_mul(pixel_count);
        if plane_start + pixel_count > planar.len() {
            break;
        }
        for pixel in 0..pixel_count {
            interleaved[pixel * channels + channel] = planar[plane_start + pixel];
        }
    }
    interleaved
}

fn histogram(values: &[u16]) -> [u32; 256] {
    let mut bins = [0u32; 256];
    for value in values {
        let index = usize::from(*value >> 8);
        bins[index] = bins[index].saturating_add(1);
    }
    bins
}

fn channel_names(
    model: ColorModel,
    base_count: usize,
    total_count: usize,
    photoshop: Option<&[u8]>,
) -> Vec<String> {
    let mut names = match model {
        ColorModel::Rgb => vec!["Red", "Green", "Blue"],
        ColorModel::Cmyk => vec!["Cyan", "Magenta", "Yellow", "Black"],
        ColorModel::Gray => vec!["Gray"],
        ColorModel::Other => (0..base_count).map(|index| format!("Channel {}", index + 1)).collect(),
    };
    names.truncate(base_count);
    while names.len() < base_count {
        names.push(format!("Channel {}", names.len() + 1));
    }

    let extra_count = total_count.saturating_sub(base_count);
    let photoshop_names = photoshop.map(parse_photoshop_channel_names).unwrap_or_default();
    for index in 0..extra_count {
        let name = photoshop_names
            .get(index)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| format!("Spot/Alpha {}", index + 1));
        names.push(unique_name(name, &names));
    }
    while names.len() < total_count {
        names.push(format!("Channel {}", names.len() + 1));
    }
    names.truncate(total_count);
    names
}

fn unique_name(mut name: String, existing: &[String]) -> String {
    if !existing.iter().any(|item| item == &name) {
        return name;
    }
    let root = name.clone();
    let mut suffix = 2;
    while existing.iter().any(|item| item == &name) {
        name = format!("{root} ({suffix})");
        suffix += 1;
    }
    name
}

/// Extract Photoshop Image Resource alpha/spot channel names. Photoshop has
/// used both Pascal-string resource 1006 and Unicode resource 1045. Some files
/// contain more than one block, so resources are accumulated instead of the old
/// last-block-wins behavior.
fn parse_photoshop_channel_names(resources: &[u8]) -> Vec<String> {
    let mut offset = 0usize;
    let mut pascal_names = Vec::new();
    let mut unicode_names = Vec::new();

    while offset + 12 <= resources.len() {
        if &resources[offset..offset + 4] != b"8BIM" {
            offset += 1;
            continue;
        }
        offset += 4;
        if offset + 2 > resources.len() { break; }
        let id = u16::from_be_bytes([resources[offset], resources[offset + 1]]);
        offset += 2;

        if offset >= resources.len() { break; }
        let name_len = resources[offset] as usize;
        offset += 1;
        if offset + name_len > resources.len() { break; }
        offset += name_len;
        if (1 + name_len) % 2 != 0 {
            offset = offset.saturating_add(1);
        }
        if offset + 4 > resources.len() { break; }
        let size = u32::from_be_bytes([
            resources[offset], resources[offset + 1], resources[offset + 2], resources[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + size > resources.len() { break; }
        let data = &resources[offset..offset + size];

        match id {
            1006 => pascal_names.extend(parse_pascal_names(data)),
            1045 => unicode_names.extend(parse_unicode_names(data)),
            _ => {}
        }

        offset += size;
        if size % 2 != 0 {
            offset = offset.saturating_add(1);
        }
    }

    if !unicode_names.is_empty() { unicode_names } else { pascal_names }
}

fn parse_pascal_names(data: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let len = data[offset] as usize;
        offset += 1;
        if offset + len > data.len() { break; }
        names.push(String::from_utf8_lossy(&data[offset..offset + len]).into_owned());
        offset += len;
    }
    names
}

fn parse_unicode_names(data: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut offset = 0usize;
    while offset + 4 <= data.len() {
        let units = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]) as usize;
        offset += 4;
        let byte_len = match units.checked_mul(2) {
            Some(value) => value,
            None => break,
        };
        if offset + byte_len > data.len() { break; }
        let mut utf16 = Vec::with_capacity(units);
        for pair in data[offset..offset + byte_len].chunks_exact(2) {
            utf16.push(u16::from_be_bytes([pair[0], pair[1]]));
        }
        names.push(String::from_utf16_lossy(&utf16).trim_end_matches('\0').to_owned());
        offset += byte_len;
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_planar_data() {
        let planar = vec![1, 2, 3, 10, 20, 30];
        assert_eq!(planar_to_interleaved(&planar, 3, 2), vec![1, 10, 2, 20, 3, 30]);
    }

    #[test]
    fn names_all_photoshop_extra_channels() {
        let names = channel_names(ColorModel::Cmyk, 4, 6, None);
        assert_eq!(names.len(), 6);
        assert_eq!(names[0], "Cyan");
        assert_eq!(names[4], "Spot/Alpha 1");
        assert_eq!(names[5], "Spot/Alpha 2");
    }

    #[test]
    fn photometric_overrides_multiband_shape() {
        let fake = ColorType::Multiband { bit_depth: 8, num_samples: 6 };
        assert_eq!(infer_color_model(&fake, Some(2)), (ColorModel::Rgb, 3));
        assert_eq!(infer_color_model(&fake, Some(5)), (ColorModel::Cmyk, 4));
    }
}
