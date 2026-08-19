use std::path::Path;
use std::sync::Arc;

use crate::tiff_io::{self, ColorModel as TiffColorModel, PreviewFace, TiffMetadata};
use windows_shade_editor::design_source::{
    DesignSourceColorModel, SourceImageFormat, SourceLossiness, TransparencyState,
};
use windows_shade_editor::design_source_preview::{
    DesignSourcePreview, OwnedDesignSourceDescriptor,
};
use windows_shade_editor::jpeg_source::decode_jpeg_source;
use windows_shade_editor::png_source::decode_png_source;

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

/// Owned preview carrier used by application Faces. The TIFF variant retains the
/// real TIFF metadata needed for existing Spot/extra-channel behavior. The
/// Design variant owns normalized PNG/JPEG preview planes and source metadata
/// without fabricating TIFF state.
#[derive(Clone, Debug)]
pub enum RuntimePreview {
    Tiff(PreviewFace),
    Design(DesignSourcePreview),
}

impl RuntimePreview {
    pub fn load(path: &Path, max_dimension: u32) -> Result<Self, String> {
        match source_format_from_path(path) {
            Some(SourceImageFormat::Tiff) => tiff_io::load_preview(path, max_dimension)
                .map(Self::Tiff)
                .map_err(|err| format!("Cannot load TIFF source {}: {err}", path.display())),
            Some(SourceImageFormat::Png) => {
                let decoded = decode_png_source(path)?;
                DesignSourcePreview::from_png(&decoded, max_dimension).map(Self::Design)
            }
            Some(SourceImageFormat::Jpeg) => {
                let decoded = decode_jpeg_source(path)?;
                DesignSourcePreview::from_jpeg(&decoded, max_dimension).map(Self::Design)
            }
            None => Err(format!(
                "Unsupported design source extension for {}. Use TIFF, PNG or JPEG.",
                path.display()
            )),
        }
    }

    pub fn source_descriptor(&self) -> OwnedDesignSourceDescriptor {
        match self {
            Self::Tiff(preview) => {
                let color_model = match preview.metadata.color_model {
                    TiffColorModel::Gray => DesignSourceColorModel::Gray,
                    TiffColorModel::Rgb => DesignSourceColorModel::Rgb,
                    TiffColorModel::Cmyk => DesignSourceColorModel::Cmyk,
                    TiffColorModel::Other => DesignSourceColorModel::Other,
                };
                OwnedDesignSourceDescriptor {
                    format: SourceImageFormat::Tiff,
                    color_model,
                    bit_depth: preview.metadata.bit_depth,
                    channel_count: preview.metadata.samples_per_pixel,
                    embedded_icc: preview.metadata.icc_profile.clone(),
                    transparency: TransparencyState::None,
                    lossiness: SourceLossiness::Lossless,
                }
            }
            Self::Design(preview) => preview.source.clone(),
        }
    }

    pub fn source_dimensions(&self) -> (u32, u32) {
        match self {
            Self::Tiff(preview) => (preview.metadata.width, preview.metadata.height),
            Self::Design(preview) => (preview.source_width, preview.source_height),
        }
    }

    pub fn as_tiff(&self) -> Option<&PreviewFace> {
        match self {
            Self::Tiff(preview) => Some(preview),
            Self::Design(_) => None,
        }
    }

    pub fn is_tiff(&self) -> bool {
        matches!(self, Self::Tiff(_))
    }
}

impl RuntimePreviewSource for RuntimePreview {
    fn width(&self) -> usize {
        match self {
            Self::Tiff(preview) => preview.width(),
            Self::Design(preview) => preview.width(),
        }
    }

    fn height(&self) -> usize {
        match self {
            Self::Tiff(preview) => preview.height(),
            Self::Design(preview) => preview.height(),
        }
    }

    fn channel_names(&self) -> &[String] {
        match self {
            Self::Tiff(preview) => preview.channel_names(),
            Self::Design(preview) => preview.channel_names(),
        }
    }

    fn channels(&self) -> &[Vec<u16>] {
        match self {
            Self::Tiff(preview) => preview.channels(),
            Self::Design(preview) => preview.channels(),
        }
    }

    fn histograms(&self) -> &[[u32; 256]] {
        match self {
            Self::Tiff(preview) => preview.histograms(),
            Self::Design(preview) => preview.histograms(),
        }
    }

    fn color_model(&self) -> RuntimeColorModel {
        match self {
            Self::Tiff(preview) => preview.color_model(),
            Self::Design(preview) => preview.color_model(),
        }
    }

    fn embedded_icc(&self) -> Option<&[u8]> {
        match self {
            Self::Tiff(preview) => preview.embedded_icc(),
            Self::Design(preview) => preview.embedded_icc(),
        }
    }

    fn alpha(&self) -> Option<&[u16]> {
        match self {
            Self::Tiff(preview) => preview.alpha(),
            Self::Design(preview) => preview.alpha(),
        }
    }

    fn tiff_metadata(&self) -> Option<&TiffMetadata> {
        match self {
            Self::Tiff(preview) => preview.tiff_metadata(),
            Self::Design(preview) => preview.tiff_metadata(),
        }
    }
}

pub fn source_format_from_path(path: &Path) -> Option<SourceImageFormat> {
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("tif") || extension.eq_ignore_ascii_case("tiff") {
        Some(SourceImageFormat::Tiff)
    } else if extension.eq_ignore_ascii_case("png") {
        Some(SourceImageFormat::Png)
    } else if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        Some(SourceImageFormat::Jpeg)
    } else {
        None
    }
}

pub fn is_supported_design_source_path(path: &Path) -> bool {
    source_format_from_path(path).is_some()
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

    #[test]
    fn path_dispatch_is_case_insensitive_and_rejects_other_formats() {
        assert_eq!(
            source_format_from_path(Path::new("Face.TIFF")),
            Some(SourceImageFormat::Tiff)
        );
        assert_eq!(
            source_format_from_path(Path::new("Face.PnG")),
            Some(SourceImageFormat::Png)
        );
        assert_eq!(
            source_format_from_path(Path::new("Face.JPEG")),
            Some(SourceImageFormat::Jpeg)
        );
        assert!(source_format_from_path(Path::new("Face.webp")).is_none());
    }

    #[test]
    fn runtime_carrier_preserves_png_descriptor_without_tiff_metadata() {
        let decoded = DecodedPngSource {
            width: 2,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![1, 2, 3, 4, 5, 6],
            alpha: Some(vec![7, 8]),
            icc_profile: Some(vec![9, 10]),
            declares_srgb: false,
        };
        let design = DesignSourcePreview::from_png(&decoded, 512).expect("PNG preview");
        let preview = RuntimePreview::Design(design);
        let descriptor = preview.source_descriptor();
        assert_eq!(descriptor.format, SourceImageFormat::Png);
        assert_eq!(descriptor.bit_depth, 16);
        assert_eq!(descriptor.channel_count, 3);
        assert_eq!(preview.source_dimensions(), (2, 1));
        assert_eq!(preview.alpha(), Some(&[7, 8][..]));
        assert!(preview.tiff_metadata().is_none());
        assert!(!preview.is_tiff());
    }
}