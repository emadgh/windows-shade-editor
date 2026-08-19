use std::sync::Arc;

use crate::tiff_io::{ColorModel as TiffColorModel, PreviewFace, TiffMetadata};
use windows_shade_editor::design_source::DesignSourceColorModel;
use windows_shade_editor::design_source_preview::DesignSourcePreview;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeColorModel {
    Gray,
    Rgb,
    Cmyk,
    Other,
}

impl RuntimeColorModel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Gray => "Gray",
            Self::Rgb => "RGB",
            Self::Cmyk => "CMYK",
            Self::Other => "Multichannel",
        }
    }
}

impl From<TiffColorModel> for RuntimeColorModel {
    fn from(value: TiffColorModel) -> Self {
        match value {
            TiffColorModel::Gray => Self::Gray,
            TiffColorModel::Rgb => Self::Rgb,
            TiffColorModel::Cmyk => Self::Cmyk,
            TiffColorModel::Other => Self::Other,
        }
    }
}

impl From<DesignSourceColorModel> for RuntimeColorModel {
    fn from(value: DesignSourceColorModel) -> Self {
        match value {
            DesignSourceColorModel::Gray => Self::Gray,
            DesignSourceColorModel::Rgb => Self::Rgb,
            DesignSourceColorModel::Cmyk => Self::Cmyk,
            DesignSourceColorModel::Other => Self::Other,
        }
    }
}

/// Borrowed, source-format-neutral preview contract used by adjustment,
/// viewport rendering and preview-only ICC color management.
///
/// TIFF-specific Photoshop Spot metadata is exposed only through
/// `tiff_metadata()`. PNG/JPEG never synthesize a `TiffMetadata` value. Their
/// alpha plane remains separate from `channels()` so it cannot become a printing
/// channel or adjustment target accidentally.
pub trait RuntimePreviewSource {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn channel_names(&self) -> &[String];
    fn channels(&self) -> &[Vec<u16>];
    fn histograms(&self) -> &[[u32; 256]];
    fn color_model(&self) -> RuntimeColorModel;
    fn embedded_icc(&self) -> Option<&[u8]>;

    fn alpha(&self) -> Option<&[u16]> {
        None
    }

    fn tiff_metadata(&self) -> Option<&TiffMetadata> {
        None
    }
}

impl<T: RuntimePreviewSource + ?Sized> RuntimePreviewSource for Arc<T> {
    fn width(&self) -> usize {
        self.as_ref().width()
    }

    fn height(&self) -> usize {
        self.as_ref().height()
    }

    fn channel_names(&self) -> &[String] {
        self.as_ref().channel_names()
    }

    fn channels(&self) -> &[Vec<u16>] {
        self.as_ref().channels()
    }

    fn histograms(&self) -> &[[u32; 256]] {
        self.as_ref().histograms()
    }

    fn color_model(&self) -> RuntimeColorModel {
        self.as_ref().color_model()
    }

    fn embedded_icc(&self) -> Option<&[u8]> {
        self.as_ref().embedded_icc()
    }

    fn alpha(&self) -> Option<&[u16]> {
        self.as_ref().alpha()
    }

    fn tiff_metadata(&self) -> Option<&TiffMetadata> {
        self.as_ref().tiff_metadata()
    }
}

impl RuntimePreviewSource for PreviewFace {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn channel_names(&self) -> &[String] {
        &self.metadata.channel_names
    }

    fn channels(&self) -> &[Vec<u16>] {
        &self.channels
    }

    fn histograms(&self) -> &[[u32; 256]] {
        &self.histograms
    }

    fn color_model(&self) -> RuntimeColorModel {
        self.metadata.color_model.into()
    }

    fn embedded_icc(&self) -> Option<&[u8]> {
        self.metadata.icc_profile.as_deref()
    }

    fn tiff_metadata(&self) -> Option<&TiffMetadata> {
        Some(&self.metadata)
    }
}

impl RuntimePreviewSource for DesignSourcePreview {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn channel_names(&self) -> &[String] {
        &self.channel_names
    }

    fn channels(&self) -> &[Vec<u16>] {
        &self.channels
    }

    fn histograms(&self) -> &[[u32; 256]] {
        &self.histograms
    }

    fn color_model(&self) -> RuntimeColorModel {
        self.source.color_model.into()
    }

    fn embedded_icc(&self) -> Option<&[u8]> {
        self.source.embedded_icc.as_deref()
    }

    fn alpha(&self) -> Option<&[u16]> {
        self.alpha.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_shade_editor::png_source::{DecodedPngSource, PngSourceModel};

    #[test]
    fn png_runtime_view_keeps_alpha_outside_channel_topology() {
        let decoded = DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![100, 200, 300],
            alpha: Some(vec![400]),
            icc_profile: Some(vec![1, 2, 3]),
            declares_srgb: false,
        };
        let preview = DesignSourcePreview::from_png(&decoded, 512).expect("PNG preview");
        assert_eq!(
            RuntimePreviewSource::color_model(&preview),
            RuntimeColorModel::Rgb
        );
        assert_eq!(RuntimePreviewSource::channels(&preview).len(), 3);
        assert_eq!(
            RuntimePreviewSource::channel_names(&preview),
            ["Red", "Green", "Blue"]
        );
        assert_eq!(RuntimePreviewSource::alpha(&preview), Some(&[400][..]));
        assert_eq!(
            RuntimePreviewSource::embedded_icc(&preview),
            Some(&[1, 2, 3][..])
        );
        assert!(RuntimePreviewSource::tiff_metadata(&preview).is_none());
    }

    #[test]
    fn arc_delegates_runtime_preview_contract() {
        let decoded = DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Gray,
            samples: vec![123],
            alpha: None,
            icc_profile: None,
            declares_srgb: false,
        };
        let preview = Arc::new(DesignSourcePreview::from_png(&decoded, 512).expect("PNG preview"));
        assert_eq!(RuntimePreviewSource::width(&preview), 1);
        assert_eq!(RuntimePreviewSource::channels(&preview), &[vec![123]]);
        assert_eq!(
            RuntimePreviewSource::color_model(&preview),
            RuntimeColorModel::Gray
        );
    }
}
