use std::fs;
use std::path::{Path, PathBuf};

use lcms2::{
    ColorSpaceSignature, Flags, InfoType, Intent, Locale, PixelFormat, Profile, Transform,
};

use crate::model::PreviewRenderingIntent;
use crate::tiff_io::{ColorModel, TiffMetadata};

#[derive(Clone, Debug)]
pub struct InstalledIccProfile {
    pub path: PathBuf,
    pub description: String,
    color_space: ColorSpaceSignature,
    device_class: String,
}

impl InstalledIccProfile {
    pub fn filename(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn color_space_label(&self) -> String {
        color_space_label(self.color_space)
    }

    pub fn device_class_label(&self) -> &str {
        &self.device_class
    }

    pub fn compatible_with(&self, model: ColorModel) -> bool {
        expected_color_space(model).is_some_and(|expected| expected == self.color_space)
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        self.description.to_lowercase().contains(&query)
            || self.filename().to_lowercase().contains(&query)
            || self.path.to_string_lossy().to_lowercase().contains(&query)
            || self.color_space_label().to_lowercase().contains(&query)
            || self.device_class.to_lowercase().contains(&query)
    }
}

#[derive(Clone, Debug)]
pub struct PreviewColorConfig {
    pub enabled: bool,
    pub intent: PreviewRenderingIntent,
    pub black_point_compensation: bool,
    pub assigned_profile_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewProfileSource {
    Embedded,
    Assigned(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewColorStatus {
    Pending,
    Disabled,
    NoEmbeddedProfile,
    Applied {
        description: String,
        intent: PreviewRenderingIntent,
        source: PreviewProfileSource,
        black_point_compensation: bool,
    },
    Fallback {
        reason: String,
        requested_label: Option<String>,
    },
}

impl Default for PreviewColorStatus {
    fn default() -> Self {
        Self::Pending
    }
}

impl PreviewColorStatus {
    pub fn button_label(&self) -> String {
        match self {
            Self::Pending => "ICC...".to_owned(),
            Self::Disabled => "Color preview off".to_owned(),
            Self::NoEmbeddedProfile => "No embedded ICC".to_owned(),
            Self::Applied { description, .. } => description.clone(),
            Self::Fallback {
                requested_label: Some(label),
                ..
            } => format!("{label} (fallback)"),
            Self::Fallback { .. } => "ICC fallback".to_owned(),
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::Pending => "Preview color management has not rendered this Face yet.".to_owned(),
            Self::Disabled => "Project color preview is disabled. TIFF data and metadata are unchanged.".to_owned(),
            Self::NoEmbeddedProfile => "This TIFF has no embedded ICC profile and no preview profile is assigned. Shade Editor is using its unmanaged display fallback.".to_owned(),
            Self::Applied {
                description,
                intent,
                source,
                black_point_compensation,
            } => {
                let source = match source {
                    PreviewProfileSource::Embedded => "embedded TIFF profile".to_owned(),
                    PreviewProfileSource::Assigned(path) => {
                        format!("assigned preview profile {}", path.display())
                    }
                };
                let bpc = if *black_point_compensation {
                    " · black point compensation on"
                } else {
                    ""
                };
                format!(
                    "{} ({source}) → sRGB preview · {} intent{bpc}. Preview-only; TIFF samples, embedded ICC and Photoshop metadata are unchanged.",
                    description,
                    intent.label(),
                )
            }
            Self::Fallback { reason, .. } => format!(
                "The requested ICC could not be used for preview ({reason}). Shade Editor fell back to the unmanaged display conversion; TIFF data is unchanged."
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

/// Preview-only ICC transform. Assigned profiles reinterpret the TIFF base
/// channels for display only; they are never written back to the source/export.
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

        let expected = match expected_color_space(metadata.color_model) {
            Some(value) => value,
            None => {
                return Self::fallback(
                    "unsupported TIFF base color model".to_owned(),
                    config
                        .assigned_profile_path
                        .as_deref()
                        .map(profile_path_label),
                );
            }
        };

        let (source, source_kind, requested_label) =
            if let Some(path) = config.assigned_profile_path.as_ref() {
                let requested_label = Some(profile_path_label(path));
                match Profile::new_file(path) {
                    Ok(profile) => (
                        profile,
                        PreviewProfileSource::Assigned(path.clone()),
                        requested_label,
                    ),
                    Err(err) => {
                        return Self::fallback(
                            format!("cannot open assigned profile {}: {err}", path.display()),
                            requested_label,
                        );
                    }
                }
            } else {
                let Some(icc) = metadata.icc_profile.as_deref() else {
                    return Self {
                        transform: None,
                        status: PreviewColorStatus::NoEmbeddedProfile,
                    };
                };
                match Profile::new_icc(icc) {
                    Ok(profile) => (profile, PreviewProfileSource::Embedded, None),
                    Err(err) => {
                        return Self::fallback(
                            format!("invalid embedded profile: {err}"),
                            Some("Embedded ICC".to_owned()),
                        );
                    }
                }
            };

        let actual = source.color_space();
        if actual != expected {
            return Self::fallback(
                format!(
                    "profile color space {} does not match TIFF {}",
                    color_space_label(actual),
                    metadata.color_model.title(),
                ),
                requested_label,
            );
        }

        let description = profile_description(&source);
        let destination = Profile::new_srgb();
        let intent = to_lcms_intent(config.intent);
        let bpc = config.black_point_compensation;

        let transform = match metadata.color_model {
            ColorModel::Rgb => if bpc {
                Transform::new_flags(
                    &source,
                    PixelFormat::RGB_16,
                    &destination,
                    PixelFormat::RGB_8,
                    intent,
                    Flags::BLACKPOINT_COMPENSATION,
                )
            } else {
                Transform::new(
                    &source,
                    PixelFormat::RGB_16,
                    &destination,
                    PixelFormat::RGB_8,
                    intent,
                )
            }
            .map(BaseTransform::Rgb),
            ColorModel::Cmyk => if bpc {
                Transform::new_flags(
                    &source,
                    PixelFormat::CMYK_16,
                    &destination,
                    PixelFormat::RGB_8,
                    intent,
                    Flags::BLACKPOINT_COMPENSATION,
                )
            } else {
                Transform::new(
                    &source,
                    PixelFormat::CMYK_16,
                    &destination,
                    PixelFormat::RGB_8,
                    intent,
                )
            }
            .map(BaseTransform::Cmyk),
            ColorModel::Gray => if bpc {
                Transform::new_flags(
                    &source,
                    PixelFormat::GRAY_16,
                    &destination,
                    PixelFormat::RGB_8,
                    intent,
                    Flags::BLACKPOINT_COMPENSATION,
                )
            } else {
                Transform::new(
                    &source,
                    PixelFormat::GRAY_16,
                    &destination,
                    PixelFormat::RGB_8,
                    intent,
                )
            }
            .map(BaseTransform::Gray),
            ColorModel::Other => unreachable!(),
        };

        match transform {
            Ok(transform) => Self {
                transform: Some(transform),
                status: PreviewColorStatus::Applied {
                    description,
                    intent: config.intent,
                    source: source_kind,
                    black_point_compensation: bpc,
                },
            },
            Err(err) => Self::fallback(
                format!("cannot create ICC transform: {err}"),
                requested_label,
            ),
        }
    }

    fn fallback(reason: String, requested_label: Option<String>) -> Self {
        Self {
            transform: None,
            status: PreviewColorStatus::Fallback {
                reason,
                requested_label,
            },
        }
    }

    pub fn status(&self) -> &PreviewColorStatus {
        &self.status
    }

    /// Convert only the base RGB/CMYK/Gray channels. Spot separations remain
    /// outside this transform and are composited later from Photoshop DisplayInfo.
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

pub fn embedded_profile_description(metadata: &TiffMetadata) -> Option<String> {
    let icc = metadata.icc_profile.as_deref()?;
    Profile::new_icc(icc)
        .ok()
        .map(|profile| profile_description(&profile))
}

pub fn inspect_profile(path: &Path) -> Result<InstalledIccProfile, String> {
    let profile = Profile::new_file(path)
        .map_err(|err| format!("Cannot open ICC profile {}: {err}", path.display()))?;
    Ok(InstalledIccProfile {
        path: path.to_path_buf(),
        description: profile_description(&profile),
        color_space: profile.color_space(),
        device_class: format!("{:?}", profile.device_class()),
    })
}

/// Enumerate installed profile files from the Windows color directory. Windows
/// installs ICC/ICM profiles into System32\\spool\\drivers\\color; Browse remains
/// available for valid profiles stored elsewhere.
pub fn installed_profiles() -> Result<Vec<InstalledIccProfile>, String> {
    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let directory = windows
        .join("System32")
        .join("spool")
        .join("drivers")
        .join("color");
    let entries = fs::read_dir(&directory).map_err(|err| {
        format!(
            "Cannot read Windows color profile directory {}: {err}",
            directory.display()
        )
    })?;
    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !is_profile_path(&path) {
            continue;
        }
        if let Ok(profile) = inspect_profile(&path) {
            profiles.push(profile);
        }
    }
    profiles.sort_by(|left, right| {
        left.description
            .to_lowercase()
            .cmp(&right.description.to_lowercase())
            .then_with(|| {
                left.filename()
                    .to_lowercase()
                    .cmp(&right.filename().to_lowercase())
            })
    });
    profiles.dedup_by(|left, right| left.path == right.path);
    Ok(profiles)
}

pub fn is_profile_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("icc") || ext.eq_ignore_ascii_case("icm"))
}

fn profile_description(profile: &Profile) -> String {
    profile
        .info(InfoType::Description, Locale::none())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ICC profile".to_owned())
}

fn profile_path_label(path: &Path) -> String {
    inspect_profile(path)
        .map(|profile| profile.description)
        .unwrap_or_else(|_| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "Assigned ICC".to_owned())
        })
}

fn expected_color_space(model: ColorModel) -> Option<ColorSpaceSignature> {
    match model {
        ColorModel::Rgb => Some(ColorSpaceSignature::RgbData),
        ColorModel::Cmyk => Some(ColorSpaceSignature::CmykData),
        ColorModel::Gray => Some(ColorSpaceSignature::GrayData),
        ColorModel::Other => None,
    }
}

fn color_space_label(space: ColorSpaceSignature) -> String {
    match space {
        ColorSpaceSignature::RgbData => "RGB".to_owned(),
        ColorSpaceSignature::CmykData => "CMYK".to_owned(),
        ColorSpaceSignature::GrayData => "Gray".to_owned(),
        other => format!("{other:?}"),
    }
}

fn to_lcms_intent(intent: PreviewRenderingIntent) -> Intent {
    match intent {
        PreviewRenderingIntent::Perceptual => Intent::Perceptual,
        PreviewRenderingIntent::RelativeColorimetric => Intent::RelativeColorimetric,
        PreviewRenderingIntent::Saturation => Intent::Saturation,
        PreviewRenderingIntent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
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

    fn config() -> PreviewColorConfig {
        PreviewColorConfig {
            enabled: true,
            intent: PreviewRenderingIntent::Perceptual,
            black_point_compensation: false,
            assigned_profile_path: None,
        }
    }

    #[test]
    fn disabled_and_missing_profiles_never_create_a_transform() {
        let no_profile = rgb_metadata(None);
        let disabled = PreviewColorTransform::new(
            &no_profile,
            PreviewColorConfig {
                enabled: false,
                ..config()
            },
        );
        assert_eq!(*disabled.status(), PreviewColorStatus::Disabled);
        assert!(
            disabled
                .base_rgb8(&[vec![0], vec![0], vec![0]], 1)
                .is_none()
        );

        let missing = PreviewColorTransform::new(&no_profile, config());
        assert_eq!(*missing.status(), PreviewColorStatus::NoEmbeddedProfile);
    }

    #[test]
    fn embedded_srgb_profile_is_used_for_rgb_preview() {
        let icc = Profile::new_srgb().icc().expect("serialize sRGB profile");
        let metadata = rgb_metadata(Some(icc));
        let transform = PreviewColorTransform::new(
            &metadata,
            PreviewColorConfig {
                intent: PreviewRenderingIntent::RelativeColorimetric,
                ..config()
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
    fn assigned_profile_can_manage_preview_without_embedded_icc() {
        let icc = Profile::new_srgb().icc().expect("serialize sRGB profile");
        let path = std::env::temp_dir().join(format!(
            "shade-editor-assigned-profile-{}.icc",
            std::process::id()
        ));
        fs::write(&path, icc).expect("write temporary ICC");
        let metadata = rgb_metadata(None);
        let transform = PreviewColorTransform::new(
            &metadata,
            PreviewColorConfig {
                assigned_profile_path: Some(path.clone()),
                ..config()
            },
        );
        let _ = fs::remove_file(&path);
        assert!(transform.status().is_managed(), "{:?}", transform.status());
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
        let transform = PreviewColorTransform::new(&metadata, config());
        assert!(transform.status().is_problem());
    }
}
