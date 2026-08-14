use lcms2::{ColorSpaceSignature, InfoType, Intent, Locale, PixelFormat, Profile, Transform};
use serde::{Deserialize, Serialize};

use crate::tiff_io::{ColorModel, TiffMetadata};

/// Rendering intent used only by the preview color-management stage.
/// Export never consumes this setting and always preserves the source ICC bytes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PreviewRenderingIntent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl Default for PreviewRenderingIntent {
    fn default() -> Self {
        Self::Perceptual
    }
}

impl PreviewRenderingIntent {
    pub fn label(self) -> &'static str {
        match self {
            Self::Perceptual => "Perceptual",
            Self::RelativeColorimetric => "Relative colorimetric",
            Self::Saturation => "Saturation",
            Self::AbsoluteColorimetric => "Absolute colorimetric",
        }
    }

    fn lcms(self) -> Intent {
        match self {
            Self::Perceptual => Intent::Perceptual,
            Self::RelativeColorimetric => Intent::RelativeColorimetric,
            Self::Saturation => Intent::Saturation,
            Self::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewColorConfig {
    pub enabled: bool,
    pub intent: PreviewRenderingIntent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewColorStatus {
    Pending,
    Disabled,
    NoEmbeddedProfile,
    Applied {
        description: String,
        intent: PreviewRenderingIntent,
    },
    Fallback {
        reason: String,
    },
}

impl Default for PreviewColorStatus {
    fn default() -> Self {
        Self::Pending
    }
}

impl PreviewColorStatus {
    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Pending => "ICC: pending",
            Self::Disabled => "ICC: off",
            Self::NoEmbeddedProfile => "ICC: none",
            Self::Applied { .. } => "ICC: managed",
            Self::Fallback { .. } => "ICC: fallback",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::Pending => "Preview color management has not rendered this Face yet.".to_owned(),
            Self::Disabled => "ICC-aware preview is disabled in Settings. TIFF data and metadata are unchanged.".to_owned(),
            Self::NoEmbeddedProfile => "This TIFF has no embedded ICC profile; Shade Editor is using its unmanaged display fallback.".to_owned(),
            Self::Applied { description, intent } => format!(
                "Embedded ICC '{}' → sRGB display preview · {} intent. Preview-only; TIFF samples and metadata are unchanged.",
                description,
                intent.label(),
            ),
            Self::Fallback { reason } => format!(
                "Embedded ICC could not be used for this preview ({reason}). Shade Editor fell back to the unmanaged display conversion; TIFF data is unchanged."
            ),
        }
    }

    pub fn is_managed(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }

    pub fn is_problem(&self) -> bool {
        matches!(self, Self::Fallback { .. })
    }
}

enum BaseTransform {
    Rgb(Transform<[u16; 3], [u8; 3]>),
    Cmyk(Transform<[u16; 4], [u8; 3]>),
    Gray(Transform<[u16; 1], [u8; 3]>),
}

/// Preview-only ICC transform. This module intentionally owns all LittleCMS use
/// so a future printer/RIP soft-proof can be added here (via a proofing transform)
/// without letting color management leak into the production TIFF export path.
pub struct PreviewColorTransform {
    transform: Option<BaseTransform>,
    status: PreviewColorStatus,
}

impl PreviewColorTransform {
    pub fn new(metadata: &TiffMetadata, config: PreviewColorConfig) -> Self {
        if !config.enabled {
            return Self {
                transform: None,
                status: PreviewColorStatus::Disabled,
            };
        }
        let Some(icc) = metadata.icc_profile.as_deref() else {
            return Self {
                transform: None,
                status: PreviewColorStatus::NoEmbeddedProfile,
            };
        };
        let source = match Profile::new_icc(icc) {
            Ok(profile) => profile,
            Err(err) => return Self::fallback(format!("invalid embedded profile: {err}")),
        };
        let expected = match metadata.color_model {
            ColorModel::Rgb => Some(ColorSpaceSignature::RgbData),
            ColorModel::Cmyk => Some(ColorSpaceSignature::CmykData),
            ColorModel::Gray => Some(ColorSpaceSignature::GrayData),
            ColorModel::Other => None,
        };
        let Some(expected) = expected else {
            return Self::fallback("unsupported TIFF base color model".to_owned());
        };
        let actual = source.color_space();
        if actual != expected {
            return Self::fallback(format!(
                "profile color space {:?} does not match TIFF {}",
                actual,
                metadata.color_model.title(),
            ));
        }

        let description = source
            .info(InfoType::Description, Locale::none())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Embedded profile".to_owned());
        let destination = Profile::new_srgb();
        let intent = config.intent.lcms();
        let transform = match metadata.color_model {
            ColorModel::Rgb => Transform::new(
                &source,
                PixelFormat::RGB_16,
                &destination,
                PixelFormat::RGB_8,
                intent,
            )
            .map(BaseTransform::Rgb),
            ColorModel::Cmyk => Transform::new(
                &source,
                PixelFormat::CMYK_16,
                &destination,
                PixelFormat::RGB_8,
                intent,
            )
            .map(BaseTransform::Cmyk),
            ColorModel::Gray => Transform::new(
                &source,
                PixelFormat::GRAY_16,
                &destination,
                PixelFormat::RGB_8,
                intent,
            )
            .map(BaseTransform::Gray),
            ColorModel::Other => unreachable!(),
        };
        match transform {
            Ok(transform) => Self {
                transform: Some(transform),
                status: PreviewColorStatus::Applied {
                    description,
                    intent: config.intent,
                },
            },
            Err(err) => Self::fallback(format!("cannot create ICC transform: {err}")),
        }
    }

    fn fallback(reason: String) -> Self {
        Self {
            transform: None,
            status: PreviewColorStatus::Fallback { reason },
        }
    }

    pub fn status(&self) -> &PreviewColorStatus {
        &self.status
    }

    /// Convert only the base RGB/CMYK/Gray channels. Spot separations are kept
    /// outside the ICC transform and composited later from Photoshop DisplayInfo.
    pub fn base_rgb8(&self, planes: &[Vec<u16>], pixel_count: usize) -> Option<Vec<[u8; 3]>> {
        let transform = self.transform.as_ref()?;
        match transform {
            BaseTransform::Rgb(transform) => {
                if planes.len() < 3 {
                    return None;
                }
                let mut src = Vec::with_capacity(pixel_count);
                for pixel in 0..pixel_count {
                    src.push([
                        *planes[0].get(pixel)?,
                        *planes[1].get(pixel)?,
                        *planes[2].get(pixel)?,
                    ]);
                }
                let mut dst = vec![[0u8; 3]; pixel_count];
                transform.transform_pixels(&src, &mut dst);
                Some(dst)
            }
            BaseTransform::Cmyk(transform) => {
                if planes.len() < 4 {
                    return None;
                }
                let mut src = Vec::with_capacity(pixel_count);
                for pixel in 0..pixel_count {
                    src.push([
                        *planes[0].get(pixel)?,
                        *planes[1].get(pixel)?,
                        *planes[2].get(pixel)?,
                        *planes[3].get(pixel)?,
                    ]);
                }
                let mut dst = vec![[0u8; 3]; pixel_count];
                transform.transform_pixels(&src, &mut dst);
                Some(dst)
            }
            BaseTransform::Gray(transform) => {
                if planes.is_empty() {
                    return None;
                }
                let mut src = Vec::with_capacity(pixel_count);
                for pixel in 0..pixel_count {
                    src.push([*planes[0].get(pixel)?]);
                }
                let mut dst = vec![[0u8; 3]; pixel_count];
                transform.transform_pixels(&src, &mut dst);
                Some(dst)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb_metadata(icc_profile: Option<Vec<u8>>) -> TiffMetadata {
        TiffMetadata {
            width: 1,
            height: 1,
            bit_depth: 16,
            samples_per_pixel: 3,
            base_channel_count: 3,
            color_model: ColorModel::Rgb,
            channel_names: vec!["Red".into(), "Green".into(), "Blue".into()],
            channel_display_info: vec![None; 3],
            compression: None,
            predictor: None,
            orientation: None,
            icc_profile,
            photoshop_resources: None,
            photoshop_image_source_data: None,
        }
    }

    #[test]
    fn preview_intent_defaults_to_perceptual() {
        assert_eq!(
            PreviewRenderingIntent::default(),
            PreviewRenderingIntent::Perceptual
        );
    }

    #[test]
    fn disabled_and_missing_profiles_never_create_a_transform() {
        let no_profile = rgb_metadata(None);
        let disabled = PreviewColorTransform::new(
            &no_profile,
            PreviewColorConfig {
                enabled: false,
                intent: PreviewRenderingIntent::Perceptual,
            },
        );
        assert_eq!(*disabled.status(), PreviewColorStatus::Disabled);
        assert!(
            disabled
                .base_rgb8(&[vec![0], vec![0], vec![0]], 1)
                .is_none()
        );

        let missing = PreviewColorTransform::new(
            &no_profile,
            PreviewColorConfig {
                enabled: true,
                intent: PreviewRenderingIntent::Perceptual,
            },
        );
        assert_eq!(*missing.status(), PreviewColorStatus::NoEmbeddedProfile);
    }

    #[test]
    fn embedded_srgb_profile_is_used_for_rgb_preview() {
        let icc = Profile::new_srgb().icc().expect("serialize sRGB profile");
        let metadata = rgb_metadata(Some(icc));
        let transform = PreviewColorTransform::new(
            &metadata,
            PreviewColorConfig {
                enabled: true,
                intent: PreviewRenderingIntent::RelativeColorimetric,
            },
        );
        assert!(transform.status().is_managed(), "{:?}", transform.status());
        let output = transform
            .base_rgb8(&[vec![65535], vec![32768], vec![0]], 1)
            .expect("managed RGB output");
        assert_eq!(output.len(), 1);
        assert!(output[0][0] > 245);
        assert!((120..=136).contains(&output[0][1]));
        assert!(output[0][2] < 10);
    }

    #[test]
    fn profile_color_space_mismatch_falls_back_safely() {
        let icc = Profile::new_srgb().icc().expect("serialize sRGB profile");
        let mut metadata = rgb_metadata(Some(icc));
        metadata.color_model = ColorModel::Cmyk;
        metadata.samples_per_pixel = 4;
        metadata.base_channel_count = 4;
        metadata.channel_names.push("Black".into());
        metadata.channel_display_info.push(None);
        let transform = PreviewColorTransform::new(
            &metadata,
            PreviewColorConfig {
                enabled: true,
                intent: PreviewRenderingIntent::Perceptual,
            },
        );
        assert!(transform.status().is_problem());
    }
}
