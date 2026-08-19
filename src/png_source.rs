use std::fs::File;
use std::io::{BufRead, BufReader, Seek};
use std::path::Path;

use png::{BitDepth, ColorType, Decoder, Transformations};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngSourceModel {
    Gray,
    Rgb,
}

#[derive(Clone, Debug)]
pub struct DecodedPngSource {
    pub width: u32,
    pub height: u32,
    /// Output precision after mandatory palette/low-bit expansion. Shade Editor
    /// keeps 8-bit source precision distinguishable even though samples are
    /// normalized into the common 16-bit working representation.
    pub bit_depth: u8,
    pub model: PngSourceModel,
    /// Interleaved base samples normalized to 0..=65535. Gray has one sample per
    /// pixel; RGB has three. Alpha is deliberately stored separately.
    pub samples: Vec<u16>,
    /// Optional normalized opacity plane. This is source transparency, never a
    /// printing ink channel. Production conversion requires an explicit flatten policy.
    pub alpha: Option<Vec<u16>>,
    pub icc_profile: Option<Vec<u8>>,
    /// True when the PNG declares the standard sRGB chunk. This describes source
    /// interpretation metadata but does not silently assign an ICC profile itself.
    pub declares_srgb: bool,
}

pub fn decode_png_source(path: &Path) -> Result<DecodedPngSource, String> {
    let file = File::open(path).map_err(|err| format!("Cannot open PNG source: {err}"))?;
    decode_png_reader(BufReader::new(file))
}

pub fn decode_png_reader<R: BufRead + Seek>(reader: R) -> Result<DecodedPngSource, String> {
    let mut decoder = Decoder::new(reader);
    // EXPAND converts palette PNG to RGB/RGBA, low-bit gray to 8-bit, and tRNS
    // transparency to an alpha channel while leaving 16-bit source samples intact.
    decoder.set_transformations(Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .map_err(|err| format!("Cannot read PNG metadata: {err}"))?;

    let icc_profile = reader.info().icc_profile.as_deref().map(ToOwned::to_owned);
    let declares_srgb = reader.info().srgb.is_some();
    let buffer_size = reader
        .output_buffer_size()
        .ok_or_else(|| "PNG decoded buffer size does not fit this platform.".to_owned())?;
    let mut buffer = vec![0u8; buffer_size];
    let output = reader
        .next_frame(&mut buffer)
        .map_err(|err| format!("Cannot decode PNG pixels: {err}"))?;
    let bytes = &buffer[..output.buffer_size()];

    let bit_depth = match output.bit_depth {
        BitDepth::Eight => 8,
        BitDepth::Sixteen => 16,
        depth => {
            return Err(format!(
                "PNG expansion returned unsupported {:?} output depth.",
                depth
            ));
        }
    };

    let pixel_count = (output.width as usize)
        .checked_mul(output.height as usize)
        .ok_or_else(|| "PNG dimensions are too large.".to_owned())?;
    let (model, base_channels, has_alpha) = match output.color_type {
        ColorType::Grayscale => (PngSourceModel::Gray, 1usize, false),
        ColorType::GrayscaleAlpha => (PngSourceModel::Gray, 1usize, true),
        ColorType::Rgb => (PngSourceModel::Rgb, 3usize, false),
        ColorType::Rgba => (PngSourceModel::Rgb, 3usize, true),
        ColorType::Indexed => {
            return Err("PNG palette expansion did not produce RGB/RGBA output.".to_owned());
        }
    };
    let output_channels = base_channels + usize::from(has_alpha);
    let expected_samples = pixel_count
        .checked_mul(output_channels)
        .ok_or_else(|| "PNG sample count is too large.".to_owned())?;
    let decoded = decode_samples(bytes, bit_depth, expected_samples)?;

    let mut samples = Vec::with_capacity(pixel_count.saturating_mul(base_channels));
    let mut alpha = has_alpha.then(|| Vec::with_capacity(pixel_count));
    for pixel in decoded.chunks_exact(output_channels) {
        samples.extend_from_slice(&pixel[..base_channels]);
        if let Some(alpha) = alpha.as_mut() {
            alpha.push(pixel[base_channels]);
        }
    }

    Ok(DecodedPngSource {
        width: output.width,
        height: output.height,
        bit_depth,
        model,
        samples,
        alpha,
        icc_profile,
        declares_srgb,
    })
}

fn decode_samples(
    bytes: &[u8],
    bit_depth: u8,
    expected_samples: usize,
) -> Result<Vec<u16>, String> {
    match bit_depth {
        8 => {
            if bytes.len() != expected_samples {
                return Err(format!(
                    "PNG 8-bit sample count mismatch: {} bytes, expected {expected_samples}.",
                    bytes.len()
                ));
            }
            Ok(bytes.iter().map(|value| u16::from(*value) * 257).collect())
        }
        16 => {
            let expected_bytes = expected_samples
                .checked_mul(2)
                .ok_or_else(|| "PNG 16-bit byte count overflow.".to_owned())?;
            if bytes.len() != expected_bytes {
                return Err(format!(
                    "PNG 16-bit sample count mismatch: {} bytes, expected {expected_bytes}.",
                    bytes.len()
                ));
            }
            Ok(bytes
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect())
        }
        _ => Err(format!(
            "Unsupported normalized PNG bit depth: {bit_depth}."
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn encode_png(
        width: u32,
        height: u32,
        color: ColorType,
        depth: BitDepth,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(color);
            encoder.set_depth(depth);
            let mut writer = encoder.write_header().expect("write PNG header");
            writer.write_image_data(pixels).expect("write PNG pixels");
        }
        bytes
    }

    #[test]
    fn rgb8_is_normalized_to_16_bit_working_samples() {
        let encoded = encode_png(1, 1, ColorType::Rgb, BitDepth::Eight, &[0, 128, 255]);
        let decoded = decode_png_reader(Cursor::new(encoded)).expect("decode RGB8 PNG");
        assert_eq!(decoded.model, PngSourceModel::Rgb);
        assert_eq!(decoded.bit_depth, 8);
        assert_eq!(decoded.samples, [0, 128 * 257, u16::MAX]);
        assert!(decoded.alpha.is_none());
    }

    #[test]
    fn rgba16_preserves_precision_and_separates_alpha() {
        let values = [0x1234u16, 0x4567, 0x89ab, 0xcdef];
        let pixels = values
            .into_iter()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>();
        let encoded = encode_png(1, 1, ColorType::Rgba, BitDepth::Sixteen, &pixels);
        let decoded = decode_png_reader(Cursor::new(encoded)).expect("decode RGBA16 PNG");
        assert_eq!(decoded.model, PngSourceModel::Rgb);
        assert_eq!(decoded.bit_depth, 16);
        assert_eq!(decoded.samples, [0x1234, 0x4567, 0x89ab]);
        assert_eq!(decoded.alpha.as_deref(), Some(&[0xcdef][..]));
    }

    #[test]
    fn grayscale_alpha_is_not_interpreted_as_two_printing_channels() {
        let encoded = encode_png(1, 1, ColorType::GrayscaleAlpha, BitDepth::Eight, &[64, 192]);
        let decoded = decode_png_reader(Cursor::new(encoded)).expect("decode GA PNG");
        assert_eq!(decoded.model, PngSourceModel::Gray);
        assert_eq!(decoded.samples, [64 * 257]);
        assert_eq!(decoded.alpha.as_deref(), Some(&[192 * 257][..]));
    }

    #[test]
    fn sample_decoder_rejects_incomplete_buffers() {
        assert!(decode_samples(&[1, 2], 8, 3).is_err());
        assert!(decode_samples(&[0, 1, 0], 16, 2).is_err());
    }
}
