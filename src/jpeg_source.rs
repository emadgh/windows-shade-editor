use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use jpeg_decoder::{CodingProcess, Decoder, PixelFormat};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegSourceModel {
    Gray,
    Rgb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegCodingProcess {
    DctSequential,
    DctProgressive,
    Lossless,
}

impl JpegCodingProcess {
    pub fn is_lossy(self) -> bool {
        !matches!(self, Self::Lossless)
    }
}

impl From<CodingProcess> for JpegCodingProcess {
    fn from(value: CodingProcess) -> Self {
        match value {
            CodingProcess::DctSequential => Self::DctSequential,
            CodingProcess::DctProgressive => Self::DctProgressive,
            CodingProcess::Lossless => Self::Lossless,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecodedJpegSource {
    pub width: u32,
    pub height: u32,
    /// JPEG source precision exposed to the design-source workflow. The current
    /// accepted RGB/Gray layouts are 8 bits per component and are normalized to
    /// the common 16-bit working representation below.
    pub bit_depth: u8,
    pub model: JpegSourceModel,
    /// Interleaved base samples normalized to 0..=65535. Gray has one sample per
    /// pixel and RGB has three. JPEG never creates a printing Spot channel here.
    pub samples: Vec<u16>,
    /// Embedded ICC payload reassembled by the JPEG decoder. Missing ICC remains
    /// missing; production preflight must require an explicit source interpretation.
    pub icc_profile: Option<Vec<u8>>,
    pub coding_process: JpegCodingProcess,
}

impl DecodedJpegSource {
    pub fn is_lossy(&self) -> bool {
        self.coding_process.is_lossy()
    }
}

pub fn decode_jpeg_source(path: &Path) -> Result<DecodedJpegSource, String> {
    let file = File::open(path).map_err(|err| format!("Cannot open JPEG source: {err}"))?;
    decode_jpeg_reader(BufReader::new(file))
}

pub fn decode_jpeg_reader<R: Read>(reader: R) -> Result<DecodedJpegSource, String> {
    let mut decoder = Decoder::new(reader);
    let pixels = decoder
        .decode()
        .map_err(|err| format!("Cannot decode JPEG source: {err}"))?;
    let info = decoder
        .info()
        .ok_or_else(|| "JPEG decoder returned pixels without image metadata.".to_owned())?;
    let icc_profile = decoder.icc_profile();
    let coding_process = JpegCodingProcess::from(info.coding_process);
    let (model, bit_depth, samples) = normalize_decoded_samples(
        &pixels,
        info.width,
        info.height,
        info.pixel_format,
    )?;

    Ok(DecodedJpegSource {
        width: u32::from(info.width),
        height: u32::from(info.height),
        bit_depth,
        model,
        samples,
        icc_profile,
        coding_process,
    })
}

fn normalize_decoded_samples(
    pixels: &[u8],
    width: u16,
    height: u16,
    pixel_format: PixelFormat,
) -> Result<(JpegSourceModel, u8, Vec<u16>), String> {
    let pixel_count = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| "JPEG dimensions are too large.".to_owned())?;
    let (model, channels) = match pixel_format {
        PixelFormat::L8 => (JpegSourceModel::Gray, 1usize),
        PixelFormat::RGB24 => (JpegSourceModel::Rgb, 3usize),
        PixelFormat::CMYK32 => {
            return Err(
                "CMYK JPEG is not accepted by the RGB/design-source JPEG path; use a supported RGB source or a dedicated CMYK source workflow."
                    .to_owned(),
            );
        }
        PixelFormat::L16 => {
            return Err(
                "16-bit lossless grayscale JPEG is not yet accepted by the design-source JPEG path."
                    .to_owned(),
            );
        }
    };
    let expected_samples = pixel_count
        .checked_mul(channels)
        .ok_or_else(|| "JPEG sample count is too large.".to_owned())?;
    if pixels.len() != expected_samples {
        return Err(format!(
            "JPEG decoded sample count mismatch: {} bytes, expected {expected_samples} for {width}x{height} {pixel_format:?}.",
            pixels.len()
        ));
    }
    let samples = pixels
        .iter()
        .map(|value| u16::from(*value) * 257)
        .collect();
    Ok((model, 8, samples))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    use super::*;

    const RGB_BLACK_JPEG: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/2wBDAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/wAARCAABAAEDAREAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwD/AD/6AP/Z";
    const GRAY_128_JPEG: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/wAALCAABAAEBAREA/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/9oACAEBAAA/ACv/2Q==";
    const CMYK_JPEG: &str = "/9j/7gAOQWRvYmUAZAAAAAAA/9sAQwABAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB/8AAFAgAAQABBEMRAE0RAFkRAEsRAP/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/aAA4EQwBNAFkASwAAPwD+/iv7+K/v4r+/iv/Z";

    fn fixture(encoded: &str) -> Vec<u8> {
        STANDARD.decode(encoded).expect("valid base64 fixture")
    }

    fn with_single_icc_segment(jpeg: &[u8], payload: &[u8]) -> Vec<u8> {
        assert!(jpeg.starts_with(&[0xff, 0xd8]));
        let segment_len = 2usize + 14 + payload.len();
        let segment_len = u16::try_from(segment_len).expect("small ICC test segment");
        let mut out = Vec::with_capacity(jpeg.len() + usize::from(segment_len) + 2);
        out.extend_from_slice(&jpeg[..2]);
        out.extend_from_slice(&[0xff, 0xe2]);
        out.extend_from_slice(&segment_len.to_be_bytes());
        out.extend_from_slice(b"ICC_PROFILE\0");
        out.extend_from_slice(&[1, 1]);
        out.extend_from_slice(payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn rgb_jpeg_decodes_to_16_bit_working_samples_and_preserves_icc() {
        let encoded = with_single_icc_segment(&fixture(RGB_BLACK_JPEG), b"shade-test-icc");
        let decoded = decode_jpeg_reader(Cursor::new(encoded)).expect("decode RGB JPEG");
        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert_eq!(decoded.model, JpegSourceModel::Rgb);
        assert_eq!(decoded.bit_depth, 8);
        assert_eq!(decoded.samples, [0, 0, 0]);
        assert_eq!(decoded.icc_profile.as_deref(), Some(b"shade-test-icc".as_slice()));
        assert_eq!(decoded.coding_process, JpegCodingProcess::DctSequential);
        assert!(decoded.is_lossy());
    }

    #[test]
    fn grayscale_jpeg_is_one_design_channel_not_a_spot_channel() {
        let decoded = decode_jpeg_reader(Cursor::new(fixture(GRAY_128_JPEG)))
            .expect("decode grayscale JPEG");
        assert_eq!(decoded.model, JpegSourceModel::Gray);
        assert_eq!(decoded.bit_depth, 8);
        assert_eq!(decoded.samples, [128 * 257]);
        assert!(decoded.icc_profile.is_none());
        assert!(decoded.is_lossy());
    }

    #[test]
    fn cmyk_jpeg_fails_closed_in_rgb_design_source_path() {
        let error = decode_jpeg_reader(Cursor::new(fixture(CMYK_JPEG)))
            .expect_err("CMYK JPEG must be rejected");
        assert!(error.contains("CMYK JPEG"), "{error}");
    }

    #[test]
    fn malformed_input_is_rejected() {
        let error = decode_jpeg_reader(Cursor::new([0xff, 0xd8, 0xff, 0xd9]))
            .expect_err("truncated JPEG must fail");
        assert!(error.contains("Cannot decode JPEG source"), "{error}");
    }

    #[test]
    fn normalization_rejects_sample_count_mismatch_and_l16() {
        assert!(normalize_decoded_samples(&[0, 1], 1, 1, PixelFormat::RGB24).is_err());
        let error = normalize_decoded_samples(&[0, 0], 1, 1, PixelFormat::L16)
            .expect_err("L16 policy must be explicit");
        assert!(error.contains("16-bit lossless grayscale JPEG"));
    }

    #[test]
    fn coding_process_lossiness_is_explicit() {
        assert!(JpegCodingProcess::DctSequential.is_lossy());
        assert!(JpegCodingProcess::DctProgressive.is_lossy());
        assert!(!JpegCodingProcess::Lossless.is_lossy());
    }
}