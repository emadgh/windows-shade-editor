use crate::design_source::{
    DesignSourceColorModel, DesignSourceDescriptor, SourceImageFormat, SourceLossiness,
    TransparencyState,
};
use crate::jpeg_source::DecodedJpegSource;
use crate::png_source::DecodedPngSource;

/// Owned form of the format-neutral source descriptor suitable for runtime
/// preview state. It retains source interpretation metadata without importing
/// TIFF writer/export semantics into PNG/JPEG design sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedDesignSourceDescriptor {
    pub format: SourceImageFormat,
    pub color_model: DesignSourceColorModel,
    pub bit_depth: u8,
    pub channel_count: usize,
    pub embedded_icc: Option<Vec<u8>>,
    pub transparency: TransparencyState,
    pub lossiness: SourceLossiness,
}

impl OwnedDesignSourceDescriptor {
    pub fn from_borrowed(source: DesignSourceDescriptor<'_>) -> Self {
        Self {
            format: source.format,
            color_model: source.color_model,
            bit_depth: source.bit_depth,
            channel_count: source.channel_count,
            embedded_icc: source.embedded_icc.map(ToOwned::to_owned),
            transparency: source.transparency,
            lossiness: source.lossiness,
        }
    }

    pub fn from_png(decoded: &DecodedPngSource) -> Self {
        Self::from_borrowed(DesignSourceDescriptor::from_png(decoded))
    }

    pub fn from_jpeg(decoded: &DecodedJpegSource) -> Self {
        Self::from_borrowed(DesignSourceDescriptor::from_jpeg(decoded))
    }

    pub fn as_borrowed(&self) -> DesignSourceDescriptor<'_> {
        DesignSourceDescriptor::new(
            self.format,
            self.color_model,
            self.bit_depth,
            self.channel_count,
            self.embedded_icc.as_deref(),
            self.transparency,
            self.lossiness,
        )
    }
}

/// Bounded, format-neutral design-source preview raster.
///
/// `channels` contains only base Gray/RGB design channels. PNG alpha is retained
/// independently in `alpha` and can never become a printing channel by accident.
/// All samples use Shade Editor's common 0..=65535 working precision regardless
/// of whether the source container was originally 8- or 16-bit.
#[derive(Clone, Debug)]
pub struct DesignSourcePreview {
    pub source: OwnedDesignSourceDescriptor,
    pub source_width: u32,
    pub source_height: u32,
    pub width: usize,
    pub height: usize,
    pub channel_names: Vec<String>,
    pub channels: Vec<Vec<u16>>,
    pub alpha: Option<Vec<u16>>,
    pub histograms: Vec<[u32; 256]>,
}

impl DesignSourcePreview {
    pub fn from_png(decoded: &DecodedPngSource, max_dimension: u32) -> Result<Self, String> {
        Self::from_interleaved(
            decoded.width,
            decoded.height,
            OwnedDesignSourceDescriptor::from_png(decoded),
            &decoded.samples,
            decoded.alpha.as_deref(),
            max_dimension,
        )
    }

    pub fn from_jpeg(decoded: &DecodedJpegSource, max_dimension: u32) -> Result<Self, String> {
        Self::from_interleaved(
            decoded.width,
            decoded.height,
            OwnedDesignSourceDescriptor::from_jpeg(decoded),
            &decoded.samples,
            None,
            max_dimension,
        )
    }

    fn from_interleaved(
        source_width: u32,
        source_height: u32,
        source: OwnedDesignSourceDescriptor,
        samples: &[u16],
        alpha: Option<&[u16]>,
        max_dimension: u32,
    ) -> Result<Self, String> {
        if source_width == 0 || source_height == 0 {
            return Err("Design source dimensions must be non-zero.".to_owned());
        }
        let channel_names = base_channel_names(source.color_model)?;
        if source.channel_count != channel_names.len() {
            return Err(format!(
                "Design source descriptor declares {} channels but {} requires {}.",
                source.channel_count,
                source.color_model.title(),
                channel_names.len()
            ));
        }
        if !matches!(source.bit_depth, 8 | 16) {
            return Err(format!(
                "Unsupported design source bit depth: {}-bit.",
                source.bit_depth
            ));
        }

        let source_pixels = (source_width as usize)
            .checked_mul(source_height as usize)
            .ok_or_else(|| "Design source dimensions are too large.".to_owned())?;
        let expected_samples = source_pixels
            .checked_mul(source.channel_count)
            .ok_or_else(|| "Design source sample count is too large.".to_owned())?;
        if samples.len() != expected_samples {
            return Err(format!(
                "Design source sample count mismatch: {} samples, expected {expected_samples}.",
                samples.len()
            ));
        }
        if let Some(alpha) = alpha {
            if alpha.len() != source_pixels {
                return Err(format!(
                    "Design source alpha count mismatch: {} samples, expected {source_pixels}.",
                    alpha.len()
                ));
            }
        }
        if source.transparency == TransparencyState::PresentUnresolved && alpha.is_none() {
            return Err(
                "Design source declares unresolved transparency but has no alpha plane.".to_owned(),
            );
        }
        if alpha.is_some() && source.transparency == TransparencyState::None {
            return Err(
                "Design source carries alpha samples but descriptor declares no transparency."
                    .to_owned(),
            );
        }

        let (width, height, source_x, source_y) = preview_sampling_map(
            source_width as usize,
            source_height as usize,
            max_dimension,
        )?;
        let preview_pixels = width
            .checked_mul(height)
            .ok_or_else(|| "Design source preview dimensions are too large.".to_owned())?;
        let mut channels = (0..source.channel_count)
            .map(|_| Vec::with_capacity(preview_pixels))
            .collect::<Vec<_>>();
        let mut preview_alpha = alpha.map(|_| Vec::with_capacity(preview_pixels));

        for &source_y in &source_y {
            for &source_x in &source_x {
                let source_pixel = source_y
                    .checked_mul(source_width as usize)
                    .and_then(|base| base.checked_add(source_x))
                    .ok_or_else(|| "Design source preview index overflow.".to_owned())?;
                let source_base = source_pixel
                    .checked_mul(source.channel_count)
                    .ok_or_else(|| "Design source preview sample index overflow.".to_owned())?;
                for channel in 0..source.channel_count {
                    channels[channel].push(samples[source_base + channel]);
                }
                if let (Some(source_alpha), Some(output_alpha)) = (alpha, preview_alpha.as_mut()) {
                    output_alpha.push(source_alpha[source_pixel]);
                }
            }
        }

        if channels.iter().any(|plane| plane.len() != preview_pixels)
            || preview_alpha
                .as_ref()
                .is_some_and(|plane| plane.len() != preview_pixels)
        {
            return Err("Design source preview plane construction was incomplete.".to_owned());
        }
        let histograms = channels.iter().map(|plane| histogram(plane)).collect();

        Ok(Self {
            source,
            source_width,
            source_height,
            width,
            height,
            channel_names,
            channels,
            alpha: preview_alpha,
            histograms,
        })
    }
}

fn base_channel_names(model: DesignSourceColorModel) -> Result<Vec<String>, String> {
    match model {
        DesignSourceColorModel::Gray => Ok(vec!["Gray".to_owned()]),
        DesignSourceColorModel::Rgb => Ok(vec![
            "Red".to_owned(),
            "Green".to_owned(),
            "Blue".to_owned(),
        ]),
        DesignSourceColorModel::Cmyk | DesignSourceColorModel::Other => Err(format!(
            "{} design-source preview normalization is not enabled by this PNG/JPEG path.",
            model.title()
        )),
    }
}

fn preview_sampling_map(
    source_width: usize,
    source_height: usize,
    max_dimension: u32,
) -> Result<(usize, usize, Vec<usize>, Vec<usize>), String> {
    if source_width == 0 || source_height == 0 {
        return Err("Design source dimensions must be non-zero.".to_owned());
    }
    let max_source = source_width.max(source_height).max(1);
    // Match the existing TIFF preview floor so an unusually small preference
    // cannot collapse working previews into unusably tiny rasters.
    let max_dimension = max_dimension.max(256) as usize;
    let scale = (max_source as f64 / max_dimension as f64).max(1.0);
    let width = ((source_width as f64 / scale).round() as usize).max(1);
    let height = ((source_height as f64 / scale).round() as usize).max(1);
    let preview_pixels = width
        .checked_mul(height)
        .ok_or_else(|| "Design source preview dimensions are too large.".to_owned())?;
    if preview_pixels > 268_435_456 {
        return Err("Design source preview is unreasonably large.".to_owned());
    }

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
    Ok((width, height, source_x, source_y))
}

fn histogram(values: &[u16]) -> [u32; 256] {
    let mut bins = [0u32; 256];
    for &value in values {
        let index = usize::from(value >> 8);
        bins[index] = bins[index].saturating_add(1);
    }
    bins
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jpeg_source::{JpegCodingProcess, JpegSourceModel};
    use crate::png_source::PngSourceModel;

    #[test]
    fn png_rgb_interleaved_samples_become_three_base_planes() {
        let decoded = DecodedPngSource {
            width: 2,
            height: 1,
            bit_depth: 8,
            model: PngSourceModel::Rgb,
            samples: vec![1, 2, 3, 10, 20, 30],
            alpha: None,
            icc_profile: Some(vec![1, 2, 3]),
            declares_srgb: false,
        };
        let preview = DesignSourcePreview::from_png(&decoded, 512).expect("PNG preview");
        assert_eq!(preview.width, 2);
        assert_eq!(preview.height, 1);
        assert_eq!(preview.channel_names, ["Red", "Green", "Blue"]);
        assert_eq!(preview.channels[0], [1, 10]);
        assert_eq!(preview.channels[1], [2, 20]);
        assert_eq!(preview.channels[2], [3, 30]);
        assert!(preview.alpha.is_none());
        assert_eq!(preview.source.format, SourceImageFormat::Png);
        assert_eq!(preview.source.embedded_icc.as_deref(), Some(&[1, 2, 3][..]));
    }

    #[test]
    fn png_alpha_remains_separate_from_printing_channel_topology() {
        let decoded = DecodedPngSource {
            width: 2,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![100, 200, 300, 400, 500, 600],
            alpha: Some(vec![0x1234, 0xabcd]),
            icc_profile: None,
            declares_srgb: true,
        };
        let preview = DesignSourcePreview::from_png(&decoded, 512).expect("RGBA preview");
        assert_eq!(preview.channels.len(), 3);
        assert_eq!(preview.channel_names.len(), 3);
        assert_eq!(preview.alpha.as_deref(), Some(&[0x1234, 0xabcd][..]));
        assert_eq!(
            preview.source.transparency,
            TransparencyState::PresentUnresolved
        );
        assert!(preview.source.embedded_icc.is_none());
    }

    #[test]
    fn jpeg_gray_stays_one_base_plane_and_preserves_lossiness() {
        let decoded = DecodedJpegSource {
            width: 3,
            height: 1,
            bit_depth: 8,
            model: JpegSourceModel::Gray,
            samples: vec![0, 32768, u16::MAX],
            icc_profile: None,
            coding_process: JpegCodingProcess::DctSequential,
        };
        let preview = DesignSourcePreview::from_jpeg(&decoded, 512).expect("Gray JPEG preview");
        assert_eq!(preview.channel_names, ["Gray"]);
        assert_eq!(preview.channels, [vec![0, 32768, u16::MAX]]);
        assert_eq!(preview.source.lossiness, SourceLossiness::Lossy);
        assert!(preview.alpha.is_none());
        assert_eq!(preview.histograms[0][0], 1);
        assert_eq!(preview.histograms[0][128], 1);
        assert_eq!(preview.histograms[0][255], 1);
    }

    #[test]
    fn preview_dimensions_match_existing_tiff_sampling_policy() {
        let width = 512u32;
        let height = 256u32;
        let pixels = (width as usize) * (height as usize);
        let decoded = DecodedPngSource {
            width,
            height,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![0; pixels * 3],
            alpha: None,
            icc_profile: None,
            declares_srgb: false,
        };
        let preview = DesignSourcePreview::from_png(&decoded, 256).expect("bounded preview");
        assert_eq!((preview.width, preview.height), (256, 128));
        assert_eq!(preview.channels[0].len(), 256 * 128);
    }

    #[test]
    fn malformed_base_or_alpha_sample_counts_are_rejected() {
        let bad_samples = DecodedPngSource {
            width: 2,
            height: 1,
            bit_depth: 8,
            model: PngSourceModel::Rgb,
            samples: vec![1, 2, 3],
            alpha: None,
            icc_profile: None,
            declares_srgb: false,
        };
        assert!(DesignSourcePreview::from_png(&bad_samples, 512).is_err());

        let bad_alpha = DecodedPngSource {
            samples: vec![1, 2, 3, 4, 5, 6],
            alpha: Some(vec![u16::MAX]),
            ..bad_samples
        };
        assert!(DesignSourcePreview::from_png(&bad_alpha, 512).is_err());
    }

    #[test]
    fn descriptor_round_trip_keeps_source_preflight_metadata() {
        let decoded = DecodedJpegSource {
            width: 1,
            height: 1,
            bit_depth: 8,
            model: JpegSourceModel::Rgb,
            samples: vec![1, 2, 3],
            icc_profile: Some(vec![9, 8, 7]),
            coding_process: JpegCodingProcess::DctProgressive,
        };
        let preview = DesignSourcePreview::from_jpeg(&decoded, 512).expect("JPEG preview");
        let borrowed = preview.source.as_borrowed();
        assert_eq!(borrowed.format, SourceImageFormat::Jpeg);
        assert_eq!(borrowed.color_model, DesignSourceColorModel::Rgb);
        assert_eq!(borrowed.bit_depth, 8);
        assert_eq!(borrowed.channel_count, 3);
        assert_eq!(borrowed.embedded_icc, Some(&[9, 8, 7][..]));
        assert_eq!(borrowed.lossiness, SourceLossiness::Lossy);
    }
}
