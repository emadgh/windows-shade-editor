use sha2::{Digest, Sha256};

use crate::jpeg_source::{DecodedJpegSource, JpegSourceModel};
use crate::png_source::{DecodedPngSource, PngSourceModel};
use crate::tiff_io::{ColorModel, TiffMetadata};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceImageFormat {
    Tiff,
    Png,
    Jpeg,
}

impl SourceImageFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tiff => "TIFF",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesignSourceColorModel {
    Gray,
    Rgb,
    Cmyk,
    Other,
}

impl DesignSourceColorModel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Gray => "Gray",
            Self::Rgb => "RGB",
            Self::Cmyk => "CMYK",
            Self::Other => "Multichannel",
        }
    }
}

impl From<ColorModel> for DesignSourceColorModel {
    fn from(value: ColorModel) -> Self {
        match value {
            ColorModel::Gray => Self::Gray,
            ColorModel::Rgb => Self::Rgb,
            ColorModel::Cmyk => Self::Cmyk,
            ColorModel::Other => Self::Other,
        }
    }
}

impl From<PngSourceModel> for DesignSourceColorModel {
    fn from(value: PngSourceModel) -> Self {
        match value {
            PngSourceModel::Gray => Self::Gray,
            PngSourceModel::Rgb => Self::Rgb,
        }
    }
}

impl From<JpegSourceModel> for DesignSourceColorModel {
    fn from(value: JpegSourceModel) -> Self {
        match value {
            JpegSourceModel::Gray => Self::Gray,
            JpegSourceModel::Rgb => Self::Rgb,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransparencyState {
    None,
    PresentUnresolved,
    Flattened,
}

impl TransparencyState {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::PresentUnresolved => "Present / flatten required",
            Self::Flattened => "Flattened",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceLossiness {
    Lossless,
    Lossy,
}

impl SourceLossiness {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lossless => "Lossless",
            Self::Lossy => "Lossy",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceProfileOrigin {
    Assigned,
    Embedded,
    Missing,
}

/// Borrowed, format-neutral source metadata used by production preflight.
///
/// The descriptor intentionally contains no production-output or TIFF-writer state.
/// PNG/JPEG can therefore participate in source interpretation and preflight without
/// being mistaken for a production Face. Embedded ICC bytes are borrowed from the
/// decoded source and are never modified.
#[derive(Clone, Copy, Debug)]
pub struct DesignSourceDescriptor<'a> {
    pub format: SourceImageFormat,
    pub color_model: DesignSourceColorModel,
    pub bit_depth: u8,
    pub channel_count: usize,
    pub embedded_icc: Option<&'a [u8]>,
    pub transparency: TransparencyState,
    pub lossiness: SourceLossiness,
}

impl<'a> DesignSourceDescriptor<'a> {
    pub fn new(
        format: SourceImageFormat,
        color_model: DesignSourceColorModel,
        bit_depth: u8,
        channel_count: usize,
        embedded_icc: Option<&'a [u8]>,
        transparency: TransparencyState,
        lossiness: SourceLossiness,
    ) -> Self {
        Self {
            format,
            color_model,
            bit_depth,
            channel_count,
            embedded_icc,
            transparency,
            lossiness,
        }
    }

    pub fn from_tiff_metadata(metadata: &'a TiffMetadata) -> Self {
        Self::new(
            SourceImageFormat::Tiff,
            metadata.color_model.into(),
            metadata.bit_depth,
            metadata.samples_per_pixel,
            metadata.icc_profile.as_deref(),
            TransparencyState::None,
            SourceLossiness::Lossless,
        )
    }

    pub fn from_png(decoded: &'a DecodedPngSource) -> Self {
        let color_model = DesignSourceColorModel::from(decoded.model);
        let channel_count = match color_model {
            DesignSourceColorModel::Gray => 1,
            DesignSourceColorModel::Rgb => 3,
            DesignSourceColorModel::Cmyk | DesignSourceColorModel::Other => unreachable!(),
        };
        Self::new(
            SourceImageFormat::Png,
            color_model,
            decoded.bit_depth,
            channel_count,
            decoded.icc_profile.as_deref(),
            if decoded.alpha.is_some() {
                TransparencyState::PresentUnresolved
            } else {
                TransparencyState::None
            },
            SourceLossiness::Lossless,
        )
    }

    pub fn from_jpeg(decoded: &'a DecodedJpegSource) -> Self {
        let color_model = DesignSourceColorModel::from(decoded.model);
        let channel_count = match color_model {
            DesignSourceColorModel::Gray => 1,
            DesignSourceColorModel::Rgb => 3,
            DesignSourceColorModel::Cmyk | DesignSourceColorModel::Other => unreachable!(),
        };
        Self::new(
            SourceImageFormat::Jpeg,
            color_model,
            decoded.bit_depth,
            channel_count,
            decoded.icc_profile.as_deref(),
            TransparencyState::None,
            if decoded.is_lossy() {
                SourceLossiness::Lossy
            } else {
                SourceLossiness::Lossless
            },
        )
    }

    pub fn embedded_icc_sha256(self) -> Option<String> {
        self.embedded_icc
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
    }

    /// An explicit assignment always wins over embedded source metadata. This is
    /// interpretation precedence only and never implies a pixel conversion.
    pub fn preferred_profile_origin(self, has_assigned_profile: bool) -> SourceProfileOrigin {
        if has_assigned_profile {
            SourceProfileOrigin::Assigned
        } else if self.embedded_icc.is_some() {
            SourceProfileOrigin::Embedded
        } else {
            SourceProfileOrigin::Missing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jpeg_source::JpegCodingProcess;
    use crate::tiff_io::PhotoshopChannelDisplay;

    fn tiff_metadata(icc_profile: Option<Vec<u8>>) -> TiffMetadata {
        TiffMetadata {
            width: 16,
            height: 8,
            bit_depth: 16,
            samples_per_pixel: 3,
            base_channel_count: 3,
            color_model: ColorModel::Rgb,
            non_cmyk_separated: false,
            channel_names: vec!["Red".into(), "Green".into(), "Blue".into()],
            channel_display_info: vec![None::<PhotoshopChannelDisplay>; 3],
            compression: None,
            predictor: None,
            orientation: None,
            icc_profile,
            photoshop_resources: None,
            photoshop_image_source_data: None,
        }
    }

    #[test]
    fn tiff_descriptor_preserves_actual_source_metadata() {
        let metadata = tiff_metadata(Some(vec![1, 2, 3, 4]));
        let source = DesignSourceDescriptor::from_tiff_metadata(&metadata);
        assert_eq!(source.format, SourceImageFormat::Tiff);
        assert_eq!(source.color_model, DesignSourceColorModel::Rgb);
        assert_eq!(source.bit_depth, 16);
        assert_eq!(source.channel_count, 3);
        assert_eq!(source.transparency, TransparencyState::None);
        assert_eq!(source.lossiness, SourceLossiness::Lossless);
        assert!(source.embedded_icc_sha256().is_some());
    }

    #[test]
    fn png_precision_and_alpha_are_explicit() {
        let png8 = DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 8,
            model: PngSourceModel::Rgb,
            samples: vec![0, 0, 0],
            alpha: None,
            icc_profile: None,
            declares_srgb: true,
        };
        let source8 = DesignSourceDescriptor::from_png(&png8);
        assert_eq!(source8.format, SourceImageFormat::Png);
        assert_eq!(source8.bit_depth, 8);
        assert_eq!(source8.channel_count, 3);
        assert_eq!(source8.transparency, TransparencyState::None);
        assert!(
            source8.embedded_icc.is_none(),
            "sRGB chunk must not become an ICC assignment"
        );

        let png16_alpha = DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![0, 0, 0],
            alpha: Some(vec![u16::MAX]),
            icc_profile: Some(vec![9, 8, 7]),
            declares_srgb: false,
        };
        let source16 = DesignSourceDescriptor::from_png(&png16_alpha);
        assert_eq!(source16.bit_depth, 16);
        assert_eq!(source16.transparency, TransparencyState::PresentUnresolved);
        assert!(source16.embedded_icc.is_some());
    }

    #[test]
    fn jpeg_coding_process_controls_lossiness_without_guessing() {
        let lossy = DecodedJpegSource {
            width: 1,
            height: 1,
            bit_depth: 8,
            model: JpegSourceModel::Rgb,
            samples: vec![0, 0, 0],
            icc_profile: None,
            coding_process: JpegCodingProcess::DctProgressive,
        };
        assert_eq!(
            DesignSourceDescriptor::from_jpeg(&lossy).lossiness,
            SourceLossiness::Lossy
        );

        let lossless = DecodedJpegSource {
            coding_process: JpegCodingProcess::Lossless,
            icc_profile: Some(vec![1, 3, 3, 7]),
            ..lossy
        };
        let source = DesignSourceDescriptor::from_jpeg(&lossless);
        assert_eq!(source.lossiness, SourceLossiness::Lossless);
        assert!(source.embedded_icc_sha256().is_some());
    }

    #[test]
    fn assigned_profile_has_precedence_over_embedded_profile() {
        let metadata = tiff_metadata(Some(vec![1, 2, 3]));
        let source = DesignSourceDescriptor::from_tiff_metadata(&metadata);
        assert_eq!(
            source.preferred_profile_origin(true),
            SourceProfileOrigin::Assigned
        );
        assert_eq!(
            source.preferred_profile_origin(false),
            SourceProfileOrigin::Embedded
        );

        let metadata = tiff_metadata(None);
        let source = DesignSourceDescriptor::from_tiff_metadata(&metadata);
        assert_eq!(
            source.preferred_profile_origin(false),
            SourceProfileOrigin::Missing
        );
    }
}
