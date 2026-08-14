from pathlib import Path
import re
import os

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def write(path, text):
    (ROOT / path).write_text(text, encoding="utf-8", newline="\n")


def must_replace(text, old, new, label, count=1):
    actual = text.count(old)
    if actual < count:
        raise RuntimeError(f"{label}: expected at least {count} occurrence(s), found {actual}")
    return text.replace(old, new, count)


def must_regex(text, pattern, repl, label, count=1, flags=0):
    updated, actual = re.subn(pattern, repl, text, count=count, flags=flags)
    if actual != count:
        raise RuntimeError(f"{label}: expected {count} replacement(s), got {actual}")
    return updated


# ---------------------------------------------------------------------------
# Canonicalize the active source layout and remove superseded implementations.
# ---------------------------------------------------------------------------
legacy_to_active = [
    ("src/app_main.rs", "src/main.rs"),
    ("src/model_v6.rs", "src/model.rs"),
    ("src/settings_v6.rs", "src/settings.rs"),
    ("src/export_v6.rs", "src/export.rs"),
    ("src/update_v4.rs", "src/update.rs"),
    ("src/workflow_v0103.rs", "src/workflow.rs"),
]

for source, destination in legacy_to_active:
    src = ROOT / source
    dst = ROOT / destination
    if not src.exists():
        raise RuntimeError(f"Missing active source: {source}")
    if dst.exists():
        dst.unlink()
    src.replace(dst)

for obsolete in [
    "src/model_v4.rs",
    "src/model_v5.rs",
    "src/settings_v4.rs",
    "src/export_v4.rs",
]:
    path = ROOT / obsolete
    if path.exists():
        path.unlink()

# Old one-off documentation is consolidated into current docs below.
for obsolete_doc in [
    "docs/AI_AGENT_v0.6.md",
    "docs/V0103_NOTES.md",
    "docs/v0.6.0-notes.md",
    "docs/vector-icons-v061.md",
]:
    path = ROOT / obsolete_doc
    if path.exists():
        path.unlink()

cargo = read("Cargo.toml")
cargo = must_replace(cargo, 'path = "src/app_main.rs"', 'path = "src/main.rs"', "Cargo bin path")
write("Cargo.toml", cargo)

main = read("src/main.rs")
for old, new, label in [
    ('#[path = "app_log.rs"]\nmod app_log;', 'mod app_log;', "app_log module"),
    ('#[path = "color_management.rs"]\nmod color_management;', 'mod color_management;', "color module"),
    ('#[path = "dpi.rs"]\nmod dpi;', 'mod dpi;', "dpi module"),
    ('#[path = "export_v6.rs"]\nmod export;', 'mod export;', "export module"),
    ('#[path = "export_batch.rs"]\nmod export_batch;', 'mod export_batch;', "export batch module"),
    ('#[path = "history.rs"]\nmod history;', 'mod history;', "history module"),
    ('#[path = "model_v6.rs"]\nmod model;', 'mod model;', "model module"),
    ('#[path = "palette.rs"]\nmod palette;', 'mod palette;', "palette module"),
    ('#[path = "previous_shades.rs"]\nmod previous_shades;', 'mod previous_shades;', "previous shades module"),
    ('#[path = "recovery.rs"]\nmod recovery;', 'mod recovery;', "recovery module"),
    ('#[path = "render.rs"]\nmod render;', 'mod render;', "render module"),
    ('#[path = "safe_fs.rs"]\nmod safe_fs;', 'mod safe_fs;', "safe fs module"),
    ('#[path = "settings_v6.rs"]\nmod settings;', 'mod settings;', "settings module"),
    ('#[path = "thumbnail.rs"]\nmod thumbnail;', 'mod thumbnail;', "thumbnail module"),
    ('#[path = "tiff_io.rs"]\nmod tiff_io;', 'mod tiff_io;', "tiff module"),
    ('#[path = "update_v4.rs"]\nmod update;', 'mod update;', "update module"),
    ('#[path = "validation.rs"]\nmod validation;', 'mod validation;', "validation module"),
    ('#[path = "workflow_v0103.rs"]\nmod workflow_v0103;', 'mod workflow;', "workflow module"),
]:
    main = must_replace(main, old, new, label)
main = main.replace("workflow_v0103::", "workflow::")
main = must_replace(main, '"Shader Editor v"', '"Shade Editor v"', "window title typo")
write("src/main.rs", main)

lib = read("src/lib.rs")
lib = re.sub(r'#\[path = "(?:export_v6|model_v6)\.rs"\]\n', '', lib)
write("src/lib.rs", lib)

# ---------------------------------------------------------------------------
# Project-owned preview color settings. Schema stays v9: all new fields default.
# ---------------------------------------------------------------------------
model = read("src/model.rs")
model_insert = r'''
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
}

/// Project-owned display-only color setup. It is serialized in `.shade`, but is
/// never consumed by TIFF export and never changes source TIFF metadata/samples.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PreviewColorSettings {
    pub enabled: bool,
    /// None = use the active TIFF's embedded profile. Some(path) = temporarily
    /// assign that ICC/ICM profile to the TIFF base channels for app preview.
    pub assigned_profile_path: Option<String>,
    pub rendering_intent: PreviewRenderingIntent,
    pub black_point_compensation: bool,
}

impl Default for PreviewColorSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            assigned_profile_path: None,
            rendering_intent: PreviewRenderingIntent::Perceptual,
            black_point_compensation: false,
        }
    }
}
'''.strip()
model = must_replace(
    model,
    'pub const MAX_SNAPSHOT_HISTORY_STATES: usize = 50;\n',
    'pub const MAX_SNAPSHOT_HISTORY_STATES: usize = 50;\n\n' + model_insert + '\n',
    "model preview types",
)
model = must_replace(
    model,
    '    #[serde(default)]\n    pub channel_palette: Option<ChannelPalette>,\n',
    '    #[serde(default)]\n    pub channel_palette: Option<ChannelPalette>,\n    /// Preview-only ICC assignment and transform options for this project.\n    #[serde(default)]\n    pub preview_color: PreviewColorSettings,\n',
    "project preview field",
)
model = must_replace(
    model,
    '            channel_palette: None,\n            thumbnail: None,',
    '            channel_palette: None,\n            preview_color: PreviewColorSettings::default(),\n            thumbnail: None,',
    "project preview default",
)
# Add a focused serialization/default regression test to the existing test module.
test_marker = 'mod tests {\n    use super::*;\n'
if test_marker in model:
    model = must_replace(
        model,
        test_marker,
        test_marker + r'''

    #[test]
    fn preview_color_settings_default_to_embedded_non_destructive_preview() {
        let project = ShadeProject::default();
        assert!(project.preview_color.enabled);
        assert!(project.preview_color.assigned_profile_path.is_none());
        assert_eq!(
            project.preview_color.rendering_intent,
            PreviewRenderingIntent::Perceptual
        );
        assert!(!project.preview_color.black_point_compensation);

        let json = serde_json::to_string(&project).expect("serialize project");
        let restored: ShadeProject = serde_json::from_str(&json).expect("deserialize project");
        assert_eq!(restored.preview_color, project.preview_color);
    }
''',
        "model preview test",
    )
write("src/model.rs", model)

# App settings keep diagnostics/layout preferences only. ICC assignment is project-owned.
settings = read("src/settings.rs")
settings = settings.replace('use crate::color_management::PreviewRenderingIntent;\n', '')
settings = settings.replace('    pub icc_preview: bool,\n', '')
settings = settings.replace('    pub icc_rendering_intent: PreviewRenderingIntent,\n', '')
settings = settings.replace('            icc_preview: true,\n', '')
settings = settings.replace('            icc_rendering_intent: PreviewRenderingIntent::Perceptual,\n', '')
settings = must_regex(
    settings,
    r'    #\[test\]\n    fn color_management_and_clipping_defaults_are_safe\(\) \{.*?\n    \}\n',
    '    #[test]\n    fn clipping_defaults_are_safe() {\n        assert!(AppSettings::default().show_clipping_warnings);\n    }\n',
    "settings ICC test cleanup",
    flags=re.S,
)
write("src/settings.rs", settings)

# ---------------------------------------------------------------------------
# Dedicated preview color engine + installed ICC catalog.
# ---------------------------------------------------------------------------
color_management = r'''use std::fs;
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

#[derive(Clone, Copy, Debug)]
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
            ColorModel::Rgb => {
                if bpc {
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
                .map(BaseTransform::Rgb)
            }
            ColorModel::Cmyk => {
                if bpc {
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
                .map(BaseTransform::Cmyk)
            }
            ColorModel::Gray => {
                if bpc {
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
                .map(BaseTransform::Gray)
            }
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
    Profile::new_icc(icc).ok().map(|profile| profile_description(&profile))
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
    let directory = windows.join("System32").join("spool").join("drivers").join("color");
    let entries = fs::read_dir(&directory)
        .map_err(|err| format!("Cannot read Windows color profile directory {}: {err}", directory.display()))?;
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
            .then_with(|| left.filename().to_lowercase().cmp(&right.filename().to_lowercase()))
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
        assert!(disabled.base_rgb8(&[vec![0], vec![0], vec![0]], 1).is_none());

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
'''
write("src/color_management.rs", color_management)

# ---------------------------------------------------------------------------
# Main application integration and searchable profile assignment UI.
# ---------------------------------------------------------------------------
main = read("src/main.rs")
main = must_replace(
    main,
    'use color_management::{PreviewColorConfig, PreviewColorStatus, PreviewRenderingIntent};',
    'use color_management::{InstalledIccProfile, PreviewColorConfig, PreviewColorStatus};',
    "main color imports",
)
main = must_replace(
    main,
    'use model::{ChannelAdjustment, ShadeProject, TEST_CODE_ALL_CHANNELS, TestCodePosition};',
    'use model::{\n    ChannelAdjustment, PreviewRenderingIntent, ShadeProject, TEST_CODE_ALL_CHANNELS, TestCodePosition,\n};',
    "main model imports",
)
main = must_replace(
    main,
    '    show_settings: bool,\n    show_about: bool,',
    '    show_settings: bool,\n    show_color_management: bool,\n    icc_profile_query: String,\n    icc_profiles: Vec<InstalledIccProfile>,\n    icc_profile_selected: Option<String>,\n    icc_profile_scan_done: bool,\n    icc_profile_scan_error: Option<String>,\n    icc_show_incompatible: bool,\n    show_about: bool,',
    "main color dialog fields",
)
main = must_replace(
    main,
    '            show_settings: false,\n            show_about: false,',
    '            show_settings: false,\n            show_color_management: false,\n            icc_profile_query: String::new(),\n            icc_profiles: Vec::new(),\n            icc_profile_selected: None,\n            icc_profile_scan_done: false,\n            icc_profile_scan_error: None,\n            icc_show_incompatible: false,\n            show_about: false,',
    "main color dialog defaults",
)
main = must_replace(
    main,
    '        self.show_close_confirmation = false;\n        self.close_after_save = false;',
    '        self.show_close_confirmation = false;\n        self.close_after_save = false;\n        self.show_color_management = false;\n        self.icc_profile_query.clear();\n        self.icc_profile_selected = None;',
    "new project color dialog reset",
)
main = must_replace(
    main,
    '    /// Re-render textures for application-only display settings without marking\n    /// the .shade project dirty. ICC preview settings never alter project/TIFF data.\n',
    '    /// Re-render textures for display-only color settings. The caller decides\n    /// whether the project should be marked dirty; TIFF source/export data is never changed.\n',
    "invalidate comment",
)
main = must_replace(
    main,
    '        let color_config = PreviewColorConfig {\n            enabled: self.settings.icc_preview,\n            intent: self.settings.icc_rendering_intent,\n        };',
    '        let color_config = PreviewColorConfig {\n            enabled: self.project.preview_color.enabled,\n            intent: self.project.preview_color.rendering_intent,\n            black_point_compensation: self.project.preview_color.black_point_compensation,\n            assigned_profile_path: self\n                .project\n                .preview_color\n                .assigned_profile_path\n                .as_ref()\n                .map(PathBuf::from),\n        };',
    "render project color config",
)

old_status = '''            let icc_response = if color_status.is_problem() {
                ui.colored_label(egui::Color32::YELLOW, color_status.short_label())
            } else if color_status.is_managed() {
                ui.colored_label(egui::Color32::LIGHT_GREEN, color_status.short_label())
            } else {
                ui.label(color_status.short_label())
            };
            icc_response.on_hover_text(color_status.detail());
'''
new_status = '''            let profile_text = if color_status.is_problem() {
                egui::RichText::new(color_status.button_label()).color(egui::Color32::YELLOW)
            } else if color_status.is_managed() {
                egui::RichText::new(color_status.button_label()).color(egui::Color32::LIGHT_GREEN)
            } else {
                egui::RichText::new(color_status.button_label())
            };
            let icc_response = ui.small_button(profile_text);
            let open_color_management = icc_response.clicked();
            icc_response.on_hover_text(format!("{}\nClick to manage the preview profile.", color_status.detail()));
            if open_color_management {
                self.show_color_management = true;
                self.icc_profile_selected = self.project.preview_color.assigned_profile_path.clone();
            }
'''
main = must_replace(main, old_status, new_status, "clickable ICC status")

# Remove application-global ICC controls from Settings; keep clipping diagnostics.
main = must_replace(
    main,
    '        let color_preview_before = (\n            self.settings.icc_preview,\n            self.settings.icc_rendering_intent,\n        );\n',
    '',
    "settings color before state",
)
settings_color_block = '''                ui.heading("Color management & clipping");
                changed |= ui
                    .checkbox(
                        &mut self.settings.icc_preview,
                        "ICC-aware preview (embedded profile → sRGB)",
                    )
                    .changed();
                egui::ComboBox::from_label("Preview rendering intent")
                    .selected_text(self.settings.icc_rendering_intent.label())
                    .show_ui(ui, |ui| {
                        for intent in [
                            PreviewRenderingIntent::Perceptual,
                            PreviewRenderingIntent::RelativeColorimetric,
                            PreviewRenderingIntent::Saturation,
                            PreviewRenderingIntent::AbsoluteColorimetric,
                        ] {
                            changed |= ui
                                .selectable_value(
                                    &mut self.settings.icc_rendering_intent,
                                    intent,
                                    intent.label(),
                                )
                                .changed();
                        }
                    });
                changed |= ui
                    .checkbox(
                        &mut self.settings.show_clipping_warnings,
                        "Show per-channel clipping warnings",
                    )
                    .changed();
                ui.small("ICC is preview-only: it converts the TIFF base RGB/CMYK/Gray channels to sRGB for the screen, then composites declared Photoshop Spot channels. Alpha channels remain excluded. Exported samples, embedded ICC bytes and Photoshop resources are not changed by this setting.");
                ui.small("Clipping percentages are estimates from the loaded preview samples. Yellow starts at 0.10%; red at 1.00%. Full-resolution export data is not sampled or modified for these warnings.");
                ui.small("The color-management stage is isolated so a printer/RIP Soft Proof profile can be added later without changing the TIFF export pipeline.");
'''
settings_diag_block = '''                ui.heading("Preview diagnostics");
                changed |= ui
                    .checkbox(
                        &mut self.settings.show_clipping_warnings,
                        "Show per-channel clipping warnings",
                    )
                    .changed();
                ui.small("Clipping percentages are estimates from the loaded preview samples. Yellow starts at 0.10%; red at 1.00%. Full-resolution export data is not sampled or modified for these warnings.");
                ui.small("ICC profile assignment is project-owned. Click the profile name beside the active Face metadata to open Color Management.");
'''
main = must_replace(main, settings_color_block, settings_diag_block, "settings preview diagnostics")
main = must_replace(
    main,
    '        if color_preview_before\n            != (\n                self.settings.icc_preview,\n                self.settings.icc_rendering_intent,\n            )\n        {\n            self.invalidate_display_previews();\n        }\n',
    '',
    "settings color invalidation cleanup",
)

# Insert the Color Management window immediately before Settings.
color_window_method = r'''
    fn refresh_icc_profile_catalog(&mut self) {
        self.icc_profile_scan_done = true;
        match color_management::installed_profiles() {
            Ok(profiles) => {
                self.icc_profiles = profiles;
                self.icc_profile_scan_error = None;
            }
            Err(err) => {
                self.icc_profiles.clear();
                self.icc_profile_scan_error = Some(err);
            }
        }
    }

    fn ui_color_management_window(&mut self, ctx: &egui::Context) {
        if !self.show_color_management {
            return;
        }
        if !self.icc_profile_scan_done {
            self.refresh_icc_profile_catalog();
        }

        let Some(active_face) = self.faces.get(self.current_face) else {
            self.show_color_management = false;
            return;
        };
        let active_model = active_face.preview.metadata.color_model;
        let embedded_name = color_management::embedded_profile_description(&active_face.preview.metadata)
            .unwrap_or_else(|| "No embedded ICC".to_owned());
        let profiles = self.icc_profiles.clone();
        let scan_error = self.icc_profile_scan_error.clone();
        let current_status = active_face.color_status.clone();

        let original_query = self.icc_profile_query.clone();
        let mut query = original_query.clone();
        let mut selected = self
            .icc_profile_selected
            .clone()
            .or_else(|| self.project.preview_color.assigned_profile_path.clone());
        let mut enabled = self.project.preview_color.enabled;
        let mut intent = self.project.preview_color.rendering_intent;
        let mut bpc = self.project.preview_color.black_point_compensation;
        let mut show_incompatible = self.icc_show_incompatible;
        let mut requested_profile: Option<Option<PathBuf>> = None;
        let mut browse_requested = false;
        let mut refresh_requested = false;
        let mut open = self.show_color_management;

        egui::Window::new("Color Management / Preview Profile")
            .open(&mut open)
            .resizable(true)
            .default_size([760.0, 650.0])
            .show(ctx, |ui| {
                ui.heading("Preview profile assignment");
                ui.small("This changes only Shade Editor's preview. The source TIFF, exported samples, embedded ICC tag and Photoshop resources are never rewritten by profile assignment.");
                ui.add_space(5.0);
                egui::Grid::new("preview-profile-current")
                    .num_columns(2)
                    .striped(true)
                    .spacing([14.0, 5.0])
                    .show(ui, |ui| {
                        ui.strong("Active preview");
                        ui.label(current_status.button_label())
                            .on_hover_text(current_status.detail());
                        ui.end_row();
                        ui.strong("TIFF base model");
                        ui.label(active_model.title());
                        ui.end_row();
                        ui.strong("Embedded profile");
                        ui.label(&embedded_name);
                        ui.end_row();
                        ui.strong("Assigned profile");
                        ui.label(
                            self.project
                                .preview_color
                                .assigned_profile_path
                                .as_deref()
                                .unwrap_or("Embedded profile"),
                        );
                        ui.end_row();
                    });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Use embedded profile").clicked() {
                        requested_profile = Some(None);
                    }
                    if ui.button("Browse ICC / ICM...").clicked() {
                        browse_requested = true;
                    }
                    if ui.button("Refresh system profiles").clicked() {
                        refresh_requested = true;
                    }
                });

                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut enabled, "Enable color-managed preview");
                    ui.checkbox(&mut bpc, "Black point compensation");
                });
                egui::ComboBox::from_label("Rendering intent")
                    .selected_text(intent.label())
                    .show_ui(ui, |ui| {
                        for value in [
                            PreviewRenderingIntent::Perceptual,
                            PreviewRenderingIntent::RelativeColorimetric,
                            PreviewRenderingIntent::Saturation,
                            PreviewRenderingIntent::AbsoluteColorimetric,
                        ] {
                            ui.selectable_value(&mut intent, value, value.label());
                        }
                    });
                ui.small("Black point compensation is optional and is most useful with relative-colorimetric transforms. The preview destination remains sRGB; no monitor or printer/RIP proof profile is applied here.");

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Search");
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut query)
                            .hint_text("Profile name, filename, RGB/CMYK/Gray, path")
                            .desired_width(430.0),
                    );
                    if !search.has_focus() && !ctx.wants_keyboard_input() {
                        let typed = ctx.input(|input| {
                            input
                                .events
                                .iter()
                                .filter_map(|event| match event {
                                    egui::Event::Text(text) if !text.chars().all(char::is_control) => {
                                        Some(text.as_str())
                                    }
                                    _ => None,
                                })
                                .collect::<String>()
                        });
                        if !typed.is_empty() {
                            query.push_str(&typed);
                            search.request_focus();
                        }
                    }
                    ui.checkbox(&mut show_incompatible, "Show incompatible");
                });

                let visible = profiles
                    .iter()
                    .filter(|profile| profile.matches_query(&query))
                    .filter(|profile| show_incompatible || profile.compatible_with(active_model))
                    .collect::<Vec<_>>();
                let compatible_paths = visible
                    .iter()
                    .filter(|profile| profile.compatible_with(active_model))
                    .map(|profile| profile.path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                if query != original_query {
                    selected = compatible_paths.first().cloned();
                }
                let current_position = selected
                    .as_deref()
                    .and_then(|path| compatible_paths.iter().position(|item| item == path));
                let (up, down, enter) = ctx.input(|input| {
                    (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                        input.key_pressed(egui::Key::Enter),
                    )
                });
                if !compatible_paths.is_empty() && (up || down) {
                    let next = match (current_position, up, down) {
                        (Some(position), true, _) => position.saturating_sub(1),
                        (Some(position), _, true) => (position + 1).min(compatible_paths.len() - 1),
                        (None, _, true) => 0,
                        (None, true, _) => compatible_paths.len() - 1,
                        _ => 0,
                    };
                    selected = compatible_paths.get(next).cloned();
                }
                if enter {
                    if let Some(path) = selected.as_ref() {
                        requested_profile = Some(Some(PathBuf::from(path)));
                    }
                }

                ui.add_space(4.0);
                ui.strong(format!(
                    "System profiles · {} compatible / {} loaded",
                    profiles
                        .iter()
                        .filter(|profile| profile.compatible_with(active_model))
                        .count(),
                    profiles.len()
                ));
                if let Some(error) = scan_error.as_ref() {
                    ui.colored_label(egui::Color32::YELLOW, error);
                }
                if visible.is_empty() {
                    ui.label("No matching assignable profiles.");
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("icc-profile-list")
                        .auto_shrink([false, false])
                        .max_height(330.0)
                        .show(ui, |ui| {
                            for profile in visible {
                                let path_text = profile.path.to_string_lossy().into_owned();
                                let compatible = profile.compatible_with(active_model);
                                let label = format!(
                                    "{}  ·  {}  ·  {}",
                                    profile.description,
                                    profile.color_space_label(),
                                    profile.filename()
                                );
                                let response = ui
                                    .add_enabled(
                                        compatible,
                                        egui::Button::new(label)
                                            .selected(selected.as_deref() == Some(path_text.as_str())),
                                    )
                                    .on_hover_text(format!(
                                        "{}\nClass: {}{}",
                                        profile.path.display(),
                                        profile.device_class_label(),
                                        if compatible {
                                            ""
                                        } else {
                                            "\nIncompatible with the active TIFF base color model."
                                        }
                                    ));
                                if response.clicked() {
                                    selected = Some(path_text.clone());
                                }
                                if response.double_clicked() && compatible {
                                    requested_profile = Some(Some(profile.path.clone()));
                                }
                            }
                        });
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let can_assign = selected.as_deref().is_some_and(|path| {
                        profiles.iter().any(|profile| {
                            profile.path.to_string_lossy() == path
                                && profile.compatible_with(active_model)
                        })
                    });
                    if ui
                        .add_enabled(can_assign, egui::Button::new("Assign selected profile"))
                        .clicked()
                    {
                        requested_profile = selected
                            .as_ref()
                            .map(|path| Some(PathBuf::from(path)));
                    }
                    ui.small("Up/Down changes selection; Enter assigns it.");
                });
            });

        self.show_color_management = open;
        self.icc_profile_query = query;
        self.icc_profile_selected = selected;
        self.icc_show_incompatible = show_incompatible;

        if refresh_requested {
            self.icc_profile_scan_done = false;
            self.refresh_icc_profile_catalog();
        }

        if browse_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_profile = Some(Some(path));
            }
        }

        let mut changed = false;
        if self.project.preview_color.enabled != enabled {
            self.project.preview_color.enabled = enabled;
            changed = true;
        }
        if self.project.preview_color.rendering_intent != intent {
            self.project.preview_color.rendering_intent = intent;
            changed = true;
        }
        if self.project.preview_color.black_point_compensation != bpc {
            self.project.preview_color.black_point_compensation = bpc;
            changed = true;
        }

        if let Some(requested) = requested_profile {
            match requested {
                None => {
                    if self.project.preview_color.assigned_profile_path.is_some() {
                        self.project.preview_color.assigned_profile_path = None;
                        self.icc_profile_selected = None;
                        changed = true;
                    }
                }
                Some(path) => match color_management::inspect_profile(&path) {
                    Ok(profile) if profile.compatible_with(active_model) => {
                        let path_text = path.to_string_lossy().into_owned();
                        if self.project.preview_color.assigned_profile_path.as_deref()
                            != Some(path_text.as_str())
                        {
                            self.project.preview_color.assigned_profile_path = Some(path_text.clone());
                            self.icc_profile_selected = Some(path_text);
                            changed = true;
                        }
                    }
                    Ok(profile) => self.report_error(format!(
                        "Cannot assign '{}': profile color space {} does not match active TIFF {}.",
                        profile.description,
                        profile.color_space_label(),
                        active_model.title(),
                    )),
                    Err(err) => self.report_error(err),
                },
            }
        }

        if changed {
            self.project_dirty = true;
            self.invalidate_display_previews();
        }
    }

'''
main = must_replace(
    main,
    '    fn ui_settings_window(&mut self, ctx: &egui::Context) {',
    color_window_method + '    fn ui_settings_window(&mut self, ctx: &egui::Context) {',
    "color management window insertion",
)
main = must_replace(
    main,
    '        self.ui_settings_window(ui.ctx());\n        self.ui_about_window(ui.ctx());',
    '        self.ui_settings_window(ui.ctx());\n        self.ui_color_management_window(ui.ctx());\n        self.ui_about_window(ui.ctx());',
    "color management update call",
)
write("src/main.rs", main)

# ---------------------------------------------------------------------------
# Current documentation: consolidate architecture, fix pipeline order, document
# preview assignment accurately, and remove stale version-suffixed source names.
# ---------------------------------------------------------------------------
readme = r'''# Shade Editor

Native Windows shade editor for multi-channel TIFF artwork used in digital ceramic printing.

Shade Editor keeps source TIFF Faces immutable and stores non-destructive shade recipes and project preview settings in a `.shade` file beside the artwork. The application is native Rust/egui; there is no WebView/Electron runtime.

## Current features

- Open multiple TIFF files as project **Faces** and switch between them.
- Dynamic channel model for RGB/CMYK plus additional/Spot separations.
- Composite preview and isolated separation preview.
- Per-channel histogram, Levels, Curve and N×N Channel Mixer.
- Preview clipping diagnostics for Levels/Curve.
- Photoshop Spot DisplayInfo color/Solidity parsing; declared Alpha channels are excluded from the printing composite.
- ICC-aware preview with embedded-profile support and non-destructive **preview profile assignment**.
- Searchable installed Windows ICC/ICM profile list, keyboard navigation, rendering intent and optional black-point compensation.
- Assigned preview profile is saved in `.shade`; TIFF ICC bytes and source/export samples are never changed by it.
- Optional test-code raster in one separation or all separations.
- Export current Face or all Faces with production-oriented metadata preservation.
- BigTIFF selection/preservation, bounded strip/tile/planar processing and atomic destination replacement.
- Production round-trip validator for sample and critical TIFF/Photoshop metadata comparison.
- Persistent Project View, Snapshots, adjustment history, recovery and Windows Explorer Shell integration.

## `.shade` projects

A `.shade` file contains project state, adjustment recipes, preview color settings, cached metadata and a compact thumbnail. It does not contain TIFF pixel data.

```text
Moonstone/
├─ moonstone-face1.tif
├─ moonstone-face2.tif
├─ moonstone-face3.tif
└─ moonstone-test-1.shade
```

Schema version 9 is the current clean project format. New optional fields use Serde defaults so the preview-profile settings can be added without changing the schema number. Source TIFF paths are stored relative to the `.shade` file when possible; an assigned external ICC path is stored as a preview-only reference.

## Adjustment pipeline

For every channel the production adjustment order is:

```text
Source sample -> Levels -> Channel Mixer -> Curve -> Export sample
```

The mixer is dynamic: every discovered channel can contribute to every output channel. No production adjustment is hard-coded to four channels.

## Preview color management

Composite preview uses a separate display-only pipeline:

```text
Adjusted base RGB/CMYK/Gray samples
  -> embedded ICC OR assigned preview ICC
  -> selected rendering intent (+ optional BPC)
  -> sRGB preview
  -> Photoshop Spot DisplayInfo composite
```

Click the ICC/profile name beside the active Face metadata to open **Color Management / Preview Profile**. The window lists compatible ICC/ICM profiles from the Windows color-profile directory, supports search plus Up/Down/Enter navigation, and also allows browsing to another `.icc`/`.icm` file.

`Use embedded profile` returns the project to the TIFF profile. Assigning another profile reinterprets the base channel values only for Shade Editor's preview. It does **not** assign/write that profile into the TIFF and export does not consume the preview transform.

This is profile assignment / color-managed preview, not a printer/RIP proof simulation. A real proofing transform would require a separate proof-device profile; that is intentionally not part of the current scope.

## TIFF compatibility scope

The production-oriented path targets 8-bit and 16-bit RGB/CMYK TIFF with optional additional samples/Spot Channels. ICC tag 34675, Photoshop Image Resources 34377 and ImageSourceData 37724 are retained where supported. Photoshop/RIP interoperability must still be validated with representative production TIFFs; see `docs/PRODUCTION_VALIDATION.md`.

## Build and test

Requirements:

- Windows 10/11 x64
- Stable Rust toolchain with `x86_64-pc-windows-msvc`
- Visual Studio Build Tools / MSVC C++ tools

```powershell
cargo check --target x86_64-pc-windows-msvc
cargo test --target x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

Executable:

```text
target\x86_64-pc-windows-msvc\release\ShadeEditor.exe
```

The repository CI uploads validation/build artifacts; project development does not require publishing GitHub Releases.

## Project structure

```text
src/
├─ main.rs              Native UI and application orchestration
├─ model.rs             .shade schema and adjustment/project model
├─ color_management.rs  ICC preview transform and Windows ICC catalog
├─ tiff_io.rs           TIFF decode/channel/Photoshop metadata discovery
├─ render.rs            Non-destructive preview render pipeline
├─ export.rs            Full-resolution TIFF export
├─ validation.rs        Production round-trip validation
├─ settings.rs          Application-only persistent preferences
├─ previous_shades.rs   Project View history/index
├─ recovery.rs          Crash recovery
├─ update.rs            Update subsystem
└─ workflow.rs          Missing-Face/relink workflow helpers
```

See `docs/ARCHITECTURE.md` for invariants and extension points.

## License

MIT License. Copyright © 2026 Emad Ghasemi.
'''
write("README.md", readme)

architecture = r'''# Shade Editor architecture

This document is the current hand-off map for developers and AI agents.

## Hard invariants

1. The application is a native Windows desktop program. Do not introduce WebView, Electron, Tauri web front-ends or browser-hosted UI.
2. Source TIFF Face files are immutable inputs. `.shade` stores recipes/references; export creates/replaces output TIFFs only through the export backend.
3. Never hard-code production adjustment logic to exactly four channels. RGB/CMYK base channels may be followed by zero or more additional/Spot channels.
4. Real TIFF channel names/order remain authoritative. Palette aliases are UI-only.
5. UI code must not become the TIFF parser, export engine or ICC engine. Keep IO/model/render/export/color-management code independently testable.
6. Preview ICC assignment must never leak into `export.rs`. Export preserves the source embedded ICC payload and operates on adjustment output samples, not screen RGB.
7. Do not claim Photoshop/RIP compatibility without round-trip testing on real production files.

## Active source layout

Legacy version-suffixed implementations were removed. Active modules have canonical names:

- `main.rs` — egui UI and application orchestration.
- `model.rs` — schema-v9 `.shade` model, Snapshots, adjustments, Test Code and project preview-color settings.
- `color_management.rs` — embedded/assigned ICC preview transforms and installed Windows profile discovery.
- `tiff_io.rs` — TIFF decode, channel discovery, Photoshop resources, Spot polarity and metadata.
- `render.rs` — preview adjustment pipeline, clipping estimates and RGB/Spot composition.
- `export.rs` — full-resolution production TIFF renderer/writer.
- `validation.rs` — production round-trip comparison.
- `settings.rs` — application-only preferences such as layout, diagnostics, palettes and export defaults.
- `previous_shades.rs` — Project View cache/search/inspection.
- `recovery.rs` — rotating recovery states.
- `update.rs` — isolated self-update subsystem.
- `workflow.rs` — missing-Face/relink UI helpers.

`lib.rs` exposes the production backend required by TIFF conformance tests. `Cargo.toml` explicitly builds `src/main.rs` as `ShadeEditor`.

## `.shade` model

Schema v9 remains the clean-break format. New fields that are safe to default can be added with `#[serde(default)]` without forcing a schema bump; incompatible semantic changes still require incrementing `SHADE_SCHEMA_VERSION`.

`ShadeProject::preview_color` is project-wide and contains:

- enabled/disabled color-managed preview;
- optional assigned ICC/ICM path (`None` means the TIFF embedded profile);
- rendering intent;
- optional black-point compensation.

These values are not part of Snapshot adjustment history because they describe the project viewing environment rather than a shade recipe. They are saved in `.shade` so reopening the project reproduces the preview setup when the referenced profile is available.

## Adjustment/render data flow

Production adjustment order:

```text
TIFF source samples
  -> Levels
  -> N×N Channel Mixer
  -> Curve
  -> export sample
```

Preview reuses the adjusted downsampled planes, then performs display conversion:

```text
adjusted base RGB/CMYK/Gray
  -> embedded ICC or assigned preview ICC
  -> LittleCMS intent (+ optional BPC)
  -> sRGB
  -> declared Photoshop Spot DisplayInfo composite
  -> egui texture
```

Solo-channel view intentionally remains an engineering separation view, not a colorimetric composite.

Assigned ICC is an **input/source-profile override for preview**. It is not a proofing profile. A true printer/RIP soft proof would require LittleCMS proofing transforms and a separate proof-device profile and is currently out of scope.

## ICC profile catalog

`color_management::installed_profiles()` scans the standard Windows color-profile directory (`%WINDIR%\System32\spool\drivers\color`) for `.icc`/`.icm`, opens each valid profile with LittleCMS and records its description, base color space and device class. UI assignment is allowed only when the profile color space matches the active TIFF base model. Browse permits valid compatible profiles outside the system directory.

The Color Management window follows Project View's search/navigation behavior: typing focuses/updates search, Up/Down changes the compatible selection and Enter assigns it.

## TIFF / Spot rules

`tiff_io.rs` retains ICC tag 34675, Photoshop Image Resources 34377 and ImageSourceData 37724. Photoshop DisplayInfo resource 1077 drives declared Spot display color/Solidity. Known Alpha channels are not composite printing inks.

Photoshop Spot samples are normalized internally to ink-coverage polarity and converted back for export. Do not change this contract without fixtures and production validation.

## DPI

`dpi.rs` owns physical-resolution parsing and fallback. `AppSettings::default_dpi` defaults to 220 DPI. `DpiInfo::used_default` distinguishes source DPI from fallback. Do not introduce a 72-DPI fallback in UI, Test Code or export.

## Channel palettes

`palette.rs` owns built-in palettes; `settings.rs` owns custom palette library/default choice; `ShadeProject::channel_palette` stores the project snapshot. Palette names/colors are presentation only. TIFF names/order, adjustment keys, mixer keys, Test Code channel IDs and export metadata always use real source channel names.

## Production export boundary

`export.rs` applies full-resolution adjustments and Test Code, preserves approved metadata, uses bounded streaming/spooling paths, selects/preserves BigTIFF when required and commits output atomically. Preview ICC settings are forbidden inputs to this module.

## Remaining production validation

- Reopen identity exports in Photoshop and the production RIP and confirm Spot type/order/name, DisplayInfo, ICC, DPI and press interpretation.
- Keep regression fixtures/baselines for each production TIFF family.
- Validate large BigTIFF and tiled/planar production artwork.
- Validate Windows Shell install/upgrade/removal on clean workstations.
'''
write("docs/ARCHITECTURE.md", architecture)

roadmap = r'''# Shade Editor production roadmap

## Current blocking validation

- Run no-adjustment `Validate face` round trips on representative production CMYK + Spot TIFFs in Photoshop and the production RIP.
- Confirm Spot type/order/name, Photoshop DisplayInfo/Solidity, embedded ICC preservation, Photoshop resources, DPI, predictor/compression and RIP interpretation.
- Production-test missing-Face relink behavior against moved project folders/storage roots.
- Production-test optional post-export validation on large CMYK + Spot artwork before considering default-on behavior.

## Backend follow-up

- Production-test BigTIFF output above 4 GiB and confirm Photoshop/RIP acceptance.
- Production-test bounded tiled/planar streaming with real artwork; synthetic fixtures remain CI coverage.
- Continue TIFF conformance coverage across compression, predictors, bit depth, ExtraSamples and Photoshop metadata.
- Add fixtures for preview-profile assignment failure modes (missing external ICC, wrong color space, corrupt profile).

## Color-management scope

Implemented now: embedded ICC preview, project-owned temporary ICC assignment, rendering intents, optional black-point compensation, searchable installed Windows profiles and sRGB preview output.

Intentionally deferred: monitor-profile output transforms and printer/RIP proof-device transforms. A true proofing transform should only be added when a real production proof profile/workflow is available to validate it; do not label source-profile assignment as printer soft proof.

## Native Windows integration

- Validate `.shade` thumbnail/property handler install, Explorer cache/indexing, file association, upgrade and removal on a clean workstation.

## Explicitly out of scope

- More Snapshot metadata/features beyond the current workflow.
- Duplicate-content detection beyond current duplicate-reference behavior.
- Additional adjustment types until production transport/interchange validation is complete.
'''
write("docs/ROADMAP.md", roadmap)

validation = read("docs/PRODUCTION_VALIDATION.md")
if "Preview profile assignment" not in validation:
    validation += r'''

## Preview profile assignment boundary

Color Management can temporarily assign another ICC/ICM profile to the project preview. This is deliberately excluded from production interchange validation because it must not affect exported TIFF samples or metadata. When validating a build, verify that changing the assigned preview profile changes only the on-screen composite and `.shade` JSON, while identity export retains the source TIFF's original embedded ICC bytes.
'''
write("docs/PRODUCTION_VALIDATION.md", validation)

release_notes = read("RELEASE_NOTES.md")
unreleased = r'''# Unreleased

- Replace generic `ICC: managed` metadata text with the active ICC profile description; the profile label is clickable and opens Color Management.
- Add project-owned non-destructive Preview Profile Assignment: use embedded ICC or assign a compatible ICC/ICM only for Shade Editor preview, with Rendering Intent and optional Black Point Compensation stored in `.shade`.
- Add searchable Windows ICC/ICM catalog with Project View-style typing, Up/Down selection and Enter assignment, plus Browse and compatibility checks against the active TIFF base color model.
- Keep assigned profiles strictly out of production export; TIFF samples, embedded ICC and Photoshop resources remain unchanged.
- Consolidate active Rust sources under canonical filenames and remove obsolete versioned implementations and stale one-off documentation.
- Update README/architecture/roadmap to reflect the actual Levels → Mixer → Curve order and current ICC preview boundary.

'''
if not release_notes.startswith("# Unreleased"):
    release_notes = unreleased + release_notes
write("RELEASE_NOTES.md", release_notes)

print("Preview profile assignment migration applied.")
