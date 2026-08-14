from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]

def read(path):
    return (ROOT / path).read_text(encoding="utf-8")

def write(path, text):
    p = ROOT / path
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text, encoding="utf-8", newline="\n")

def replace_once(text, old, new, label):
    if old not in text:
        raise RuntimeError(f"{label}: anchor not found")
    return text.replace(old, new, 1)

def regex_once(text, pattern, repl, label, flags=re.S):
    out, count = re.subn(pattern, repl, text, count=1, flags=flags)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1 replacement, got {count}")
    return out

# ---------------- Cargo/version ----------------
cargo = read("Cargo.toml")
cargo = replace_once(cargo, 'version = "0.16.0"', 'version = "0.17.0"', "version bump")
write("Cargo.toml", cargo)

# ---------------- model preview settings ----------------
model = read("src/model.rs")
model = replace_once(
    model,
    '''    pub assigned_profile_path: Option<String>,
    pub rendering_intent: PreviewRenderingIntent,
    pub black_point_compensation: bool,
''',
    '''    pub assigned_profile_path: Option<String>,
    pub rendering_intent: PreviewRenderingIntent,
    pub black_point_compensation: bool,
    /// Optional printer/RIP proof-device profile used only for on-screen soft proof.
    pub soft_proof_enabled: bool,
    pub proof_profile_path: Option<String>,
    pub proofing_intent: PreviewRenderingIntent,
''',
    "preview proof fields",
)
model = replace_once(
    model,
    '''            assigned_profile_path: None,
            rendering_intent: PreviewRenderingIntent::Perceptual,
            black_point_compensation: false,
''',
    '''            assigned_profile_path: None,
            rendering_intent: PreviewRenderingIntent::Perceptual,
            black_point_compensation: false,
            soft_proof_enabled: false,
            proof_profile_path: None,
            proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
''',
    "preview proof defaults",
)
# expand existing preview settings test if present
needle = '''        assert!(!project.preview_color.black_point_compensation);
'''
if needle in model:
    model = replace_once(
        model,
        needle,
        needle + '''        assert!(!project.preview_color.soft_proof_enabled);
        assert!(project.preview_color.proof_profile_path.is_none());
        assert_eq!(
            project.preview_color.proofing_intent,
            PreviewRenderingIntent::RelativeColorimetric
        );
''',
        "preview proof test",
    )
write("src/model.rs", model)

# ---------------- color management (full canonical rewrite) ----------------
color_management = r'''use std::fs;
use std::path::{Path, PathBuf};

use lcms2::{
    ColorSpaceSignature, Flags, InfoType, Intent, Locale, PixelFormat, Profile,
    ProfileClassSignature, Transform,
};

use crate::model::{PreviewRenderingIntent, ShadeProject};
use crate::tiff_io::{ColorModel, TiffMetadata};

#[derive(Clone, Debug)]
pub struct InstalledIccProfile {
    pub path: PathBuf,
    pub description: String,
    color_space: ColorSpaceSignature,
    device_class: ProfileClassSignature,
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

    pub fn device_class_label(&self) -> String {
        profile_class_label(self.device_class)
    }

    pub fn compatible_with(&self, model: ColorModel) -> bool {
        expected_color_space(model).is_some_and(|expected| expected == self.color_space)
    }

    pub fn is_output_profile(&self) -> bool {
        self.device_class == ProfileClassSignature::OutputClass
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
            || self.device_class_label().to_lowercase().contains(&query)
    }
}

#[derive(Clone, Debug)]
pub struct PreviewColorConfig {
    pub enabled: bool,
    pub intent: PreviewRenderingIntent,
    pub black_point_compensation: bool,
    pub assigned_profile_path: Option<PathBuf>,
    pub soft_proof_enabled: bool,
    pub proof_profile_path: Option<PathBuf>,
    pub proofing_intent: PreviewRenderingIntent,
}

impl PreviewColorConfig {
    pub fn from_project(project: &ShadeProject) -> Self {
        Self {
            enabled: project.preview_color.enabled,
            intent: project.preview_color.rendering_intent,
            black_point_compensation: project.preview_color.black_point_compensation,
            assigned_profile_path: project
                .preview_color
                .assigned_profile_path
                .as_ref()
                .map(PathBuf::from),
            soft_proof_enabled: project.preview_color.soft_proof_enabled,
            proof_profile_path: project
                .preview_color
                .proof_profile_path
                .as_ref()
                .map(PathBuf::from),
            proofing_intent: project.preview_color.proofing_intent,
        }
    }
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
        proof_description: Option<String>,
        proofing_intent: Option<PreviewRenderingIntent>,
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
            Self::Disabled => {
                "Project color preview is disabled. TIFF data and metadata are unchanged.".to_owned()
            }
            Self::NoEmbeddedProfile => "This TIFF has no embedded ICC profile and no preview profile is assigned. Shade Editor is using its unmanaged display fallback.".to_owned(),
            Self::Applied {
                description,
                intent,
                source,
                black_point_compensation,
                proof_description,
                proofing_intent,
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
                if let (Some(proof), Some(proof_intent)) =
                    (proof_description.as_ref(), proofing_intent.as_ref())
                {
                    format!(
                        "{} ({source}) → printer/RIP soft proof '{}' → sRGB display · source {} intent · proof {} intent{bpc}. Preview-only; TIFF samples, embedded ICC and Photoshop metadata are unchanged.",
                        description,
                        proof,
                        intent.label(),
                        proof_intent.label(),
                    )
                } else {
                    format!(
                        "{} ({source}) → sRGB preview · {} intent{bpc}. Preview-only; TIFF samples, embedded ICC and Photoshop metadata are unchanged.",
                        description,
                        intent.label(),
                    )
                }
            }
            Self::Fallback { reason, .. } => format!(
                "The requested color-management transform could not be used ({reason}). Shade Editor fell back to the unmanaged display conversion; TIFF data is unchanged."
            ),
        }
    }

    pub fn is_managed(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }

    pub fn is_problem(&self) -> bool {
        matches!(self, Self::Fallback { .. })
    }

    pub fn is_soft_proofing(&self) -> bool {
        matches!(
            self,
            Self::Applied {
                proof_description: Some(_),
                ..
            }
        )
    }
}

enum BaseTransform {
    Rgb(Transform<[u16; 3], [u8; 3]>),
    Cmyk(Transform<[u16; 4], [u8; 3]>),
    Gray(Transform<[u16; 1], [u8; 3]>),
}

/// Preview-only ICC transform. Assigned source profiles and proof profiles are
/// never written to TIFF or consumed by production export.
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

        let proof = if config.soft_proof_enabled {
            let Some(path) = config.proof_profile_path.as_ref() else {
                return Self::fallback(
                    "printer/RIP soft proof is enabled but no proof profile is selected".to_owned(),
                    Some("Soft proof".to_owned()),
                );
            };
            let proof = match Profile::new_file(path) {
                Ok(profile) => profile,
                Err(err) => {
                    return Self::fallback(
                        format!("cannot open proof profile {}: {err}", path.display()),
                        Some(profile_path_label(path)),
                    );
                }
            };
            if proof.device_class() != ProfileClassSignature::OutputClass {
                return Self::fallback(
                    format!(
                        "proof profile '{}' is {}, not an output/printer profile",
                        profile_description(&proof),
                        profile_class_label(proof.device_class()),
                    ),
                    Some(profile_path_label(path)),
                );
            }
            Some((proof, profile_path_label(path)))
        } else {
            None
        };

        let proofing_intent = to_lcms_intent(config.proofing_intent);
        let proof_flags = if bpc {
            Flags::SOFT_PROOFING | Flags::BLACKPOINT_COMPENSATION
        } else {
            Flags::SOFT_PROOFING
        };

        macro_rules! make_transform {
            ($input:expr, $variant:ident) => {{
                let result = if let Some((proof_profile, _)) = proof.as_ref() {
                    Transform::new_proofing(
                        &source,
                        $input,
                        &destination,
                        PixelFormat::RGB_8,
                        proof_profile,
                        intent,
                        proofing_intent,
                        proof_flags,
                    )
                } else if bpc {
                    Transform::new_flags(
                        &source,
                        $input,
                        &destination,
                        PixelFormat::RGB_8,
                        intent,
                        Flags::BLACKPOINT_COMPENSATION,
                    )
                } else {
                    Transform::new(
                        &source,
                        $input,
                        &destination,
                        PixelFormat::RGB_8,
                        intent,
                    )
                };
                result.map(BaseTransform::$variant)
            }};
        }

        let transform = match metadata.color_model {
            ColorModel::Rgb => make_transform!(PixelFormat::RGB_16, Rgb),
            ColorModel::Cmyk => make_transform!(PixelFormat::CMYK_16, Cmyk),
            ColorModel::Gray => make_transform!(PixelFormat::GRAY_16, Gray),
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
                    proof_description: proof.as_ref().map(|(_, label)| label.clone()),
                    proofing_intent: proof.as_ref().map(|_| config.proofing_intent),
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
        device_class: profile.device_class(),
    })
}

/// Enumerate installed profile files from the Windows color directory.
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
                .unwrap_or_else(|| "ICC profile".to_owned())
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

fn profile_class_label(class: ProfileClassSignature) -> String {
    match class {
        ProfileClassSignature::InputClass => "Input".to_owned(),
        ProfileClassSignature::DisplayClass => "Display".to_owned(),
        ProfileClassSignature::OutputClass => "Output / printer".to_owned(),
        ProfileClassSignature::LinkClass => "DeviceLink".to_owned(),
        ProfileClassSignature::AbstractClass => "Abstract".to_owned(),
        ProfileClassSignature::ColorSpaceClass => "Color space".to_owned(),
        ProfileClassSignature::NamedColorClass => "Named color".to_owned(),
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
    use crate::tiff_io::TiffMetadata;

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
            soft_proof_enabled: false,
            proof_profile_path: None,
            proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
        }
    }

    #[test]
    fn no_profile_is_reported_without_modifying_data() {
        let transform = PreviewColorTransform::new(&rgb_metadata(None), config());
        assert!(matches!(
            transform.status(),
            PreviewColorStatus::NoEmbeddedProfile
        ));
        assert!(transform.base_rgb8(&[vec![0], vec![0], vec![0]], 1).is_none());
    }

    #[test]
    fn disabled_preview_is_explicit() {
        let mut cfg = config();
        cfg.enabled = false;
        let transform = PreviewColorTransform::new(&rgb_metadata(None), cfg);
        assert!(matches!(transform.status(), PreviewColorStatus::Disabled));
    }
}
'''
write("src/color_management.rs", color_management)

# ---------------- thumbnail now uses the exact color-managed/proof render ----------------
thumbnail = read("src/thumbnail.rs")
thumbnail = replace_once(
    thumbnail,
    '''use crate::model::{ProjectThumbnail, ShadeProject};
use crate::render;
''',
    '''use crate::color_management::{PreviewColorConfig, PreviewColorTransform};
use crate::model::{ProjectThumbnail, ShadeProject};
use crate::render;
''',
    "thumbnail imports",
)
thumbnail = replace_once(
    thumbnail,
    '''    let planes = render::adjusted_planes(face, project);
    let rgba = render::rgba_from_planes(face, &planes, None);
''',
    '''    let planes = render::adjusted_planes(face, project);
    let color = PreviewColorTransform::new(
        &face.metadata,
        PreviewColorConfig::from_project(project),
    );
    let rgba = render::rgba_from_planes_with_color(face, &planes, None, &color);
''',
    "thumbnail managed render",
)
write("src/thumbnail.rs", thumbnail)

# ---------------- History labels ----------------
history = read("src/history.rs")
history = history.replace('format!("{kind} - {}", channels[0])', 'format!("{kind} · {}", channels[0])')
history = history.replace('format!("{kind} - {} channels", channels.len())', 'format!("{kind} · {} channels", channels.len())')
history = history.replace('"Levels - Cyan"', '"Levels · Cyan"')
write("src/history.rs", history)

# ---------------- export naming + folder template ----------------
export_batch = r'''use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_EXPORT_TEMPLATE: &str = "{project}_{face}_{snapshot}_{date}";
pub const DEFAULT_FOLDER_TEMPLATE: &str = "";

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConflictPolicy {
    Overwrite,
    Skip,
    #[default]
    AutoNumber,
}

impl ConflictPolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Overwrite => "Overwrite",
            Self::Skip => "Skip",
            Self::AutoNumber => "Auto-number",
        }
    }
}

pub struct ExportNameContext<'a> {
    pub shade_name: Option<&'a str>,
    pub project_name: &'a str,
    pub snapshot_code: &'a str,
    pub face_number: usize,
    pub face_name: &'a str,
    pub source_name: &'a str,
    pub date: &'a str,
}

fn render_tokens(template: &str, context: &ExportNameContext<'_>) -> String {
    let project_name = nonempty(context.project_name).unwrap_or("Shade");
    let shade_name = context
        .shade_name
        .and_then(nonempty)
        .unwrap_or(project_name);
    let snapshot = nonempty(context.snapshot_code).unwrap_or("Working");
    let face = nonempty(context.face_name).unwrap_or("face");
    let source = nonempty(context.source_name).unwrap_or(face);
    let mut value = template.to_owned();

    // New compact tokens.
    value = value.replace("{project}", project_name);
    value = value.replace("{face}", face);
    value = value.replace("{snapshot}", snapshot);
    value = value.replace("{source}", source);
    value = value.replace("{date}", context.date);

    // Backward-compatible tokens from earlier Shade Editor builds.
    value = value.replace("{shade-name|project-name}", shade_name);
    value = value.replace("{shade-name}", shade_name);
    value = value.replace("{project-name}", project_name);
    value = value.replace("{snapshot-code}", snapshot);
    value = value.replace("{face-number}", &context.face_number.to_string());
    value = value.replace("{face-name}", face);
    value
}

pub fn render_export_filename(template: &str, context: &ExportNameContext<'_>) -> String {
    let template = if template.trim().is_empty() {
        DEFAULT_EXPORT_TEMPLATE
    } else {
        template
    };
    let stem = sanitize_filename_stem(&render_tokens(template, context));
    format!("{stem}.tif")
}

pub fn render_export_folder(
    base_folder: &Path,
    template: &str,
    context: &ExportNameContext<'_>,
) -> PathBuf {
    if template.trim().is_empty() {
        return base_folder.to_path_buf();
    }
    let rendered = render_tokens(template, context);
    let mut output = base_folder.to_path_buf();
    for component in rendered.split(['/', '\\']) {
        let component = component.trim();
        if component.is_empty() || component == "." || component == ".." {
            continue;
        }
        output.push(sanitize_folder_component(component));
    }
    output
}

pub fn sanitize_filename_stem(value: &str) -> String {
    sanitize_component(value, "shade-export")
}

fn sanitize_folder_component(value: &str) -> String {
    sanitize_component(value, "output")
}

fn sanitize_component(value: &str, fallback: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        let invalid =
            ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
        output.push(if invalid { '-' } else { ch });
    }
    let trimmed = output
        .trim()
        .trim_matches('.')
        .trim()
        .trim_matches('-')
        .trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub enum DestinationDecision {
    Write(PathBuf),
    Skip(PathBuf),
}

pub fn resolve_destination(
    folder: &Path,
    filename: &str,
    policy: ConflictPolicy,
) -> DestinationDecision {
    let mut reserved = BTreeSet::new();
    resolve_destination_reserved(folder, filename, policy, &mut reserved)
}

pub fn resolve_destination_reserved(
    folder: &Path,
    filename: &str,
    policy: ConflictPolicy,
    reserved: &mut BTreeSet<PathBuf>,
) -> DestinationDecision {
    let target = folder.join(filename);
    if policy == ConflictPolicy::Overwrite {
        reserved.insert(target.clone());
        return DestinationDecision::Write(target);
    }
    if !target.exists() && !reserved.contains(&target) {
        reserved.insert(target.clone());
        return DestinationDecision::Write(target);
    }
    if policy == ConflictPolicy::Skip {
        return DestinationDecision::Skip(target);
    }

    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "shade-export".to_owned());
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tif".to_owned());
    for number in 2u64.. {
        let candidate = folder.join(format!("{stem} ({number}).{extension}"));
        if !candidate.exists() && !reserved.contains(&candidate) {
            reserved.insert(candidate.clone());
            return DestinationDecision::Write(candidate);
        }
    }
    unreachable!()
}

pub fn folder_tiff_count(folder: &Path) -> usize {
    let Ok(entries) = fs::read_dir(folder) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("tif") || ext.eq_ignore_ascii_case("tiff")
                })
        })
        .count()
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context<'a>() -> ExportNameContext<'a> {
        ExportNameContext {
            shade_name: Some("Tile Blue"),
            project_name: "Project A",
            snapshot_code: "T-42",
            face_number: 3,
            face_name: "Face 3",
            source_name: "source-file",
            date: "2026-08-14",
        }
    }

    #[test]
    fn new_template_tokens_render() {
        let name = render_export_filename("{project}_{face}_{snapshot}_{date}", &context());
        assert_eq!(name, "Project A_Face 3_T-42_2026-08-14.tif");
    }

    #[test]
    fn legacy_tokens_still_render() {
        let name = render_export_filename(
            "{shade-name|project-name} - ({snapshot-code}) - {face-number}",
            &context(),
        );
        assert_eq!(name, "Tile Blue - (T-42) - 3.tif");
    }

    #[test]
    fn folder_template_is_safe_and_nested() {
        let folder = render_export_folder(
            Path::new(r"C:\exports"),
            "{project}/{date}/{snapshot}",
            &context(),
        );
        assert!(folder.ends_with(Path::new(r"Project A\2026-08-14\T-42")));
    }

    #[test]
    fn windows_reserved_filename_characters_are_sanitized() {
        assert_eq!(sanitize_filename_stem("A*B:C?D"), "A-B-C-D");
    }
}
'''
write("src/export_batch.rs", export_batch)

# ---------------- settings folder template ----------------
settings = read("src/settings.rs")
settings = replace_once(
    settings,
    'use crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE};',
    'use crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE, DEFAULT_FOLDER_TEMPLATE};',
    "settings import",
)
settings = replace_once(
    settings,
    '''    pub export_all_template: String,
    pub export_all_conflict_policy: ConflictPolicy,
''',
    '''    pub export_all_template: String,
    pub export_folder_template: String,
    pub export_all_conflict_policy: ConflictPolicy,
''',
    "settings folder field",
)
settings = replace_once(
    settings,
    '''            export_all_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),
            export_all_conflict_policy: ConflictPolicy::AutoNumber,
''',
    '''            export_all_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),
            export_folder_template: DEFAULT_FOLDER_TEMPLATE.to_owned(),
            export_all_conflict_policy: ConflictPolicy::AutoNumber,
''',
    "settings folder default",
)
if 'assert_eq!(settings.export_all_template, DEFAULT_EXPORT_TEMPLATE);' in settings:
    settings = replace_once(
        settings,
        '        assert_eq!(settings.export_all_template, DEFAULT_EXPORT_TEMPLATE);\n',
        '        assert_eq!(settings.export_all_template, DEFAULT_EXPORT_TEMPLATE);\n        assert_eq!(settings.export_folder_template, DEFAULT_FOLDER_TEMPLATE);\n',
        "settings test folder default",
    )
write("src/settings.rs", settings)

# ---------------- Export Queue backend ----------------
export_queue = r'''use std::sync::mpsc;
use std::thread;

use crate::export;
use crate::model::ShadeProject;
use crate::validation;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportQueueStatus {
    Waiting,
    Processing,
    Done,
    Failed,
    Cancelled,
}

impl ExportQueueStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::Processing => "Processing",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug)]
pub struct ExportQueueMark {
    pub snapshot_id: u64,
    pub face_key: String,
    pub folder: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ExportQueueSpec {
    pub label: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub project: ShadeProject,
    pub default_dpi: f64,
    pub force_lzw: bool,
    pub validate_after_export: bool,
    pub mark: Option<ExportQueueMark>,
}

#[derive(Clone, Debug)]
pub struct ExportQueueItem {
    pub id: u64,
    pub label: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub status: ExportQueueStatus,
    pub progress: f32,
    pub detail: String,
    pub error: Option<String>,
    spec: ExportQueueSpec,
}

#[derive(Clone, Debug)]
pub struct ExportQueueCompletion {
    pub id: u64,
    pub result: Result<String, String>,
    pub mark: Option<ExportQueueMark>,
}

enum ExportQueueEvent {
    Progress {
        id: u64,
        fraction: f32,
        detail: String,
    },
    Finished {
        id: u64,
        result: Result<String, String>,
        mark: Option<ExportQueueMark>,
    },
}

pub struct ExportQueue {
    items: Vec<ExportQueueItem>,
    next_id: u64,
    active_id: Option<u64>,
    stop_after_current: bool,
    tx: mpsc::Sender<ExportQueueEvent>,
    rx: mpsc::Receiver<ExportQueueEvent>,
}

impl Default for ExportQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportQueue {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            items: Vec::new(),
            next_id: 1,
            active_id: None,
            stop_after_current: false,
            tx,
            rx,
        }
    }

    pub fn enqueue(&mut self, spec: ExportQueueSpec) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.items.push(ExportQueueItem {
            id,
            label: spec.label.clone(),
            source: spec.source.clone(),
            destination: spec.destination.clone(),
            status: ExportQueueStatus::Waiting,
            progress: 0.0,
            detail: String::new(),
            error: None,
            spec,
        });
        id
    }

    pub fn items(&self) -> &[ExportQueueItem] {
        &self.items
    }

    pub fn pending_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item.status, ExportQueueStatus::Waiting | ExportQueueStatus::Processing))
            .count()
    }

    pub fn has_pending(&self) -> bool {
        self.pending_count() > 0
    }

    pub fn active_summary(&self) -> Option<(f32, String)> {
        let id = self.active_id?;
        let item = self.items.iter().find(|item| item.id == id)?;
        let text = if item.detail.trim().is_empty() {
            item.label.clone()
        } else {
            format!("{} · {}", item.label, item.detail)
        };
        Some((item.progress, text))
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        match item.status {
            ExportQueueStatus::Waiting => {
                item.status = ExportQueueStatus::Cancelled;
                item.detail = "Cancelled before processing".to_owned();
                true
            }
            ExportQueueStatus::Processing => {
                self.stop_after_current = true;
                item.detail = "Stop requested · current atomic export will finish safely".to_owned();
                true
            }
            _ => false,
        }
    }

    pub fn retry(&mut self, id: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if !matches!(item.status, ExportQueueStatus::Failed | ExportQueueStatus::Cancelled) {
            return false;
        }
        item.status = ExportQueueStatus::Waiting;
        item.progress = 0.0;
        item.detail.clear();
        item.error = None;
        true
    }

    pub fn cancel_all_waiting(&mut self) {
        for item in &mut self.items {
            if item.status == ExportQueueStatus::Waiting {
                item.status = ExportQueueStatus::Cancelled;
                item.detail = "Cancelled before processing".to_owned();
            }
        }
    }

    pub fn clear_finished(&mut self) {
        self.items.retain(|item| !item.status.finished());
    }

    pub fn poll(&mut self) -> Vec<ExportQueueCompletion> {
        let mut completions = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            match event {
                ExportQueueEvent::Progress {
                    id,
                    fraction,
                    detail,
                } => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        item.progress = fraction.clamp(0.0, 1.0);
                        item.detail = detail;
                    }
                }
                ExportQueueEvent::Finished { id, result, mark } => {
                    self.active_id = None;
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        item.progress = 1.0;
                        match &result {
                            Ok(message) => {
                                item.status = ExportQueueStatus::Done;
                                item.detail = message.clone();
                                item.error = None;
                            }
                            Err(err) => {
                                item.status = ExportQueueStatus::Failed;
                                item.detail = "Export failed".to_owned();
                                item.error = Some(err.clone());
                            }
                        }
                    }
                    completions.push(ExportQueueCompletion { id, result, mark });
                    if self.stop_after_current {
                        self.stop_after_current = false;
                        self.cancel_all_waiting();
                    }
                }
            }
        }

        if self.active_id.is_none() && !self.stop_after_current {
            self.start_next();
        }
        completions
    }

    fn start_next(&mut self) {
        let Some(index) = self
            .items
            .iter()
            .position(|item| item.status == ExportQueueStatus::Waiting)
        else {
            return;
        };
        let id = self.items[index].id;
        let spec = self.items[index].spec.clone();
        self.items[index].status = ExportQueueStatus::Processing;
        self.items[index].progress = 0.0;
        self.items[index].detail = "Starting".to_owned();
        self.items[index].error = None;
        self.active_id = Some(id);

        let tx = self.tx.clone();
        thread::spawn(move || {
            let validate_after_export = spec.validate_after_export;
            let progress_tx = tx.clone();
            let result = export::export_face_with_progress_options(
                &spec.source,
                &spec.destination,
                &spec.project,
                spec.default_dpi,
                export::ExportOptions {
                    force_lzw: spec.force_lzw,
                },
                move |fraction, detail| {
                    let _ = progress_tx.send(ExportQueueEvent::Progress {
                        id,
                        fraction: if validate_after_export {
                            fraction * 0.90
                        } else {
                            fraction
                        },
                        detail: detail.to_owned(),
                    });
                },
            )
            .and_then(|_| {
                if spec.validate_after_export {
                    let _ = tx.send(ExportQueueEvent::Progress {
                        id,
                        fraction: 0.94,
                        detail: "Validating exported TIFF".to_owned(),
                    });
                    let verified = validation::validate_export_transport_with_options(
                        &spec.source,
                        &spec.destination,
                        spec.force_lzw,
                    )?;
                    Ok(format!("Done · {verified}"))
                } else {
                    Ok("Done".to_owned())
                }
            });

            let mark = result.as_ref().ok().and(spec.mark);
            let _ = tx.send(ExportQueueEvent::Finished { id, result, mark });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_item_can_be_cancelled_and_retried_without_io() {
        let mut queue = ExportQueue::new();
        let id = queue.enqueue(ExportQueueSpec {
            label: "test".to_owned(),
            source: PathBuf::from("missing.tif"),
            destination: PathBuf::from("out.tif"),
            project: ShadeProject::default(),
            default_dpi: 220.0,
            force_lzw: true,
            validate_after_export: false,
            mark: None,
        });
        assert!(queue.cancel(id));
        assert_eq!(queue.items()[0].status, ExportQueueStatus::Cancelled);
        assert!(queue.retry(id));
        assert_eq!(queue.items()[0].status, ExportQueueStatus::Waiting);
    }
}
'''
write("src/export_queue.rs", export_queue)

# ---------------- TIFF inspector backend ----------------
tiff_inspect = r'''use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use tiff::decoder::{Decoder, Limits};
use tiff::tags::Tag;

use crate::color_management;
use crate::dpi;
use crate::tiff_io::{self, ChunkStorage};

#[derive(Clone, Debug)]
pub struct TiffInspection {
    pub path: PathBuf,
    pub report: String,
}

pub fn inspect(path: &Path, default_dpi: f64) -> Result<TiffInspection, String> {
    let stream = tiff_io::stream_info(path)?;
    let metadata = &stream.metadata;
    let file_size = fs::metadata(path)
        .map_err(|err| format!("Cannot read TIFF file metadata: {err}"))?
        .len();

    let file = File::open(path).map_err(|err| format!("Cannot inspect TIFF tags: {err}"))?;
    let mut decoder = Decoder::new(BufReader::new(file))
        .map_err(|err| format!("Cannot initialize TIFF inspector: {err}"))?
        .with_limits(Limits::unlimited());

    let photometric = decoder
        .find_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
        .ok()
        .flatten();
    let extra_samples = decoder
        .get_tag_u64_vec(Tag::ExtraSamples)
        .unwrap_or_default()
        .into_iter()
        .map(|value| value as u16)
        .collect::<Vec<_>>();

    let container = tiff_container(path)?;
    let dpi = dpi::read_dpi(path, default_dpi);
    let estimated = u128::from(metadata.width)
        .saturating_mul(u128::from(metadata.height))
        .saturating_mul(metadata.samples_per_pixel as u128)
        .saturating_mul(u128::from(metadata.bit_depth))
        / 8;

    let icc = color_management::embedded_profile_description(metadata)
        .unwrap_or_else(|| "None".to_owned());
    let photoshop_bytes = metadata
        .photoshop_resources
        .as_ref()
        .map(Vec::len)
        .unwrap_or(0);
    let image_source_bytes = metadata
        .photoshop_image_source_data
        .as_ref()
        .map(Vec::len)
        .unwrap_or(0);

    let mut report = String::new();
    macro_rules! line {
        ($($arg:tt)*) => {{
            report.push_str(&format!($($arg)*));
            report.push('\n');
        }};
    }

    line!("Shade Editor TIFF Inspection");
    line!("============================");
    line!("File: {}", path.display());
    line!("Container: {container}");
    line!("File size: {}", format_bytes(file_size as u128));
    line!("Dimensions: {} × {} px", metadata.width, metadata.height);
    line!("Bits per sample: {}", metadata.bit_depth);
    line!(
        "PhotometricInterpretation: {}",
        photometric_label(photometric)
    );
    line!(
        "PlanarConfiguration: {}",
        planar_label(stream.planar_configuration)
    );
    line!(
        "Compression: {}",
        compression_label(metadata.compression)
    );
    line!("Predictor: {}", predictor_label(metadata.predictor));
    line!("SamplesPerPixel: {}", metadata.samples_per_pixel);
    line!("Base color model: {}", metadata.color_model.title());
    line!("Base channel count: {}", metadata.base_channel_count);
    line!(
        "ExtraSamples: {}",
        if extra_samples.is_empty() {
            "None".to_owned()
        } else {
            extra_samples
                .iter()
                .map(|value| format!("{} ({})", value, extra_sample_label(*value)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    line!(
        "Storage: {}",
        match stream.storage {
            ChunkStorage::Strips => "Strips",
            ChunkStorage::Tiles => "Tiles",
        }
    );
    line!(
        "Coding unit: {} × {} px · {} unit(s) · streamable={}",
        stream.chunk_width,
        stream.chunk_height,
        stream.coding_unit_count,
        stream.streamable
    );
    line!(
        "DPI: {}",
        if dpi.has_physical_resolution {
            format!(
                "{:.4} × {:.4} dpi (ResolutionUnit={})",
                dpi.dpi_x, dpi.dpi_y, dpi.unit
            )
        } else {
            "No physical source DPI tags".to_owned()
        }
    );
    line!("ICC: {icc}");
    line!(
        "ICC payload: {}",
        metadata
            .icc_profile
            .as_ref()
            .map(|bytes| format_bytes(bytes.len() as u128))
            .unwrap_or_else(|| "None".to_owned())
    );
    line!(
        "Photoshop Image Resources (34377): {}",
        if photoshop_bytes == 0 {
            "None".to_owned()
        } else {
            format_bytes(photoshop_bytes as u128)
        }
    );
    line!(
        "Photoshop ImageSourceData (37724): {}",
        if image_source_bytes == 0 {
            "None".to_owned()
        } else {
            format_bytes(image_source_bytes as u128)
        }
    );
    line!(
        "Estimated uncompressed sample data: {}",
        format_bytes(estimated)
    );
    line!("");
    line!("Channel order");
    line!("-------------");
    for (index, name) in metadata.channel_names.iter().enumerate() {
        let role = if index < metadata.base_channel_count {
            "base".to_owned()
        } else {
            match metadata
                .channel_display_info
                .get(index)
                .and_then(|value| *value)
            {
                Some(info) if info.is_spot() => {
                    format!("Spot · Solidity {:.0}%", info.solidity * 100.0)
                }
                Some(_) => "Alpha / auxiliary".to_owned(),
                None => "Extra (type not declared)".to_owned(),
            }
        };
        line!("{:02}. {} · {}", index + 1, name, role);
    }

    let spot_names = metadata
        .channel_names
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            metadata
                .channel_display_info
                .get(*index)
                .and_then(|value| *value)
                .is_some_and(|info| info.is_spot())
        })
        .map(|(_, name)| name.clone())
        .collect::<Vec<_>>();
    line!("");
    line!(
        "Declared Spot order: {}",
        if spot_names.is_empty() {
            "None".to_owned()
        } else {
            spot_names.join(" → ")
        }
    );

    Ok(TiffInspection {
        path: path.to_path_buf(),
        report,
    })
}

fn tiff_container(path: &Path) -> Result<&'static str, String> {
    let mut file = File::open(path).map_err(|err| format!("Cannot read TIFF header: {err}"))?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)
        .map_err(|err| format!("Cannot read TIFF header: {err}"))?;
    let little = &header[..2] == b"II";
    let big = &header[..2] == b"MM";
    if !little && !big {
        return Err("Invalid TIFF byte-order signature.".to_owned());
    }
    let magic = if little {
        u16::from_le_bytes([header[2], header[3]])
    } else {
        u16::from_be_bytes([header[2], header[3]])
    };
    match magic {
        42 => Ok("Classic TIFF"),
        43 => Ok("BigTIFF"),
        other => Err(format!("Unknown TIFF magic value {other}.")),
    }
}

fn photometric_label(value: Option<u16>) -> String {
    match value {
        Some(0) => "0 · WhiteIsZero".to_owned(),
        Some(1) => "1 · BlackIsZero".to_owned(),
        Some(2) => "2 · RGB".to_owned(),
        Some(3) => "3 · Palette".to_owned(),
        Some(4) => "4 · Transparency mask".to_owned(),
        Some(5) => "5 · Separated / CMYK".to_owned(),
        Some(6) => "6 · YCbCr".to_owned(),
        Some(8) => "8 · CIELab".to_owned(),
        Some(value) => format!("{value}"),
        None => "Not present".to_owned(),
    }
}

fn planar_label(value: u16) -> String {
    match value {
        1 => "1 · Chunky / contiguous".to_owned(),
        2 => "2 · Planar / separate".to_owned(),
        other => format!("{other}"),
    }
}

fn compression_label(value: Option<u16>) -> String {
    match value {
        Some(1) => "1 · None".to_owned(),
        Some(5) => "5 · LZW".to_owned(),
        Some(7) => "7 · JPEG".to_owned(),
        Some(8) => "8 · Deflate".to_owned(),
        Some(32773) => "32773 · PackBits".to_owned(),
        Some(32946) => "32946 · Deflate".to_owned(),
        Some(value) => format!("{value}"),
        None => "Not present".to_owned(),
    }
}

fn predictor_label(value: Option<u16>) -> String {
    match value {
        Some(1) => "1 · None".to_owned(),
        Some(2) => "2 · Horizontal differencing".to_owned(),
        Some(3) => "3 · Floating point".to_owned(),
        Some(value) => format!("{value}"),
        None => "Not present".to_owned(),
    }
}

fn extra_sample_label(value: u16) -> &'static str {
    match value {
        0 => "Unspecified",
        1 => "Associated alpha",
        2 => "Unassociated alpha",
        _ => "Unknown",
    }
}

fn format_bytes(bytes: u128) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let value = bytes as f64;
    if value >= TIB {
        format!("{:.2} TiB ({} bytes)", value / TIB, bytes)
    } else if value >= GIB {
        format!("{:.2} GiB ({} bytes)", value / GIB, bytes)
    } else if value >= MIB {
        format!("{:.2} MiB ({} bytes)", value / MIB, bytes)
    } else if value >= KIB {
        format!("{:.2} KiB ({} bytes)", value / KIB, bytes)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_tag_labels_are_human_readable() {
        assert!(photometric_label(Some(5)).contains("CMYK"));
        assert!(compression_label(Some(5)).contains("LZW"));
        assert!(planar_label(2).contains("Planar"));
    }
}
'''
write("src/tiff_inspect.rs", tiff_inspect)

# ---------------- main module/import/state wiring ----------------
main = read("src/main.rs")
main = replace_once(main, "mod export_batch;\n", "mod export_batch;\nmod export_queue;\n", "main export queue module")
main = replace_once(main, "mod tiff_io;\n", "mod tiff_io;\nmod tiff_inspect;\n", "main tiff inspect module")
main = replace_once(
    main,
    "use std::collections::{BTreeMap, VecDeque};",
    "use std::collections::{BTreeMap, BTreeSet, VecDeque};",
    "main BTreeSet import",
)

# Old SnapshotExport structs are still used by validation jobs; keep them.
# App state.
main = replace_once(
    main,
    '''    icc_show_incompatible: bool,
    show_about: bool,
''',
    '''    icc_show_incompatible: bool,
    proof_profile_selected: Option<String>,
    show_about: bool,
''',
    "proof selected state",
)
main = replace_once(
    main,
    '''    show_export_all: bool,
    export_all_folder: String,
''',
    '''    show_export_all: bool,
    export_all_folder: String,
    show_export_queue: bool,
    export_queue: export_queue::ExportQueue,
    show_tiff_inspector: bool,
    tiff_inspection_path: Option<PathBuf>,
    tiff_inspection_report: String,
''',
    "queue/inspect state",
)
main = replace_once(
    main,
    '''            icc_show_incompatible: false,
            show_about: false,
''',
    '''            icc_show_incompatible: false,
            proof_profile_selected: None,
            show_about: false,
''',
    "proof state init",
)
main = replace_once(
    main,
    '''            show_export_all: false,
            export_all_folder: String::new(),
            remind_after_export: false,
''',
    '''            show_export_all: false,
            export_all_folder: String::new(),
            show_export_queue: false,
            export_queue: export_queue::ExportQueue::new(),
            show_tiff_inspector: false,
            tiff_inspection_path: None,
            tiff_inspection_report: String::new(),
            remind_after_export: false,
''',
    "queue/inspect init",
)
main = replace_once(
    main,
    '''        self.icc_profile_selected = None;
        self.remind_after_export = false;
''',
    '''        self.icc_profile_selected = None;
        self.proof_profile_selected = None;
        self.remind_after_export = false;
''',
    "new project proof reset",
)

# Render uses canonical project->color config helper including proof.
main = regex_once(
    main,
    r'''        let color_config = PreviewColorConfig \{
            enabled: self\.project\.preview_color\.enabled,
            intent: self\.project\.preview_color\.rendering_intent,
            black_point_compensation: self\.project\.preview_color\.black_point_compensation,
            assigned_profile_path: self
                \.project
                \.preview_color
                \.assigned_profile_path
                \.as_ref\(\)
                \.map\(PathBuf::from\),
        \};''',
    '''        let color_config = PreviewColorConfig::from_project(&self.project);''',
    "render config helper",
)

# Queue polling + UI called in frame.
main = replace_once(
    main,
    '''        self.poll_job();
        self.poll_render(ui.ctx());
''',
    '''        self.poll_job();
        self.poll_export_queue();
        self.poll_render(ui.ctx());
''',
    "poll export queue",
)
main = replace_once(
    main,
    '''        self.ui_previous_shades_window(ui.ctx());
        self.ui_export_all_window(ui.ctx());
        self.ui_recovery_window(ui.ctx());
''',
    '''        self.ui_previous_shades_window(ui.ctx());
        self.ui_export_all_window(ui.ctx());
        self.ui_export_queue_window(ui.ctx());
        self.ui_tiff_inspector_window(ui.ctx());
        self.ui_recovery_window(ui.ctx());
''',
    "queue/inspect ui calls",
)

# Toolbar: add File > Inspect TIFF and Queue access.
toolbar_anchor = '''            ui.horizontal_wrapped(|ui| {
                let enabled = self.job.is_none();
'''
toolbar_repl = '''            ui.horizontal_wrapped(|ui| {
                let enabled = self.job.is_none();
                ui.menu_button("File", |ui| {
                    if ui.button("Inspect TIFF...").clicked() {
                        ui.close();
                        self.inspect_tiff_dialog();
                    }
                });
'''
main = replace_once(main, toolbar_anchor, toolbar_repl, "file inspect menu")
main = replace_once(
    main,
    '''                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Export all")).clicked() { self.export_all_dialog(); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Validate face"))''',
    '''                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Export all")).clicked() { self.export_all_dialog(); }
                let queue_label = if self.export_queue.pending_count() > 0 {
                    format!("Export Queue ({})", self.export_queue.pending_count())
                } else {
                    "Export Queue".to_owned()
                };
                if ui.button(queue_label).clicked() { self.show_export_queue = true; }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Validate face"))''',
    "toolbar queue button",
)
# Operation progress gets queue progress between job and render.
main = replace_once(
    main,
    '''        if self.render_busy.is_some() {
            ui.add(
''',
    '''        if let Some((fraction, text)) = self.export_queue.active_summary() {
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width(320.0)
                    .text(text)
                    .animate(false),
            );
            return;
        }
        if self.render_busy.is_some() {
            ui.add(
''',
    "queue compact progress",
)

# ---------------- replace export current with enqueue ----------------
main = regex_once(
    main,
    r'''    fn export_current_dialog\(&mut self\) \{.*?
    \}

    fn validate_current_face_dialog''',
    r'''    fn export_current_dialog(&mut self) {
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let source_name = face
            .path
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "source".to_owned());
        let face_name = self
            .project
            .faces
            .get(self.current_face)
            .map(|face| face.label.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&source_name);
        let snapshot = self
            .project
            .active_snapshot_name()
            .map(str::to_owned)
            .unwrap_or_else(|| "Working".to_owned());
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let date = Local::now().format("%Y-%m-%d").to_string();
        let suggested = export_batch::render_export_filename(
            &self.settings.export_all_template,
            &export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &self.project.name,
                snapshot_code: &snapshot,
                face_number: self.current_face + 1,
                face_name,
                source_name: &source_name,
                date: &date,
            },
        );
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("TIFF image", &["tif", "tiff"])
            .set_file_name(suggested)
            .save_file()
        else {
            return;
        };
        let mark = self.project.active_snapshot_id.map(|snapshot_id| {
            export_queue::ExportQueueMark {
                snapshot_id,
                face_key: face.path.to_string_lossy().into_owned(),
                folder: destination
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf(),
            }
        });
        self.export_queue.enqueue(export_queue::ExportQueueSpec {
            label: format!("Face {} / {}", self.current_face + 1, snapshot),
            source: face.path.clone(),
            destination,
            project: self.project.clone(),
            default_dpi: self.settings.default_dpi,
            force_lzw: self.settings.lzw_compression,
            validate_after_export: self.settings.validate_after_export,
            mark,
        });
        self.show_export_queue = true;
        self.report_info("Export added to queue");
    }

    fn validate_current_face_dialog''',
    "enqueue current export",
)

# ---------------- replace start_export_all with enqueue loop ----------------
main = regex_once(
    main,
    r'''    fn start_export_all\(&mut self\) \{.*?
    \}

    fn ui_export_all_window''',
    r'''    fn start_export_all(&mut self) {
        if self.faces.is_empty() {
            return;
        }
        if self.faces.iter().any(|face| !face.available) {
            self.report_error("Export all requires every Face source TIFF to be available. Relink missing Faces first.");
            return;
        }
        let base_folder = PathBuf::from(self.export_all_folder.trim());
        if self.export_all_folder.trim().is_empty() {
            self.report_error("Choose an Export All folder first.");
            return;
        }
        if let Err(err) = std::fs::create_dir_all(&base_folder) {
            self.report_error(format!(
                "Cannot create Export All folder {}: {err}",
                base_folder.display()
            ));
            return;
        }

        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let project_name = self.project.name.clone();
        let snapshot = self
            .project
            .active_snapshot_name()
            .map(str::to_owned)
            .unwrap_or_else(|| "Working".to_owned());
        let date = Local::now().format("%Y-%m-%d").to_string();
        let mut queued = 0usize;
        let mut skipped = 0usize;
        let mut reserved = BTreeSet::new();
        let mut export_project = self.project.clone();
        export_project.test_code.enabled = self.settings.export_all_test_code;

        for (index, face) in self.faces.iter().enumerate() {
            let source_name = face
                .path
                .file_stem()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("face-{}", index + 1));
            let face_name = self
                .project
                .faces
                .get(index)
                .map(|face| face.label.as_str())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&source_name);
            let context = export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &project_name,
                snapshot_code: &snapshot,
                face_number: index + 1,
                face_name,
                source_name: &source_name,
                date: &date,
            };
            let folder = export_batch::render_export_folder(
                &base_folder,
                &self.settings.export_folder_template,
                &context,
            );
            if let Err(err) = std::fs::create_dir_all(&folder) {
                self.report_error(format!(
                    "Cannot create export folder {}: {err}",
                    folder.display()
                ));
                return;
            }
            let filename =
                export_batch::render_export_filename(&self.settings.export_all_template, &context);
            let destination = match export_batch::resolve_destination_reserved(
                &folder,
                &filename,
                self.settings.export_all_conflict_policy,
                &mut reserved,
            ) {
                export_batch::DestinationDecision::Write(path) => path,
                export_batch::DestinationDecision::Skip(_) => {
                    skipped += 1;
                    continue;
                }
            };
            let mark = self.project.active_snapshot_id.map(|snapshot_id| {
                export_queue::ExportQueueMark {
                    snapshot_id,
                    face_key: face.path.to_string_lossy().into_owned(),
                    folder: folder.clone(),
                }
            });
            self.export_queue.enqueue(export_queue::ExportQueueSpec {
                label: format!("Face {} / {}", index + 1, snapshot),
                source: face.path.clone(),
                destination,
                project: export_project.clone(),
                default_dpi: self.settings.default_dpi,
                force_lzw: self.settings.lzw_compression,
                validate_after_export: self.settings.validate_after_export,
                mark,
            });
            queued += 1;
        }

        self.show_export_all = false;
        self.show_export_queue = true;
        self.settings.sanitize();
        if let Err(err) = self.settings.save() {
            self.log.error(&err);
        }
        self.report_info(if skipped > 0 {
            format!("Queued {queued} export(s) · skipped {skipped} existing file(s)")
        } else {
            format!("Queued {queued} export(s)")
        });
    }

    fn ui_export_all_window''',
    "enqueue export all",
)

# Export All preview contexts: add source/date and folder template UI.
main = replace_once(
    main,
    '''        let snapshot_code = self.project.effective_test_code_text();
        let face_name = self
''',
    '''        let snapshot_code = self
            .project
            .active_snapshot_name()
            .map(str::to_owned)
            .unwrap_or_else(|| "Working".to_owned());
        let date = Local::now().format("%Y-%m-%d").to_string();
        let face_name = self
''',
    "export all preview snapshot/date",
)
main = replace_once(
    main,
    '''        let preview_name = export_batch::render_export_filename(
            &self.settings.export_all_template,
            &export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &self.project.name,
                snapshot_code: &snapshot_code,
                face_number: 1,
                face_name,
            },
        );
''',
    '''        let source_name = self
            .faces
            .first()
            .and_then(|face| face.path.file_stem())
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "source".to_owned());
        let preview_context = export_batch::ExportNameContext {
            shade_name: shade_name.as_deref(),
            project_name: &self.project.name,
            snapshot_code: &snapshot_code,
            face_number: 1,
            face_name,
            source_name: &source_name,
            date: &date,
        };
        let preview_name = export_batch::render_export_filename(
            &self.settings.export_all_template,
            &preview_context,
        );
        let preview_folder = export_batch::render_export_folder(
            &folder,
            &self.settings.export_folder_template,
            &preview_context,
        );
''',
    "export all preview context",
)
main = replace_once(
    main,
    '''                ui.small("Tokens: {shade-name|project-name}, {shade-name}, {project-name}, {snapshot-code}, {face-number}, {face-name}");
                ui.small("Windows-reserved characters such as * are converted to '-' in the generated filename.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Preview:");
                    ui.monospace(&preview_name);
                });

                ui.add_space(8.0);
''',
    '''                ui.small("Tokens: {project}, {face}, {snapshot}, {date}, {source}; legacy tokens remain supported.");
                ui.small("Windows-reserved characters such as * are converted to '-' in the generated filename.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Preview:");
                    ui.monospace(&preview_name);
                });
                ui.add_space(8.0);
                ui.strong("Folder template");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.settings.export_folder_template)
                            .hint_text("{project}/{date}/{snapshot}/")
                            .desired_width(455.0),
                    )
                    .changed();
                ui.small("Leave empty to export directly into the selected base folder.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Folder preview:");
                    ui.monospace(preview_folder.display().to_string());
                });

                ui.add_space(8.0);
''',
    "folder template ui",
)

# ---------------- Snapshot exports now enqueue ----------------
main = regex_once(
    main,
    r'''    fn export_snapshot_dialog\(&mut self, snapshot_id: u64\) \{.*?
    \}

    fn export_snapshot_group_dialog''',
    r'''    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let Some(snapshot) = self
            .project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == snapshot_id)
            .cloned()
        else {
            return;
        };
        let source_name = face
            .path
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "source".to_owned());
        let face_name = self
            .project
            .faces
            .get(self.current_face)
            .map(|face| face.label.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&source_name);
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let date = Local::now().format("%Y-%m-%d").to_string();
        let suggested = export_batch::render_export_filename(
            &self.settings.export_all_template,
            &export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &self.project.name,
                snapshot_code: &snapshot.name,
                face_number: self.current_face + 1,
                face_name,
                source_name: &source_name,
                date: &date,
            },
        );
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("TIFF image", &["tif", "tiff"])
            .set_file_name(suggested)
            .save_file()
        else {
            return;
        };

        let folder = destination
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut project = self.project.clone();
        project.adjustments = snapshot.adjustments.clone();
        project.active_snapshot_id = Some(snapshot.id);
        self.export_queue.enqueue(export_queue::ExportQueueSpec {
            label: format!("Face {} / {}", self.current_face + 1, snapshot.name),
            source: face.path.clone(),
            destination,
            project,
            default_dpi: self.settings.default_dpi,
            force_lzw: self.settings.lzw_compression,
            validate_after_export: self.settings.validate_after_export,
            mark: Some(export_queue::ExportQueueMark {
                snapshot_id: snapshot.id,
                face_key: face.path.to_string_lossy().into_owned(),
                folder,
            }),
        });
        self.show_export_queue = true;
        self.report_info("Snapshot export added to queue");
    }

    fn export_snapshot_group_dialog''',
    "enqueue snapshot export",
)

main = regex_once(
    main,
    r'''    fn export_snapshot_group_dialog\(&mut self, snapshot_ids: Vec<u64>, label: String\) \{.*?
    \}

    fn ensure_project_palette_for_model''',
    r'''    fn export_snapshot_group_dialog(&mut self, snapshot_ids: Vec<u64>, label: String) {
        if snapshot_ids.is_empty() {
            return;
        }
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let Some(base_folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let source = face.path.clone();
        let source_name = source
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "source".to_owned());
        let face_name = self
            .project
            .faces
            .get(self.current_face)
            .map(|face| face.label.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&source_name)
            .to_owned();
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let date = Local::now().format("%Y-%m-%d").to_string();
        let snapshots = snapshot_ids
            .into_iter()
            .filter_map(|id| {
                self.project
                    .snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .cloned()
            })
            .collect::<Vec<_>>();
        if snapshots.is_empty() {
            return;
        }

        let mut reserved = BTreeSet::new();
        let mut queued = 0usize;
        for snapshot in snapshots {
            let context = export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &self.project.name,
                snapshot_code: &snapshot.name,
                face_number: self.current_face + 1,
                face_name: &face_name,
                source_name: &source_name,
                date: &date,
            };
            let folder = export_batch::render_export_folder(
                &base_folder,
                &self.settings.export_folder_template,
                &context,
            );
            if let Err(err) = std::fs::create_dir_all(&folder) {
                self.report_error(format!("Cannot create export folder {}: {err}", folder.display()));
                return;
            }
            let filename =
                export_batch::render_export_filename(&self.settings.export_all_template, &context);
            let destination = match export_batch::resolve_destination_reserved(
                &folder,
                &filename,
                self.settings.export_all_conflict_policy,
                &mut reserved,
            ) {
                export_batch::DestinationDecision::Write(path) => path,
                export_batch::DestinationDecision::Skip(_) => continue,
            };
            let mut project = self.project.clone();
            project.adjustments = snapshot.adjustments.clone();
            project.active_snapshot_id = Some(snapshot.id);
            self.export_queue.enqueue(export_queue::ExportQueueSpec {
                label: format!("Face {} / {}", self.current_face + 1, snapshot.name),
                source: source.clone(),
                destination,
                project,
                default_dpi: self.settings.default_dpi,
                force_lzw: self.settings.lzw_compression,
                validate_after_export: self.settings.validate_after_export,
                mark: Some(export_queue::ExportQueueMark {
                    snapshot_id: snapshot.id,
                    face_key: source.to_string_lossy().into_owned(),
                    folder,
                }),
            });
            queued += 1;
        }
        self.show_export_queue = true;
        self.report_info(format!("Queued {queued} snapshot export(s) ({label})"));
    }

    fn ensure_project_palette_for_model''',
    "enqueue snapshot group",
)

# ---------------- queue poll/window + TIFF inspect methods inserted before poll_job ----------------
insert_anchor = '''    fn poll_job(&mut self) {
'''
new_methods = r'''    fn poll_export_queue(&mut self) {
        let completions = self.export_queue.poll();
        for completion in completions {
            match completion.result {
                Ok(message) => {
                    self.log.info(&format!("Export queue #{}: {message}", completion.id));
                    self.status_message = "Export queue item complete".to_owned();
                    if let Some(mark) = completion.mark {
                        self.project.record_snapshot_export(
                            mark.snapshot_id,
                            mark.face_key,
                            mark.folder.to_string_lossy().into_owned(),
                            unix_ms_now(),
                        );
                        self.project_dirty = true;
                    }
                }
                Err(err) => {
                    self.log.error(&format!("Export queue #{} failed: {err}", completion.id));
                    self.toast = Some(ErrorToast {
                        message: format!("Export failed: {err}"),
                        created: Instant::now(),
                    });
                    self.status_message = "Export queue item failed".to_owned();
                }
            }
        }
    }

    fn ui_export_queue_window(&mut self, ctx: &egui::Context) {
        if !self.show_export_queue {
            return;
        }
        let mut open = self.show_export_queue;
        let rows = self
            .export_queue
            .items()
            .iter()
            .map(|item| {
                (
                    item.id,
                    item.label.clone(),
                    item.destination.clone(),
                    item.status,
                    item.progress,
                    item.detail.clone(),
                    item.error.clone(),
                )
            })
            .collect::<Vec<_>>();
        let mut cancel = None;
        let mut retry = None;
        let mut clear_finished = false;
        let mut cancel_waiting = false;

        egui::Window::new("Export Queue")
            .open(&mut open)
            .resizable(true)
            .default_size([860.0, 520.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Export Queue");
                    ui.separator();
                    ui.label(format!("{} pending", self.export_queue.pending_count()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clear_finished = ui.button("Clear finished").clicked();
                        cancel_waiting = ui.button("Cancel waiting").clicked();
                    });
                });
                ui.small("Waiting items cancel immediately. Cancelling a Processing item requests a safe stop after the current atomic TIFF export completes; the next queued item will not start.");
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("export-queue-list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (id, label, destination, status, progress, detail, error) in &rows {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.strong(label);
                                    ui.label(status.label());
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| match status {
                                            export_queue::ExportQueueStatus::Waiting
                                            | export_queue::ExportQueueStatus::Processing => {
                                                if ui.small_button("Cancel").clicked() {
                                                    cancel = Some(*id);
                                                }
                                            }
                                            export_queue::ExportQueueStatus::Failed
                                            | export_queue::ExportQueueStatus::Cancelled => {
                                                if ui.small_button("Retry").clicked() {
                                                    retry = Some(*id);
                                                }
                                            }
                                            export_queue::ExportQueueStatus::Done => {}
                                        },
                                    );
                                });
                                ui.add(
                                    egui::ProgressBar::new(*progress)
                                        .desired_width(f32::INFINITY)
                                        .text(if detail.trim().is_empty() {
                                            status.label().to_owned()
                                        } else {
                                            detail.clone()
                                        }),
                                );
                                ui.small(destination.display().to_string());
                                if let Some(error) = error {
                                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                                }
                            });
                            ui.add_space(4.0);
                        }
                    });
                if rows.is_empty() {
                    ui.centered_and_justified(|ui| ui.label("Queue is empty."));
                }
            });

        self.show_export_queue = open;
        if let Some(id) = cancel {
            self.export_queue.cancel(id);
        }
        if let Some(id) = retry {
            self.export_queue.retry(id);
        }
        if cancel_waiting {
            self.export_queue.cancel_all_waiting();
        }
        if clear_finished {
            self.export_queue.clear_finished();
        }
    }

    fn inspect_tiff_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter("TIFF image", &["tif", "tiff"]);
        if let Some(parent) = self
            .faces
            .get(self.current_face)
            .and_then(|face| face.path.parent())
        {
            dialog = dialog.set_directory(parent);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        match tiff_inspect::inspect(&path, self.settings.default_dpi) {
            Ok(inspection) => {
                self.tiff_inspection_path = Some(inspection.path);
                self.tiff_inspection_report = inspection.report;
                self.show_tiff_inspector = true;
            }
            Err(err) => self.report_error(format!("TIFF inspection failed: {err}")),
        }
    }

    fn ui_tiff_inspector_window(&mut self, ctx: &egui::Context) {
        if !self.show_tiff_inspector {
            return;
        }
        let mut open = self.show_tiff_inspector;
        let mut copy = false;
        egui::Window::new("Inspect TIFF")
            .open(&mut open)
            .resizable(true)
            .default_size([820.0, 680.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(path) = self.tiff_inspection_path.as_ref() {
                        ui.strong(path.display().to_string());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        copy = ui.button("Copy report").clicked();
                    });
                });
                ui.separator();
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.tiff_inspection_report)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(34)
                                .interactive(false),
                        );
                    });
            });
        self.show_tiff_inspector = open;
        if copy {
            ctx.copy_text(self.tiff_inspection_report.clone());
            self.report_info("TIFF inspection report copied");
        }
    }

'''
main = replace_once(main, insert_anchor, new_methods + insert_anchor, "queue/inspect methods")

# Close guard for active queue.
main = replace_once(
    main,
    '''        if self.allow_close_once {
            self.allow_close_once = false;
            return;
        }
        if self.project_dirty {
''',
    '''        if self.allow_close_once {
            self.allow_close_once = false;
            return;
        }
        if self.export_queue.has_pending() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.show_export_queue = true;
            self.report_info("Export Queue is still active. Cancel waiting items or let the current atomic export finish before closing.");
            return;
        }
        if self.project_dirty {
''',
    "close queue guard",
)

# ---------------- Color Management UI: add proof state and controls ----------------
main = replace_once(
    main,
    '''        let mut bpc = self.project.preview_color.black_point_compensation;
        let mut show_incompatible = self.icc_show_incompatible;
        let mut requested_profile: Option<Option<PathBuf>> = None;
        let mut browse_requested = false;
        let mut refresh_requested = false;
''',
    '''        let mut bpc = self.project.preview_color.black_point_compensation;
        let mut soft_proof_enabled = self.project.preview_color.soft_proof_enabled;
        let mut proofing_intent = self.project.preview_color.proofing_intent;
        let mut proof_selected = self
            .proof_profile_selected
            .clone()
            .or_else(|| self.project.preview_color.proof_profile_path.clone());
        let mut show_incompatible = self.icc_show_incompatible;
        let mut requested_profile: Option<Option<PathBuf>> = None;
        let mut requested_proof_profile: Option<Option<PathBuf>> = None;
        let mut browse_requested = false;
        let mut browse_proof_requested = false;
        let mut refresh_requested = false;
''',
    "proof ui locals",
)
main = replace_once(
    main,
    '''                ui.small("Black point compensation is optional and is most useful with relative-colorimetric transforms. The preview destination remains sRGB; no monitor or printer/RIP proof profile is applied here.");

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Search");
''',
    '''                ui.small("Black point compensation is optional and is most useful with relative-colorimetric transforms. The display destination remains sRGB.");

                ui.separator();
                ui.heading("Printer / RIP Soft Proof");
                ui.checkbox(&mut soft_proof_enabled, "Enable printer/RIP soft proof");
                let proof_profiles = profiles
                    .iter()
                    .filter(|profile| profile.is_output_profile())
                    .collect::<Vec<_>>();
                let proof_label = proof_selected
                    .as_deref()
                    .and_then(|path| {
                        profiles
                            .iter()
                            .find(|profile| profile.path.to_string_lossy() == path)
                            .map(|profile| profile.description.clone())
                    })
                    .unwrap_or_else(|| "Select output/printer ICC".to_owned());
                egui::ComboBox::from_label("Proof device profile")
                    .selected_text(proof_label)
                    .width(390.0)
                    .show_ui(ui, |ui| {
                        for profile in proof_profiles {
                            let path = profile.path.to_string_lossy().into_owned();
                            if ui
                                .selectable_label(
                                    proof_selected.as_deref() == Some(path.as_str()),
                                    format!(
                                        "{} · {} · {}",
                                        profile.description,
                                        profile.color_space_label(),
                                        profile.filename()
                                    ),
                                )
                                .on_hover_text(profile.path.display().to_string())
                                .clicked()
                            {
                                proof_selected = Some(path.clone());
                                requested_proof_profile =
                                    Some(Some(PathBuf::from(path)));
                            }
                        }
                    });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Browse printer/RIP ICC...").clicked() {
                        browse_proof_requested = true;
                    }
                    if ui.button("Clear proof profile").clicked() {
                        requested_proof_profile = Some(None);
                        proof_selected = None;
                    }
                });
                egui::ComboBox::from_label("Proof rendering intent")
                    .selected_text(proofing_intent.label())
                    .show_ui(ui, |ui| {
                        for value in [
                            PreviewRenderingIntent::Perceptual,
                            PreviewRenderingIntent::RelativeColorimetric,
                            PreviewRenderingIntent::Saturation,
                            PreviewRenderingIntent::AbsoluteColorimetric,
                        ] {
                            ui.selectable_value(&mut proofing_intent, value, value.label());
                        }
                    });
                ui.small("Soft proof uses the selected output/printer ICC as LittleCMS proofing device between the document profile and the sRGB display preview. It never writes the proof ICC into TIFF/export data.");

                ui.separator();
                ui.heading("Document / source profile assignment");
                ui.horizontal(|ui| {
                    ui.label("Search");
''',
    "proof controls",
)
main = replace_once(
    main,
    '''        self.icc_profile_selected = selected;
        self.icc_show_incompatible = show_incompatible;
''',
    '''        self.icc_profile_selected = selected;
        self.proof_profile_selected = proof_selected;
        self.icc_show_incompatible = show_incompatible;
''',
    "proof selection persist ui",
)
main = replace_once(
    main,
    '''        if browse_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_profile = Some(Some(path));
            }
        }

        let mut changed = false;
''',
    '''        if browse_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_profile = Some(Some(path));
            }
        }
        if browse_proof_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_proof_profile = Some(Some(path));
            }
        }

        let mut changed = false;
''',
    "proof browse",
)
main = replace_once(
    main,
    '''        if self.project.preview_color.black_point_compensation != bpc {
            self.project.preview_color.black_point_compensation = bpc;
            changed = true;
        }

        if let Some(requested) = requested_profile {
''',
    '''        if self.project.preview_color.black_point_compensation != bpc {
            self.project.preview_color.black_point_compensation = bpc;
            changed = true;
        }
        if self.project.preview_color.soft_proof_enabled != soft_proof_enabled {
            self.project.preview_color.soft_proof_enabled = soft_proof_enabled;
            changed = true;
        }
        if self.project.preview_color.proofing_intent != proofing_intent {
            self.project.preview_color.proofing_intent = proofing_intent;
            changed = true;
        }

        if let Some(requested) = requested_profile {
''',
    "proof setting changes",
)
main = replace_once(
    main,
    '''        if changed {
            self.project_dirty = true;
            self.invalidate_display_previews();
        }
    }

    fn ui_settings_window''',
    '''        if let Some(requested) = requested_proof_profile {
            match requested {
                None => {
                    if self.project.preview_color.proof_profile_path.is_some() {
                        self.project.preview_color.proof_profile_path = None;
                        self.proof_profile_selected = None;
                        changed = true;
                    }
                }
                Some(path) => match color_management::inspect_profile(&path) {
                    Ok(profile) if profile.is_output_profile() => {
                        let path_text = path.to_string_lossy().into_owned();
                        if self.project.preview_color.proof_profile_path.as_deref()
                            != Some(path_text.as_str())
                        {
                            self.project.preview_color.proof_profile_path =
                                Some(path_text.clone());
                            self.proof_profile_selected = Some(path_text);
                            changed = true;
                        }
                    }
                    Ok(profile) => self.report_error(format!(
                        "Cannot use '{}' for printer/RIP soft proof: ICC class is {}, not Output / printer.",
                        profile.description,
                        profile.device_class_label(),
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

    fn ui_settings_window''',
    "proof apply request",
)

# When clicking profile badge, preselect proof too.
main = replace_once(
    main,
    '''                self.icc_profile_selected =
                    self.project.preview_color.assigned_profile_path.clone();
''',
    '''                self.icc_profile_selected =
                    self.project.preview_color.assigned_profile_path.clone();
                self.proof_profile_selected =
                    self.project.preview_color.proof_profile_path.clone();
''',
    "profile badge proof selection",
)

# ---------------- docs/release notes ----------------
readme = read("README.md")
readme = replace_once(
    readme,
    "- ICC-aware preview with embedded-profile support and non-destructive **preview profile assignment**.\n",
    "- ICC-aware preview with embedded-profile support and non-destructive **preview profile assignment**.\n- True printer/RIP **Soft Proof** using an output-device ICC proofing transform; proof settings remain preview-only.\n",
    "readme soft proof feature",
)
readme = replace_once(
    readme,
    "- Assigned preview profile is saved in `.shade`; TIFF ICC bytes and source/export samples are never changed by it.\n",
    "- Assigned preview/proof profiles are saved in `.shade`; TIFF ICC bytes and source/export samples are never changed by them.\n- Color-managed project thumbnails use the same assigned profile and printer/RIP soft-proof transform as the viewport.\n- Export Queue provides Waiting / Processing / Done / Failed states with safe cancel/retry controls.\n- Export filename/folder templates support `{project}`, `{face}`, `{snapshot}`, `{date}` and `{source}`.\n- `File > Inspect TIFF` reports production transport metadata and can copy a diagnostic report.\n",
    "readme production features",
)
old = "This is profile assignment / color-managed preview, not a printer/RIP proof simulation. A real proofing transform would require a separate proof-device profile; that is intentionally not part of the current scope."
if old in readme:
    readme = readme.replace(
        old,
        "When a printer/RIP output ICC is selected and Soft Proof is enabled, Shade Editor uses a LittleCMS proofing transform before the sRGB display conversion. This is still display-only: no proof profile is embedded into or applied to exported TIFF samples.",
    )
write("README.md", readme)

arch = read("docs/ARCHITECTURE.md")
arch = arch.replace(
    "Assigned ICC is an **input/source-profile override for preview**. It is not a proofing profile. A true printer/RIP soft proof would require LittleCMS proofing transforms and a separate proof-device profile and is currently out of scope.",
    "Assigned ICC is an **input/source-profile override for preview**. Printer/RIP Soft Proof is a separate project-owned output-device ICC using a LittleCMS proofing transform. Both remain display-only and are forbidden inputs to `export.rs`.",
)
if "## Export queue" not in arch:
    arch += r'''

## Export queue

`export_queue.rs` owns queued TIFF exports independently from UI state. Every queue item captures an immutable clone of the project/snapshot recipe at enqueue time. The existing export backend still writes a temporary TIFF and atomically replaces the destination only after successful completion. Waiting items cancel immediately; a Processing cancel is a safe stop-after-current request so the atomic export is never interrupted into a partial destination.

## Production TIFF inspector

`tiff_inspect.rs` is read-only. It combines decoded TIFF metadata with raw transport tags for diagnostics and never loads an inspected file into the editing project.
'''
write("docs/ARCHITECTURE.md", arch)

roadmap = read("docs/ROADMAP.md")
roadmap = roadmap.replace(
    "Intentionally deferred: monitor-profile output transforms and printer/RIP proof-device transforms. A true proofing transform should only be added when a real production proof profile/workflow is available to validate it; do not label source-profile assignment as printer soft proof.",
    "Implemented: printer/RIP proof-device transforms using a selected Output-class ICC. Still deferred: monitor-profile output transforms and production-specific gamut-alarm UX. Validate proof appearance against the real RIP/printer workflow before treating the screen as a contractual press match.",
)
write("docs/ROADMAP.md", roadmap)

notes = read("RELEASE_NOTES.md")
unreleased = r'''# Shade Editor 0.17.0

- Add true printer/RIP Soft Proof with a separate Output-class proof ICC, proof rendering intent and project persistence; the transform remains preview-only and never enters TIFF export.
- Make project thumbnails use the exact same color-managed / soft-proof preview pipeline as the viewport.
- Add non-blocking Export Queue with Waiting / Processing / Done / Failed / Cancelled states, safe cancel semantics and Retry.
- Extend export naming with `{project}`, `{face}`, `{snapshot}`, `{date}`, `{source}` and nested Folder Templates while keeping legacy tokens compatible.
- Add `File > Inspect TIFF` read-only production diagnostics with Copy report: TIFF/BigTIFF, dimensions, bits, photometric, planar configuration, compression, predictor, ExtraSamples, channel/Spot order, Photoshop resources, ICC, DPI and estimated uncompressed size.
- Change History labels to Photoshop-style `Tool · Channel` naming.
- Re-validate the existing Safe Project Save / Recovery path: `.tmp` + flush/sync + atomic replace, `.bak` backup, rotating recovery version/checksum and corrupt-state rejection.

'''
if not notes.startswith("# Shade Editor 0.17.0"):
    notes = unreleased + notes
write("RELEASE_NOTES.md", notes)

print("Production upgrade migration applied.")