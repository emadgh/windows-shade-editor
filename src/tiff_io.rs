use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

#[derive(Clone, Debug)]
pub struct TiffMetadata {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub samples_per_pixel: usize,
    pub channel_names: Vec<String>,
    pub icc_profile: Option<Vec<u8>>,
    pub photoshop_resources: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub metadata: TiffMetadata,
    /// Interleaved samples normalized to 0..=65535 regardless of original 8/16-bit depth.
    pub samples: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct PreviewFace {
    pub metadata: TiffMetadata,
    pub width: usize,
    pub height: usize,
    /// One downsampled 16-bit plane per channel.
    pub channels: Vec<Vec<u16>>,
    pub histograms: Vec<[u32; 256]>,
}

pub fn decode_full(path: &Path) -> Result<DecodedImage, String> {
    let file = File::open(path).map_err(|err| format!("Cannot open TIFF: {err}"))?;
    let mut decoder = Decoder::new(BufReader::new(file))
        .map_err(|err| format!("Invalid/unsupported TIFF: {err}"))?;

    let (width, height) = decoder.dimensions()
        .map_err(|err| format!("Cannot read TIFF dimensions: {err}"))?;
    let color_type = decoder.colortype()
        .map_err(|err| format!("Cannot read TIFF color type: {err}"))?;
    let bit_depth = color_type.bit_depth();
    let samples_per_pixel = usize::from(color_type.num_samples());
    if samples_per_pixel == 0 {
        return Err("TIFF contains no image channels.".to_owned());
    }
    if !matches!(bit_depth, 8 | 16) {
        return Err(format!("First version supports 8-bit and 16-bit TIFF only; file is {bit_depth}-bit."));
    }

    let planar_configuration = decoder
        .find_tag_unsigned::<u16>(Tag::PlanarConfiguration)
        .ok()
        .flatten()
        .unwrap_or(1);
    let icc_profile = decoder.get_tag_u8_vec(Tag::IccProfile).ok();
    let photoshop_resources = decoder.get_tag_u8_vec(Tag::Unknown(34377)).ok();
    let channel_names = channel_names(&color_type, samples_per_pixel, photoshop_resources.as_deref());

    let decoded = decoder.read_image()
        .map_err(|err| format!("Cannot decode TIFF pixels: {err}"))?;
    let mut samples = match decoded {
        DecodingResult::U8(values) => values.into_iter().map(|value| u16::from(value) * 257).collect(),
        DecodingResult::U16(values) => values,
        _ => return Err("This TIFF sample type is not supported in the first version.".to_owned()),
    };

    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| "TIFF dimensions are too large.".to_owned())?;
    let expected = pixel_count
        .checked_mul(samples_per_pixel)
        .ok_or_else(|| "TIFF sample count is too large.".to_owned())?;
    if samples.len() < expected {
        return Err(format!("Decoded TIFF data is incomplete ({} of {} samples).", samples.len(), expected));
    }
    samples.truncate(expected);

    if planar_configuration == 2 {
        samples = planar_to_interleaved(&samples, pixel_count, samples_per_pixel);
    }

    Ok(DecodedImage {
        metadata: TiffMetadata {
            path: path.to_path_buf(),
            width,
            height,
            bit_depth,
            samples_per_pixel,
            channel_names,
            icc_profile,
            photoshop_resources,
        },
        samples,
    })
}

pub fn load_preview(path: &Path, max_dimension: u32) -> Result<PreviewFace, String> {
    let decoded = decode_full(path)?;
    let source_width = decoded.metadata.width as usize;
    let source_height = decoded.metadata.height as usize;
    let max_source = source_width.max(source_height).max(1);
    let max_dimension = max_dimension.max(256) as usize;
    let scale = (max_source as f64 / max_dimension as f64).max(1.0);
    let width = ((source_width as f64 / scale).round() as usize).max(1);
    let height = ((source_height as f64 / scale).round() as usize).max(1);
    let channel_count = decoded.metadata.samples_per_pixel;

    let mut channels = (0..channel_count)
        .map(|_| Vec::with_capacity(width * height))
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

fn planar_to_interleaved(planar: &[u16], pixel_count: usize, channels: usize) -> Vec<u16> {
    let mut interleaved = vec![0u16; pixel_count * channels];
    for channel in 0..channels {
        let plane_start = channel * pixel_count;
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

fn channel_names(color_type: &ColorType, count: usize, photoshop: Option<&[u8]>) -> Vec<String> {
    let base = match color_type {
        ColorType::CMYK(_) | ColorType::CMYKA(_) => vec!["Cyan", "Magenta", "Yellow", "Black"],
        ColorType::Multiband { .. } if count >= 4 => vec!["Cyan", "Magenta", "Yellow", "Black"],
        ColorType::Gray(_) | ColorType::GrayA(_) => vec!["Gray"],
        ColorType::RGB(_) | ColorType::RGBA(_) => vec!["Red", "Green", "Blue"],
        _ => Vec::new(),
    };

    let mut names = base.into_iter().map(str::to_owned).collect::<Vec<_>>();
    let extra_count = count.saturating_sub(names.len());
    let photoshop_names = photoshop.map(parse_photoshop_channel_names).unwrap_or_default();
    for index in 0..extra_count {
        let name = photoshop_names.get(index)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| format!("Spot {}", index + 1));
        names.push(name);
    }
    while names.len() < count {
        names.push(format!("Channel {}", names.len() + 1));
    }
    names.truncate(count);
    names
}

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
            1006 => pascal_names = parse_pascal_names(data),
            1045 => unicode_names = parse_unicode_names(data),
            _ => {}
        }

        offset += size;
        if size % 2 != 0 { offset = offset.saturating_add(1); }
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
        let units = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]) as usize;
        offset += 4;
        let byte_len = match units.checked_mul(2) { Some(value) => value, None => break };
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
}
