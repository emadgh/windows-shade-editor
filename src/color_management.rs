use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use lcms2::{
    ColorSpaceSignature, Flags, InfoType, Intent, Locale, PixelFormat, Profile,
    ProfileClassSignature, Transform,
};
use sha2::{Digest, Sha256};

#[cfg(windows)]
use windows_sys::Win32::UI::ColorSystem::{ENUM_TYPE_VERSION, ENUMTYPEW, EnumColorProfilesW};

use crate::model::{IccProfileIdentity, PreviewRenderingIntent, ShadeProject};
use crate::runtime_preview::{RuntimeColorModel, RuntimePreviewSource};
use crate::settings::AppSettings;
use crate::tiff_io::{ColorModel, TiffMetadata};

#[derive(Clone, Debug)]
pub struct InstalledIccProfile {
    pub path: PathBuf,
    pub description: String,
    color_space: ColorSpaceSignature,
    device_class: ProfileClassSignature,
    identity: IccProfileIdentity,
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

    pub fn is_display_profile(&self) -> bool {
        self.device_class == ProfileClassSignature::DisplayClass
            && self.color_space == ColorSpaceSignature::RgbData
    }

    pub fn identity(&self) -> &IccProfileIdentity {
        &self.identity
    }

    pub fn matches_identity(&self, identity: &IccProfileIdentity) -> bool {
        !identity.sha256.is_empty() && self.identity.sha256.eq_ignore_ascii_case(&identity.sha256)
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
    /// Legacy/manual source-intent value kept as a safe fallback for malformed
    /// or non-ICC extended intent values. A valid source ICC header wins.
    pub intent: PreviewRenderingIntent,
    pub black_point_compensation: bool,
    pub assigned_profile_path: Option<PathBuf>,
    pub assigned_profile_identity: Option<IccProfileIdentity>,
    pub soft_proof_enabled: bool,
    pub proof_profile_path: Option<PathBuf>,
    pub proof_profile_identity: Option<IccProfileIdentity>,
    pub proofing_intent: PreviewRenderingIntent,
    pub monitor_profile_path: Option<PathBuf>,
    pub monitor_profile_identity: Option<IccProfileIdentity>,
    pub gamut_warning: bool,
}

impl PreviewColorConfig {
    /// Portable/project thumbnail configuration. Monitor ICC and gamut alarm are
    /// workstation UI concerns and are intentionally excluded from stored thumbnails.
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
            assigned_profile_identity: project.preview_color.assigned_profile_identity.clone(),
            soft_proof_enabled: project.preview_color.soft_proof_enabled,
            proof_profile_path: project
                .preview_color
                .proof_profile_path
                .as_ref()
                .map(PathBuf::from),
            proof_profile_identity: project.preview_color.proof_profile_identity.clone(),
            proofing_intent: project.preview_color.proofing_intent,
            monitor_profile_path: None,
            monitor_profile_identity: None,
            gamut_warning: false,
        }
    }

    pub fn for_viewport(project: &ShadeProject, settings: &AppSettings) -> Self {
        let mut config = Self::from_project(project);
        config.monitor_profile_path = settings.monitor_profile_path.as_ref().map(PathBuf::from);
        config.monitor_profile_identity = settings.monitor_profile_identity.clone();
        config.gamut_warning = settings.gamut_warning;
        config
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
        monitor_description: Option<String>,
        gamut_warning: bool,
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
                "Project color preview is disabled. Source data and metadata are unchanged.".to_owned()
            }
            Self::NoEmbeddedProfile => "This source has no embedded ICC profile and no preview profile is assigned. Shade Editor is using its unmanaged display fallback.".to_owned(),
            Self::Applied {
                description,
                intent,
                source,
                black_point_compensation,
                proof_description,
                proofing_intent,
                monitor_description,
                gamut_warning,
            } => {
                let source = match source {
                    PreviewProfileSource::Embedded => "embedded source profile".to_owned(),
                    PreviewProfileSource::Assigned(path) => {
                        format!("assigned preview profile {}", path.display())
                    }
                };
                let bpc = if *black_point_compensation {
                    " · black point compensation on"
                } else {
                    ""
                };
                let monitor = monitor_description
                    .as_deref()
                    .map(|value| format!("monitor '{}'", value))
                    .unwrap_or_else(|| "sRGB display fallback".to_owned());
                let gamut = if *gamut_warning {
                    " · gamut warning on"
                } else {
                    ""
                };
                if let (Some(proof), Some(proof_intent)) =
                    (proof_description.as_ref(), proofing_intent.as_ref())
                {
                    format!(
                        "{} ({source}) → printer/RIP soft proof '{}' → {monitor} · source {} intent (automatic from source ICC header) · proof {} intent{bpc}{gamut}. Preview-only; source samples and metadata are unchanged.",
                        description,
                        proof,
                        intent.label(),
                        proof_intent.label(),
                    )
                } else {
                    format!(
                        "{} ({source}) → {monitor} · {} intent (automatic from source ICC header){bpc}. Preview-only; source samples and metadata are unchanged.",
                        description,
                        intent.label(),
                    )
                }
            }
            Self::Fallback { reason, .. } => format!(
                "The requested color-management transform could not be used ({reason}). Shade Editor fell back to the unmanaged display conversion; Source data is unchanged."
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

    pub fn source_rendering_intent(&self) -> Option<PreviewRenderingIntent> {
        match self {
            Self::Applied { intent, .. } => Some(*intent),
            _ => None,
        }
    }
}

enum BaseTransform {
    Rgb(Transform<[u16; 3], [u8; 3]>),
    Cmyk(Transform<[u16; 4], [u8; 3]>),
    Gray(Transform<[u16; 1], [u8; 3]>),
}

/// Preview-only ICC transform. Assigned source, proof and monitor profiles are
/// never written to TIFF or consumed by production export.
pub struct PreviewColorTransform {
    transform: Option<BaseTransform>,
    status: PreviewColorStatus,
}

impl PreviewColorTransform {
    pub fn new(metadata: &TiffMetadata, config: PreviewColorConfig) -> Self {
        Self::new_for_parts(
            metadata.color_model.into(),
            metadata.icc_profile.as_deref(),
            config,
        )
    }

    pub fn new_for_runtime_preview<P: RuntimePreviewSource + ?Sized>(
        preview: &P,
        config: PreviewColorConfig,
    ) -> Self {
        Self::new_for_parts(preview.color_model(), preview.embedded_icc(), config)
    }

    fn new_for_parts(
        model: RuntimeColorModel,
        embedded_icc: Option<&[u8]>,
        config: PreviewColorConfig,
    ) -> Self {
        if !config.enabled {
            return Self {
                transform: None,
                status: PreviewColorStatus::Disabled,
            };
        }

        let expected = match expected_runtime_color_space(model) {
            Some(value) => value,
            None => {
                return Self::fallback(
                    "unsupported source base color model".to_owned(),
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
                if let Err(err) = verify_profile_identity(
                    path,
                    config.assigned_profile_identity.as_ref(),
                    "assigned source",
                ) {
                    return Self::fallback(err, requested_label);
                }
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
                let Some(icc) = embedded_icc else {
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
                    "profile color space {} does not match source {}",
                    color_space_label(actual),
                    model.title(),
                ),
                requested_label,
            );
        }

        // The ICC header carries the preferred source rendering intent. This is
        // especially meaningful for embedded source profiles and matches how a
        // color-managed host selects the source transform without making the
        // operator guess between Perceptual/Relative/Saturation/Absolute.
        let source_intent = preferred_profile_intent(&source, config.intent);
        let intent = to_lcms_intent(source_intent);
        let description = profile_description(&source);
        let (destination, monitor_description) = if let Some(path) =
            config.monitor_profile_path.as_ref()
        {
            if let Err(err) = verify_profile_identity(
                path,
                config.monitor_profile_identity.as_ref(),
                "monitor/display",
            ) {
                return Self::fallback(err, Some(profile_path_label(path)));
            }
            let monitor = match Profile::new_file(path) {
                Ok(profile) => profile,
                Err(err) => {
                    return Self::fallback(
                        format!("cannot open monitor profile {}: {err}", path.display()),
                        Some(profile_path_label(path)),
                    );
                }
            };
            if monitor.device_class() != ProfileClassSignature::DisplayClass
                || monitor.color_space() != ColorSpaceSignature::RgbData
            {
                return Self::fallback(
                    format!(
                        "monitor profile '{}' must be an RGB Display-class profile, found {} / {}",
                        profile_description(&monitor),
                        profile_class_label(monitor.device_class()),
                        color_space_label(monitor.color_space()),
                    ),
                    Some(profile_path_label(path)),
                );
            }
            let label = profile_description(&monitor);
            (monitor, Some(label))
        } else {
            (Profile::new_srgb(), None)
        };

        let bpc = config.black_point_compensation;

        let proof = if config.soft_proof_enabled {
            let Some(path) = config.proof_profile_path.as_ref() else {
                return Self::fallback(
                    "printer/RIP soft proof is enabled but no proof profile is selected".to_owned(),
                    Some("Soft proof".to_owned()),
                );
            };
            if let Err(err) = verify_profile_identity(
                path,
                config.proof_profile_identity.as_ref(),
                "printer/RIP proof",
            ) {
                return Self::fallback(err, Some(profile_path_label(path)));
            }
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
        let mut proof_flags = Flags::SOFT_PROOFING;
        if bpc {
            proof_flags = proof_flags | Flags::BLACKPOINT_COMPENSATION;
        }
        let gamut_warning = config.gamut_warning && proof.is_some();
        if gamut_warning {
            proof_flags = proof_flags | Flags::GAMUT_CHECK;
        }

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
                    Transform::new(&source, $input, &destination, PixelFormat::RGB_8, intent)
                };
                result.map(BaseTransform::$variant)
            }};
        }

        let transform = match model {
            RuntimeColorModel::Rgb => make_transform!(PixelFormat::RGB_16, Rgb),
            RuntimeColorModel::Cmyk => make_transform!(PixelFormat::CMYK_16, Cmyk),
            RuntimeColorModel::Gray => make_transform!(PixelFormat::GRAY_16, Gray),
            RuntimeColorModel::Other => unreachable!(),
        };

        match transform {
            Ok(transform) => Self {
                transform: Some(transform),
                status: PreviewColorStatus::Applied {
                    description,
                    intent: source_intent,
                    source: source_kind,
                    black_point_compensation: bpc,
                    proof_description: proof.as_ref().map(|(_, label)| label.clone()),
                    proofing_intent: proof.as_ref().map(|_| config.proofing_intent),
                    monitor_description,
                    gamut_warning,
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

pub fn embedded_profile_preferred_intent(
    metadata: &TiffMetadata,
) -> Option<PreviewRenderingIntent> {
    let icc = metadata.icc_profile.as_deref()?;
    let profile = Profile::new_icc(icc).ok()?;
    from_lcms_intent(profile.header_rendering_intent())
}

pub fn file_profile_preferred_intent(path: &Path) -> Result<PreviewRenderingIntent, String> {
    let profile = Profile::new_file(path)
        .map_err(|err| format!("Cannot open ICC profile {}: {err}", path.display()))?;
    from_lcms_intent(profile.header_rendering_intent()).ok_or_else(|| {
        format!(
            "ICC profile {} contains a non-standard rendering intent value",
            path.display()
        )
    })
}

#[derive(Clone)]
struct CachedProfileInspection {
    size: u64,
    modified_ns: Option<u128>,
    profile: InstalledIccProfile,
}

static PROFILE_INSPECTION_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedProfileInspection>>> =
    OnceLock::new();

fn profile_cache() -> &'static Mutex<HashMap<PathBuf, CachedProfileInspection>> {
    PROFILE_INSPECTION_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn inspect_profile(path: &Path) -> Result<InstalledIccProfile, String> {
    let metadata = fs::metadata(path)
        .map_err(|err| format!("Cannot inspect ICC profile {}: {err}", path.display()))?;
    let size = metadata.len();
    let modified_ns = metadata.modified().ok().and_then(|time| {
        time.duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_nanos())
    });
    let cache_key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(cache) = profile_cache().lock() {
        if let Some(cached) = cache.get(&cache_key) {
            if cached.size == size && cached.modified_ns == modified_ns {
                return Ok(cached.profile.clone());
            }
        }
    }

    let profile = Profile::new_file(path)
        .map_err(|err| format!("Cannot open ICC profile {}: {err}", path.display()))?;
    let description = profile_description(&profile);
    let bytes = fs::read(path)
        .map_err(|err| format!("Cannot hash ICC profile {}: {err}", path.display()))?;
    let identity = IccProfileIdentity {
        description: description.clone(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    let inspected = InstalledIccProfile {
        path: path.to_path_buf(),
        description,
        color_space: profile.color_space(),
        device_class: profile.device_class(),
        identity,
    };
    if let Ok(mut cache) = profile_cache().lock() {
        cache.insert(
            cache_key,
            CachedProfileInspection {
                size,
                modified_ns,
                profile: inspected.clone(),
            },
        );
    }
    Ok(inspected)
}

pub fn profile_identity(path: &Path) -> Result<IccProfileIdentity, String> {
    inspect_profile(path).map(|profile| profile.identity)
}

/// Inspect an embedded ICC for production source interpretation.
///
/// Unlike preview fallback behavior, production conversion requires a valid
/// profile whose declared color space matches the source samples and whose
/// bytes can be captured by a stable identity hash.
pub fn production_embedded_profile_identity(
    metadata: &TiffMetadata,
) -> Result<Option<IccProfileIdentity>, String> {
    let Some(bytes) = metadata.icc_profile.as_deref() else {
        return Ok(None);
    };
    if bytes.is_empty() {
        return Err("Embedded production Source ICC payload is empty.".to_owned());
    }
    let profile = Profile::new_icc(bytes)
        .map_err(|err| format!("Cannot open embedded production Source ICC: {err}"))?;
    let Some(expected) = expected_color_space(metadata.color_model) else {
        return Err(format!(
            "{} source data is not supported by production Source ICC assignment.",
            metadata.color_model.title()
        ));
    };
    if profile.color_space() != expected {
        return Err(format!(
            "Embedded Source ICC color space {} does not match source {}.",
            color_space_label(profile.color_space()),
            metadata.color_model.title()
        ));
    }
    Ok(Some(IccProfileIdentity {
        description: profile_description(&profile),
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }))
}

/// Reopen and verify an explicitly assigned production Source ICC.
///
/// The identity check detects replacement at the stored path. Assignment is an
/// interpretation override only; this function never transforms source pixels.
pub fn inspect_production_source_profile(
    path: &Path,
    expected_identity: &IccProfileIdentity,
    source_model: ColorModel,
) -> Result<InstalledIccProfile, String> {
    let profile = inspect_profile(path)?;
    if !profile.compatible_with(source_model) {
        return Err(format!(
            "Assigned production Source ICC '{}' declares {} but the source Face is {}.",
            profile.description,
            profile.color_space_label(),
            source_model.title()
        ));
    }
    if expected_identity.sha256.trim().is_empty() {
        return Err(
            "Assigned production Source ICC has no stored SHA-256 identity. Reassign the profile."
                .to_owned(),
        );
    }
    if !profile.matches_identity(expected_identity) {
        return Err(format!(
            "Assigned production Source ICC at {} no longer matches stored profile '{}'. Reassign or relink the profile before conversion.",
            path.display(),
            expected_identity.description
        ));
    }
    Ok(profile)
}

fn verify_profile_identity(
    path: &Path,
    expected: Option<&IccProfileIdentity>,
    role: &str,
) -> Result<(), String> {
    let Some(expected) = expected.filter(|identity| !identity.sha256.trim().is_empty()) else {
        return Ok(());
    };
    let actual = profile_identity(path)?;
    if !actual.sha256.eq_ignore_ascii_case(&expected.sha256) {
        return Err(format!(
            "{role} ICC at {} no longer matches the profile stored with this configuration (expected '{}'). Relink the profile before previewing.",
            path.display(),
            expected.description
        ));
    }
    Ok(())
}

pub fn resolve_external_profile_path(
    stored_path: Option<&str>,
    identity: Option<&IccProfileIdentity>,
    profiles: &[InstalledIccProfile],
) -> Option<PathBuf> {
    if let Some(path) = stored_path.map(PathBuf::from) {
        if path.is_file() {
            if identity.is_none()
                || inspect_profile(&path)
                    .ok()
                    .zip(identity)
                    .is_some_and(|(profile, expected)| profile.matches_identity(expected))
            {
                return Some(path);
            }
        }
    }
    let identity = identity?;
    profiles
        .iter()
        .find(|profile| profile.matches_identity(identity))
        .map(|profile| profile.path.clone())
}

pub fn relink_project_profiles(
    project: &mut ShadeProject,
    profiles: &[InstalledIccProfile],
) -> bool {
    let mut changed = false;
    changed |= relink_path(
        &mut project.preview_color.assigned_profile_path,
        project.preview_color.assigned_profile_identity.as_ref(),
        profiles,
    );
    changed |= relink_path(
        &mut project.preview_color.proof_profile_path,
        project.preview_color.proof_profile_identity.as_ref(),
        profiles,
    );
    for face in &mut project.faces {
        let Some(assignment) = face.production_source_profile.as_mut() else {
            continue;
        };
        let mut stored_path = Some(assignment.path.clone());
        if relink_path(&mut stored_path, Some(&assignment.identity), profiles) {
            assignment.path = stored_path.unwrap_or_default();
            changed = true;
        }
    }
    changed
}

pub fn relink_monitor_profile(
    settings: &mut AppSettings,
    profiles: &[InstalledIccProfile],
) -> bool {
    relink_path(
        &mut settings.monitor_profile_path,
        settings.monitor_profile_identity.as_ref(),
        profiles,
    )
}

fn relink_path(
    stored_path: &mut Option<String>,
    identity: Option<&IccProfileIdentity>,
    profiles: &[InstalledIccProfile],
) -> bool {
    let resolved = resolve_external_profile_path(stored_path.as_deref(), identity, profiles);
    let Some(resolved) = resolved else {
        return false;
    };
    let text = resolved.to_string_lossy().into_owned();
    if stored_path.as_deref() == Some(text.as_str()) {
        false
    } else {
        *stored_path = Some(text);
        true
    }
}

pub fn installed_profiles() -> Result<Vec<InstalledIccProfile>, String> {
    let directory = color_directory();
    let mut paths = Vec::new();

    #[cfg(windows)]
    {
        if let Ok(names) = registered_profile_names() {
            for name in names {
                let path = PathBuf::from(name);
                paths.push(if path.is_absolute() {
                    path
                } else {
                    directory.join(path)
                });
            }
        }
    }

    // Filesystem scan remains a fallback/supplement because vendor installers
    // sometimes place usable ICCs in the color directory without a device association.
    if let Ok(entries) = fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_profile_path(&path) {
                paths.push(path);
            }
        }
    } else if paths.is_empty() {
        return Err(format!(
            "Cannot enumerate Windows color profiles from {}",
            directory.display()
        ));
    }

    paths.sort_by_key(|path| path.to_string_lossy().to_lowercase());
    paths.dedup_by(|left, right| {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    });

    let mut profiles = paths
        .into_iter()
        .filter_map(|path| inspect_profile(&path).ok())
        .collect::<Vec<_>>();
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
    profiles.dedup_by(|left, right| left.identity.sha256 == right.identity.sha256);
    Ok(profiles)
}

fn color_directory() -> PathBuf {
    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    windows
        .join("System32")
        .join("spool")
        .join("drivers")
        .join("color")
}

#[cfg(windows)]
fn registered_profile_names() -> Result<Vec<String>, String> {
    let mut record = ENUMTYPEW::default();
    record.dwSize = std::mem::size_of::<ENUMTYPEW>() as u32;
    record.dwVersion = ENUM_TYPE_VERSION;
    record.dwFields = 0;

    let mut bytes_needed = 0u32;
    let mut profile_count = 0u32;
    unsafe {
        let _ = EnumColorProfilesW(
            std::ptr::null(),
            &record,
            std::ptr::null_mut(),
            &mut bytes_needed,
            &mut profile_count,
        );
    }
    if bytes_needed == 0 {
        return Err("Windows profile enumeration returned an empty buffer size.".to_owned());
    }

    let mut buffer = vec![0u8; bytes_needed as usize];
    let ok = unsafe {
        EnumColorProfilesW(
            std::ptr::null(),
            &record,
            buffer.as_mut_ptr(),
            &mut bytes_needed,
            &mut profile_count,
        )
    };
    if ok == 0 {
        return Err(format!(
            "EnumColorProfilesW failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let units = buffer
        .chunks_exact(2)
        .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    let mut names = Vec::with_capacity(profile_count as usize);
    let mut start = 0usize;
    for index in 0..units.len() {
        if units[index] != 0 {
            continue;
        }
        if index == start {
            break;
        }
        names.push(String::from_utf16_lossy(&units[start..index]));
        start = index + 1;
    }
    Ok(names)
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

fn expected_runtime_color_space(model: RuntimeColorModel) -> Option<ColorSpaceSignature> {
    match model {
        RuntimeColorModel::Rgb => Some(ColorSpaceSignature::RgbData),
        RuntimeColorModel::Cmyk => Some(ColorSpaceSignature::CmykData),
        RuntimeColorModel::Gray => Some(ColorSpaceSignature::GrayData),
        RuntimeColorModel::Other => None,
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

fn from_lcms_intent(intent: Intent) -> Option<PreviewRenderingIntent> {
    match intent {
        Intent::Perceptual => Some(PreviewRenderingIntent::Perceptual),
        Intent::RelativeColorimetric => Some(PreviewRenderingIntent::RelativeColorimetric),
        Intent::Saturation => Some(PreviewRenderingIntent::Saturation),
        Intent::AbsoluteColorimetric => Some(PreviewRenderingIntent::AbsoluteColorimetric),
        _ => None,
    }
}

fn preferred_profile_intent(
    profile: &Profile,
    fallback: PreviewRenderingIntent,
) -> PreviewRenderingIntent {
    from_lcms_intent(profile.header_rendering_intent()).unwrap_or(fallback)
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
    use windows_shade_editor::design_source_preview::DesignSourcePreview;
    use windows_shade_editor::png_source::{DecodedPngSource, PngSourceModel};

    fn metadata(model: ColorModel, icc_profile: Option<Vec<u8>>) -> TiffMetadata {
        let base = match model {
            ColorModel::Rgb => 3,
            ColorModel::Cmyk => 4,
            ColorModel::Gray => 1,
            ColorModel::Other => 1,
        };
        TiffMetadata {
            width: 1,
            height: 1,
            bit_depth: 16,
            samples_per_pixel: base,
            base_channel_count: base,
            color_model: model,
            non_cmyk_separated: false,
            channel_names: (0..base).map(|index| format!("C{index}")).collect(),
            channel_display_info: vec![None; base],
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
            assigned_profile_identity: None,
            soft_proof_enabled: false,
            proof_profile_path: None,
            proof_profile_identity: None,
            proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
            monitor_profile_path: None,
            monitor_profile_identity: None,
            gamut_warning: false,
        }
    }

    fn temp_profile(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "shade-color-{label}-{}-{}.icc",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut profile = Profile::new_srgb();
        profile.save_profile_to_file(&path).unwrap();
        path
    }

    #[test]
    fn no_profile_is_reported_without_modifying_data() {
        let transform = PreviewColorTransform::new(&metadata(ColorModel::Rgb, None), config());
        assert!(matches!(
            transform.status(),
            PreviewColorStatus::NoEmbeddedProfile
        ));
    }

    #[test]
    fn disabled_preview_is_explicit() {
        let mut cfg = config();
        cfg.enabled = false;
        let transform = PreviewColorTransform::new(&metadata(ColorModel::Rgb, None), cfg);
        assert!(matches!(transform.status(), PreviewColorStatus::Disabled));
    }

    #[test]
    fn missing_external_profile_is_a_fallback_not_silent_reassignment() {
        let mut cfg = config();
        cfg.assigned_profile_path = Some(PathBuf::from(r"Z:\missing\profile.icc"));
        let transform = PreviewColorTransform::new(&metadata(ColorModel::Rgb, None), cfg);
        assert!(matches!(
            transform.status(),
            PreviewColorStatus::Fallback { .. }
        ));
    }

    #[test]
    fn corrupt_external_profile_is_rejected() {
        let path = std::env::temp_dir().join(format!("shade-corrupt-{}.icc", std::process::id()));
        fs::write(&path, b"not an ICC").unwrap();
        assert!(inspect_profile(&path).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn wrong_source_color_space_is_rejected() {
        let path = temp_profile("wrong-space");
        let inspected = inspect_profile(&path).unwrap();
        let mut cfg = config();
        cfg.assigned_profile_path = Some(path.clone());
        cfg.assigned_profile_identity = Some(inspected.identity().clone());
        let transform = PreviewColorTransform::new(&metadata(ColorModel::Cmyk, None), cfg);
        assert!(matches!(
            transform.status(),
            PreviewColorStatus::Fallback { .. }
        ));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn wrong_proof_device_class_is_rejected() {
        let path = temp_profile("wrong-class");
        let inspected = inspect_profile(&path).unwrap();
        assert!(inspected.is_display_profile());
        assert!(!inspected.is_output_profile());
        let source_profile = Profile::new_srgb();
        let bytes = source_profile.icc().unwrap();
        let mut cfg = config();
        cfg.soft_proof_enabled = true;
        cfg.proof_profile_path = Some(path.clone());
        cfg.proof_profile_identity = Some(inspected.identity().clone());
        let transform = PreviewColorTransform::new(&metadata(ColorModel::Rgb, Some(bytes)), cfg);
        assert!(matches!(
            transform.status(),
            PreviewColorStatus::Fallback { .. }
        ));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn identity_relinks_moved_profile_without_embedding_it() {
        let original = temp_profile("identity-a");
        let moved =
            original.with_file_name(format!("shade-color-moved-{}.icc", std::process::id()));
        fs::copy(&original, &moved).unwrap();
        let expected = inspect_profile(&original).unwrap().identity().clone();
        let moved_profile = inspect_profile(&moved).unwrap();
        fs::remove_file(&original).unwrap();
        let resolved =
            resolve_external_profile_path(original.to_str(), Some(&expected), &[moved_profile])
                .unwrap();
        assert_eq!(resolved, moved);
        let _ = fs::remove_file(resolved);
    }

    #[test]
    fn production_embedded_profile_requires_matching_source_space() {
        let profile = Profile::new_srgb();
        let bytes = profile.icc().unwrap();
        let identity =
            production_embedded_profile_identity(&metadata(ColorModel::Rgb, Some(bytes.clone())))
                .unwrap()
                .unwrap();
        assert!(!identity.description.is_empty());
        assert_eq!(identity.sha256.len(), 64);

        let error = production_embedded_profile_identity(&metadata(ColorModel::Cmyk, Some(bytes)))
            .unwrap_err();
        assert!(error.contains("does not match source CMYK"));
    }

    #[test]
    fn production_assignment_detects_replaced_profile_identity() {
        let path = temp_profile("production-assignment");
        let inspected = inspect_profile(&path).unwrap();
        assert!(
            inspect_production_source_profile(&path, inspected.identity(), ColorModel::Rgb,)
                .is_ok()
        );

        let wrong_identity = IccProfileIdentity {
            description: "Different profile".to_owned(),
            sha256: "00".repeat(32),
        };
        let error =
            inspect_production_source_profile(&path, &wrong_identity, ColorModel::Rgb).unwrap_err();
        assert!(error.contains("no longer matches"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn production_assignment_relinks_by_identity_with_other_project_profiles() {
        let original = temp_profile("production-relink-a");
        let moved = original.with_file_name(format!(
            "shade-production-relink-b-{}.icc",
            std::process::id()
        ));
        fs::copy(&original, &moved).unwrap();
        let identity = inspect_profile(&original).unwrap().identity().clone();
        let moved_profile = inspect_profile(&moved).unwrap();
        fs::remove_file(&original).unwrap();

        let mut project = ShadeProject::default();
        project.faces.push(crate::model::FaceRef {
            path: "face.tif".to_owned(),
            label: "Face 1".to_owned(),
            status: crate::model::FaceStatus::Accepted,
            production_source_profile: Some(crate::model::ProductionSourceProfileAssignment {
                path: original.to_string_lossy().into_owned(),
                identity,
            }),
        });

        assert!(relink_project_profiles(&mut project, &[moved_profile]));
        assert_eq!(
            project.faces[0]
                .production_source_profile
                .as_ref()
                .map(|assignment| assignment.path.as_str()),
            moved.to_str()
        );
        let _ = fs::remove_file(moved);
    }

    #[test]
    fn all_standard_icc_header_intents_map_to_preview_intents() {
        for (lcms, expected) in [
            (Intent::Perceptual, PreviewRenderingIntent::Perceptual),
            (
                Intent::RelativeColorimetric,
                PreviewRenderingIntent::RelativeColorimetric,
            ),
            (Intent::Saturation, PreviewRenderingIntent::Saturation),
            (
                Intent::AbsoluteColorimetric,
                PreviewRenderingIntent::AbsoluteColorimetric,
            ),
        ] {
            assert_eq!(from_lcms_intent(lcms), Some(expected));
        }
    }

    #[test]
    fn embedded_profile_header_intent_drives_source_transform() {
        let mut profile = Profile::new_srgb();
        profile.set_header_rendering_intent(Intent::RelativeColorimetric);
        let bytes = profile.icc().unwrap();
        let mut cfg = config();
        cfg.intent = PreviewRenderingIntent::AbsoluteColorimetric;
        let transform = PreviewColorTransform::new(&metadata(ColorModel::Rgb, Some(bytes)), cfg);
        assert_eq!(
            transform.status().source_rendering_intent(),
            Some(PreviewRenderingIntent::RelativeColorimetric)
        );
    }

    #[test]
    fn assigned_profile_header_intent_drives_source_transform() {
        let path = std::env::temp_dir().join(format!(
            "shade-source-intent-{}-{}.icc",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut profile = Profile::new_srgb();
        profile.set_header_rendering_intent(Intent::AbsoluteColorimetric);
        profile.save_profile_to_file(&path).unwrap();
        let inspected = inspect_profile(&path).unwrap();
        let mut cfg = config();
        cfg.intent = PreviewRenderingIntent::Perceptual;
        cfg.assigned_profile_path = Some(path.clone());
        cfg.assigned_profile_identity = Some(inspected.identity().clone());
        let transform = PreviewColorTransform::new(&metadata(ColorModel::Rgb, None), cfg);
        assert_eq!(
            transform.status().source_rendering_intent(),
            Some(PreviewRenderingIntent::AbsoluteColorimetric)
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn png_embedded_icc_uses_runtime_preview_contract() {
        let profile = Profile::new_srgb();
        let bytes = profile.icc().unwrap();
        let decoded = DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![100, 200, 300],
            alpha: None,
            icc_profile: Some(bytes),
            declares_srgb: false,
        };
        let preview = DesignSourcePreview::from_png(&decoded, 512).expect("PNG preview");
        let transform = PreviewColorTransform::new_for_runtime_preview(&preview, config());
        assert!(matches!(
            transform.status(),
            PreviewColorStatus::Applied { .. }
        ));
    }
}
