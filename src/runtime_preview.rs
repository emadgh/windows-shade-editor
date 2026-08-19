use std::path::Path;
use std::sync::Arc;

use crate::model::FaceFileMetadata;
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

#[derive(Clone, Debug)]
pub struct MissingPreview {
    source_format: Option<SourceImageFormat>,
    source_width: u32,
    source_height: u32,
    bit_depth: u8,
    base_channel_count: usize,
    color_model: RuntimeColorModel,
    channel_names: Vec<String>,
    channels: Vec<Vec<u16>>,
    histograms: Vec<[u32; 256]>,
}

impl MissingPreview {
    fn from_cached(path: &Path, expected: Option<&FaceFileMetadata>) -> Self {
        let mut channel_names = expected
            .map(|metadata| metadata.channel_names.clone())
            .filter(|names| !names.is_empty())
            .unwrap_or_else(|| vec!["Missing source".to_owned()]);
        let channel_count = expected
            .map(|metadata| metadata.channel_count)
            .unwrap_or(channel_names.len())
            .max(channel_names.len())
            .max(1);
        while channel_names.len() < channel_count {
            channel_names.push(format!("Channel {}", channel_names.len() + 1));
        }
        channel_names.truncate(channel_count);
        let base_channel_count = expected
            .map(|metadata| metadata.base_channel_count)
            .unwrap_or(1)
            .clamp(1, channel_count);
        let color_model = expected
            .map(|metadata| runtime_color_model_from_cached(&metadata.color_model))
            .unwrap_or(RuntimeColorModel::Other);
        Self {
            source_format: source_format_from_path(path),
            source_width: expected.map(|metadata| metadata.width).unwrap_or(1).max(1),
            source_height: expected.map(|metadata| metadata.height).unwrap_or(1).max(1),
            bit_depth: expected.map(|metadata| metadata.bit_depth).unwrap_or(8),
            base_channel_count,
            color_model,
            channel_names,
            channels: (0..channel_count).map(|_| vec![0u16]).collect(),
            histograms: vec![[0u32; 256]; channel_count],
        }
    }
}

impl RuntimePreviewSource for MissingPreview {
    fn width(&self) -> usize {
        1
    }
    fn height(&self) -> usize {
        1
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
        self.color_model
    }
    fn embedded_icc(&self) -> Option<&[u8]> {
        None
    }
}

/// Owned preview carrier used by application Faces. The TIFF variant retains the
/// real TIFF metadata needed for existing Spot/extra-channel behavior. The Design
/// variant owns normalized PNG/JPEG preview planes and source metadata without
/// fabricating TIFF state. Missing keeps cached project metadata without pretending
/// the unavailable source was a TIFF.
#[derive(Clone, Debug)]
pub enum RuntimePreview {
    Tiff(PreviewFace),
    Design(DesignSourcePreview),
    Missing(MissingPreview),
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

    pub fn source_descriptor(&self) -> Option<OwnedDesignSourceDescriptor> {
        match self {
            Self::Tiff(preview) => {
                let color_model = match preview.metadata.color_model {
                    TiffColorModel::Gray => DesignSourceColorModel::Gray,
                    TiffColorModel::Rgb => DesignSourceColorModel::Rgb,
                    TiffColorModel::Cmyk => DesignSourceColorModel::Cmyk,
                    TiffColorModel::Other => DesignSourceColorModel::Other,
                };
                Some(OwnedDesignSourceDescriptor {
                    format: SourceImageFormat::Tiff,
                    color_model,
                    bit_depth: preview.metadata.bit_depth,
                    channel_count: preview.metadata.samples_per_pixel,
                    embedded_icc: preview.metadata.icc_profile.clone(),
                    transparency: TransparencyState::None,
                    lossiness: SourceLossiness::Lossless,
                })
            }
            Self::Design(preview) => Some(preview.source.clone()),
            Self::Missing(_) => None,
        }
    }

    pub fn source_dimensions(&self) -> (u32, u32) {
        match self {
            Self::Tiff(preview) => (preview.metadata.width, preview.metadata.height),
            Self::Design(preview) => (preview.source_width, preview.source_height),
            Self::Missing(preview) => (preview.source_width, preview.source_height),
        }
    }

    pub fn missing(path: &Path, expected: Option<&FaceFileMetadata>) -> Self {
        Self::Missing(MissingPreview::from_cached(path, expected))
    }

    pub fn source_format(&self) -> Option<SourceImageFormat> {
        match self {
            Self::Tiff(_) => Some(SourceImageFormat::Tiff),
            Self::Design(preview) => Some(preview.source.format),
            Self::Missing(preview) => preview.source_format,
        }
    }

    pub fn bit_depth(&self) -> u8 {
        match self {
            Self::Tiff(preview) => preview.metadata.bit_depth,
            Self::Design(preview) => preview.source.bit_depth,
            Self::Missing(preview) => preview.bit_depth,
        }
    }

    pub fn channel_count(&self) -> usize {
        self.channels().len()
    }

    pub fn base_channel_count(&self) -> usize {
        match self {
            Self::Tiff(preview) => preview.metadata.base_channel_count,
            Self::Design(preview) => preview.source.channel_count,
            Self::Missing(preview) => preview.base_channel_count,
        }
    }

    pub fn as_tiff(&self) -> Option<&PreviewFace> {
        match self {
            Self::Tiff(preview) => Some(preview),
            Self::Design(_) | Self::Missing(_) => None,
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
            Self::Missing(preview) => preview.width(),
        }
    }

    fn height(&self) -> usize {
        match self {
            Self::Tiff(preview) => preview.height(),
            Self::Design(preview) => preview.height(),
            Self::Missing(preview) => preview.height(),
        }
    }

    fn channel_names(&self) -> &[String] {
        match self {
            Self::Tiff(preview) => preview.channel_names(),
            Self::Design(preview) => preview.channel_names(),
            Self::Missing(preview) => preview.channel_names(),
        }
    }

    fn channels(&self) -> &[Vec<u16>] {
        match self {
            Self::Tiff(preview) => preview.channels(),
            Self::Design(preview) => preview.channels(),
            Self::Missing(preview) => preview.channels(),
        }
    }

    fn histograms(&self) -> &[[u32; 256]] {
        match self {
            Self::Tiff(preview) => preview.histograms(),
            Self::Design(preview) => preview.histograms(),
            Self::Missing(preview) => preview.histograms(),
        }
    }

    fn color_model(&self) -> RuntimeColorModel {
        match self {
            Self::Tiff(preview) => preview.color_model(),
            Self::Design(preview) => preview.color_model(),
            Self::Missing(preview) => preview.color_model(),
        }
    }

    fn embedded_icc(&self) -> Option<&[u8]> {
        match self {
            Self::Tiff(preview) => preview.embedded_icc(),
            Self::Design(preview) => preview.embedded_icc(),
            Self::Missing(preview) => preview.embedded_icc(),
        }
    }

    fn alpha(&self) -> Option<&[u16]> {
        match self {
            Self::Tiff(preview) => preview.alpha(),
            Self::Design(preview) => preview.alpha(),
            Self::Missing(preview) => preview.alpha(),
        }
    }

    fn tiff_metadata(&self) -> Option<&TiffMetadata> {
        match self {
            Self::Tiff(preview) => preview.tiff_metadata(),
            Self::Design(preview) => preview.tiff_metadata(),
            Self::Missing(preview) => preview.tiff_metadata(),
        }
    }
}

fn runtime_color_model_from_cached(value: &str) -> RuntimeColorModel {
    match value.trim().to_ascii_lowercase().as_str() {
        "gray" | "greyscale" | "grayscale" => RuntimeColorModel::Gray,
        "rgb" => RuntimeColorModel::Rgb,
        "cmyk" => RuntimeColorModel::Cmyk,
        _ => RuntimeColorModel::Other,
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
    use windows_shade_editor::conversion_preflight::{
        PreflightCode, SourceProfileState, build_conversion_preflight_for_source,
    };
    use windows_shade_editor::conversion_workflow::ConversionSaveGate;
    use windows_shade_editor::jpeg_source::{
        DecodedJpegSource, JpegCodingProcess, JpegSourceModel,
    };
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
        let descriptor = preview.source_descriptor().expect("available descriptor");
        assert_eq!(descriptor.format, SourceImageFormat::Png);
        assert_eq!(descriptor.bit_depth, 16);
        assert_eq!(descriptor.channel_count, 3);
        assert_eq!(preview.source_dimensions(), (2, 1));
        assert_eq!(preview.alpha(), Some(&[7, 8][..]));
        assert!(preview.tiff_metadata().is_none());
        assert!(!preview.is_tiff());
    }

    #[test]
    fn png_alpha_runtime_descriptor_blocks_production_preflight() {
        let decoded = DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![100, 200, 300],
            alpha: Some(vec![u16::MAX / 2]),
            icc_profile: None,
            declares_srgb: true,
        };
        let runtime = RuntimePreview::Design(
            DesignSourcePreview::from_png(&decoded, 512).expect("PNG runtime preview"),
        );
        let descriptor = runtime.source_descriptor().expect("PNG descriptor");
        assert_eq!(descriptor.format, SourceImageFormat::Png);
        assert_eq!(
            descriptor.transparency,
            TransparencyState::PresentUnresolved
        );
        assert_eq!(runtime.channels().len(), 3);
        assert!(runtime.alpha().is_some());
        let report = build_conversion_preflight_for_source(
            &descriptor.as_borrowed(),
            SourceProfileState::Missing,
            ConversionSaveGate::Ready,
        );
        assert!(report.contains(PreflightCode::UnresolvedTransparency));
        assert!(!report.can_convert());
    }

    #[test]
    fn jpeg_runtime_descriptor_preserves_coding_process_lossiness_for_preflight() {
        let build = |coding_process| {
            let decoded = DecodedJpegSource {
                width: 1,
                height: 1,
                bit_depth: 8,
                model: JpegSourceModel::Rgb,
                samples: vec![1, 2, 3],
                icc_profile: None,
                coding_process,
            };
            RuntimePreview::Design(
                DesignSourcePreview::from_jpeg(&decoded, 512).expect("JPEG runtime preview"),
            )
        };

        let lossy = build(JpegCodingProcess::DctProgressive);
        let lossy_descriptor = lossy.source_descriptor().expect("lossy JPEG descriptor");
        assert_eq!(lossy_descriptor.lossiness, SourceLossiness::Lossy);
        let lossy_report = build_conversion_preflight_for_source(
            &lossy_descriptor.as_borrowed(),
            SourceProfileState::Missing,
            ConversionSaveGate::Ready,
        );
        assert!(lossy_report.contains(PreflightCode::JpegLossySource));

        let lossless = build(JpegCodingProcess::Lossless);
        let lossless_descriptor = lossless
            .source_descriptor()
            .expect("lossless JPEG descriptor");
        assert_eq!(lossless_descriptor.lossiness, SourceLossiness::Lossless);
        let lossless_report = build_conversion_preflight_for_source(
            &lossless_descriptor.as_borrowed(),
            SourceProfileState::Missing,
            ConversionSaveGate::Ready,
        );
        assert!(!lossless_report.contains(PreflightCode::JpegLossySource));
    }

    #[test]
    fn tiff_runtime_carrier_preserves_preview_and_descriptor_parity() {
        let metadata = TiffMetadata {
            width: 2,
            height: 1,
            bit_depth: 16,
            samples_per_pixel: 3,
            base_channel_count: 3,
            color_model: TiffColorModel::Rgb,
            non_cmyk_separated: false,
            channel_names: vec!["Red".into(), "Green".into(), "Blue".into()],
            channel_display_info: vec![None; 3],
            compression: None,
            predictor: None,
            orientation: None,
            icc_profile: Some(vec![9, 8, 7]),
            photoshop_resources: None,
            photoshop_image_source_data: None,
        };
        let original = PreviewFace {
            metadata,
            width: 2,
            height: 1,
            channels: vec![vec![1, 2], vec![3, 4], vec![5, 6]],
            histograms: vec![[0; 256]; 3],
        };
        let expected_channels = original.channels.clone();
        let runtime = RuntimePreview::Tiff(original);
        let descriptor = runtime.source_descriptor().expect("TIFF descriptor");
        assert_eq!(descriptor.format, SourceImageFormat::Tiff);
        assert_eq!(descriptor.color_model, DesignSourceColorModel::Rgb);
        assert_eq!(descriptor.bit_depth, 16);
        assert_eq!(descriptor.channel_count, 3);
        assert_eq!(descriptor.embedded_icc.as_deref(), Some(&[9, 8, 7][..]));
        assert_eq!(descriptor.transparency, TransparencyState::None);
        assert_eq!(descriptor.lossiness, SourceLossiness::Lossless);
        assert_eq!(runtime.source_dimensions(), (2, 1));
        assert_eq!(runtime.width(), 2);
        assert_eq!(runtime.height(), 1);
        assert_eq!(runtime.channels(), expected_channels.as_slice());
        assert_eq!(runtime.channel_names(), ["Red", "Green", "Blue"]);
        assert!(runtime.tiff_metadata().is_some());
        assert!(runtime.as_tiff().is_some());
        assert!(runtime.is_tiff());
    }

    #[test]
    fn missing_preview_uses_cached_metadata_without_tiff_identity() {
        let expected = FaceFileMetadata {
            width: 640,
            height: 480,
            bit_depth: 16,
            color_model: "RGB".to_owned(),
            channel_count: 3,
            base_channel_count: 3,
            channel_names: vec!["Red".into(), "Green".into(), "Blue".into()],
            ..FaceFileMetadata::default()
        };
        let preview = RuntimePreview::missing(Path::new("missing.png"), Some(&expected));
        assert_eq!(preview.source_format(), Some(SourceImageFormat::Png));
        assert_eq!(preview.source_dimensions(), (640, 480));
        assert_eq!(preview.bit_depth(), 16);
        assert_eq!(preview.channel_count(), 3);
        assert_eq!(preview.base_channel_count(), 3);
        assert_eq!(preview.color_model(), RuntimeColorModel::Rgb);
        assert_eq!(preview.channel_names(), ["Red", "Green", "Blue"]);
        assert!(preview.source_descriptor().is_none());
        assert!(preview.tiff_metadata().is_none());
        assert!(!preview.is_tiff());
    }
}
