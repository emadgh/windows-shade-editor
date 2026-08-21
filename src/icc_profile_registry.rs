use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use lcms2::{
    ColorSpaceSignature, ColorSpaceSignatureExt, InfoType, Locale, Profile, ProfileClassSignature,
};
use sha2::{Digest, Sha256};

#[cfg(windows)]
use windows_sys::Win32::UI::ColorSystem::{ENUM_TYPE_VERSION, ENUMTYPEW, EnumColorProfilesW};

use crate::model::IccProfileIdentity;
use crate::tiff_io::ColorModel;

/// Canonical profile role used by both preview Color Management and production conversion UI.
/// Transform policy deliberately does not live in this registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IccProfileRole {
    Input,
    Display,
    Output,
    DeviceLink,
    Abstract,
    ColorSpace,
    NamedColor,
    Other,
}

impl IccProfileRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Display => "Display",
            Self::Output => "Output / printer",
            Self::DeviceLink => "DeviceLink",
            Self::Abstract => "Abstract",
            Self::ColorSpace => "Color space",
            Self::NamedColor => "Named color",
            Self::Other => "Other",
        }
    }
}

/// Immutable facts derived from the ICC bytes at `path`.
///
/// The SHA-256 identity is the authority for same-profile/replacement checks; path and
/// description are locators/display metadata only.
#[derive(Clone, Debug)]
pub struct IccProfileRecord {
    pub path: PathBuf,
    pub description: String,
    pub identity: IccProfileIdentity,
    pub role: IccProfileRole,
    color_space: ColorSpaceSignature,
    pcs_space: ColorSpaceSignature,
}

impl IccProfileRecord {
    pub fn filename(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn color_space_label(&self) -> String {
        color_space_label(self.color_space)
    }

    pub fn pcs_space_label(&self) -> String {
        color_space_label(self.pcs_space)
    }

    /// Number of channels represented by the profile's declared device/input space.
    pub fn color_space_channels(&self) -> usize {
        self.color_space.channels() as usize
    }

    /// For DeviceLinks this is the output device topology. For ordinary profiles this
    /// is normally PCS and should not be interpreted as a production output topology.
    pub fn pcs_space_channels(&self) -> usize {
        self.pcs_space.channels() as usize
    }

    pub fn is_output_profile(&self) -> bool {
        self.role == IccProfileRole::Output
    }

    pub fn is_display_profile(&self) -> bool {
        self.role == IccProfileRole::Display && self.color_space == ColorSpaceSignature::RgbData
    }

    pub fn is_device_link(&self) -> bool {
        self.role == IccProfileRole::DeviceLink
    }

    pub fn compatible_with_source_model(&self, model: ColorModel) -> bool {
        expected_color_space(model).is_some_and(|expected| expected == self.color_space)
    }

    pub fn matches_identity(&self, identity: &IccProfileIdentity) -> bool {
        !identity.sha256.trim().is_empty()
            && self
                .identity
                .sha256
                .eq_ignore_ascii_case(identity.sha256.trim())
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
            || self.role.label().to_lowercase().contains(&query)
    }
}

#[derive(Clone)]
struct CachedProfileInspection {
    size: u64,
    modified_ns: Option<u128>,
    profile: IccProfileRecord,
}

fn profile_cache() -> &'static Mutex<HashMap<PathBuf, CachedProfileInspection>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedProfileInspection>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stateless facade over the process-wide inspection cache and Windows profile registry.
#[derive(Clone, Copy, Debug, Default)]
pub struct IccProfileRegistry;

impl IccProfileRegistry {
    /// Cached inspection suitable for profile browsing/search UI.
    pub fn inspect(self, path: &Path) -> Result<IccProfileRecord, String> {
        inspect_profile(path)
    }

    pub fn installed(self) -> Result<Vec<IccProfileRecord>, String> {
        installed_profiles()
    }

    pub fn resolve(
        self,
        stored_path: Option<&str>,
        identity: Option<&IccProfileIdentity>,
        profiles: &[IccProfileRecord],
    ) -> Option<PathBuf> {
        resolve_external_profile_path(stored_path, identity, profiles)
    }

    /// Production/persistence identity boundary: always re-read and re-hash the bytes.
    /// The browsing cache is deliberately not authoritative here.
    pub fn verify_identity(
        self,
        path: &Path,
        expected: &IccProfileIdentity,
    ) -> Result<IccProfileRecord, String> {
        if expected.sha256.trim().is_empty() {
            return Err("ICC profile has no stored SHA-256 identity. Select it again.".to_owned());
        }
        let actual = inspect_profile_fresh(path)?;
        if !actual.matches_identity(expected) {
            return Err(format!(
                "ICC profile at {} no longer matches stored profile '{}'. Relink or select it again.",
                path.display(),
                expected.description
            ));
        }
        Ok(actual)
    }
}

/// Cached profile inspection. Cache metadata is an optimization only; callers that make
/// production or persistence decisions must use `inspect_profile_fresh`/`verify_identity`.
pub fn inspect_profile(path: &Path) -> Result<IccProfileRecord, String> {
    let metadata = profile_file_metadata(path)?;
    let size = metadata.len();
    let modified_ns = modified_ns(&metadata);
    let cache_key = canonical_cache_key(path);
    if let Ok(cache) = profile_cache().lock() {
        if let Some(cached) = cache.get(&cache_key) {
            if cached.size == size && cached.modified_ns == modified_ns {
                return Ok(cached.profile.clone());
            }
        }
    }
    inspect_profile_fresh(path)
}

/// Unconditionally re-read and hash profile bytes, then refresh the browsing cache.
pub fn inspect_profile_fresh(path: &Path) -> Result<IccProfileRecord, String> {
    let metadata = profile_file_metadata(path)?;
    let bytes = fs::read(path)
        .map_err(|err| format!("Cannot read ICC profile {}: {err}", path.display()))?;
    let profile = Profile::new_icc(&bytes)
        .map_err(|err| format!("Cannot open ICC profile {}: {err}", path.display()))?;
    let description = profile_description(&profile);
    let inspected = IccProfileRecord {
        path: path.to_path_buf(),
        identity: IccProfileIdentity {
            description: description.clone(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        },
        description,
        role: profile_role(profile.device_class()),
        color_space: profile.color_space(),
        pcs_space: profile.pcs(),
    };
    cache_inspection(path, &metadata, &inspected);
    Ok(inspected)
}

fn profile_file_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::metadata(path)
        .map_err(|err| format!("Cannot inspect ICC profile {}: {err}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("ICC profile is not a file: {}", path.display()));
    }
    Ok(metadata)
}

fn modified_ns(metadata: &fs::Metadata) -> Option<u128> {
    metadata.modified().ok().and_then(|time| {
        time.duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_nanos())
    })
}

fn canonical_cache_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn cache_inspection(path: &Path, metadata: &fs::Metadata, inspected: &IccProfileRecord) {
    if let Ok(mut cache) = profile_cache().lock() {
        cache.insert(
            canonical_cache_key(path),
            CachedProfileInspection {
                size: metadata.len(),
                modified_ns: modified_ns(metadata),
                profile: inspected.clone(),
            },
        );
    }
}

pub fn installed_profiles() -> Result<Vec<IccProfileRecord>, String> {
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

    // Supplement Windows registration with the color directory. Some vendor/RIP
    // installers deploy valid profiles without associating them with a device.
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
    // Same ICC bytes installed under two paths are one logical profile identity.
    profiles.dedup_by(|left, right| {
        left.identity
            .sha256
            .eq_ignore_ascii_case(&right.identity.sha256)
    });
    Ok(profiles)
}

pub fn resolve_external_profile_path(
    stored_path: Option<&str>,
    identity: Option<&IccProfileIdentity>,
    profiles: &[IccProfileRecord],
) -> Option<PathBuf> {
    if let Some(path) = stored_path.map(PathBuf::from) {
        if path.is_file() {
            if identity.is_none()
                || inspect_profile_fresh(&path)
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

fn profile_role(class: ProfileClassSignature) -> IccProfileRole {
    match class {
        ProfileClassSignature::InputClass => IccProfileRole::Input,
        ProfileClassSignature::DisplayClass => IccProfileRole::Display,
        ProfileClassSignature::OutputClass => IccProfileRole::Output,
        ProfileClassSignature::LinkClass => IccProfileRole::DeviceLink,
        ProfileClassSignature::AbstractClass => IccProfileRole::Abstract,
        ProfileClassSignature::ColorSpaceClass => IccProfileRole::ColorSpace,
        ProfileClassSignature::NamedColorClass => IccProfileRole::NamedColor,
        _ => IccProfileRole::Other,
    }
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
        ColorSpaceSignature::LabData => "Lab".to_owned(),
        ColorSpaceSignature::XYZData => "XYZ".to_owned(),
        other => format!("{other:?}"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_profile(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "shade-profile-registry-{label}-{}-{}.icc",
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
    fn inspection_has_stable_content_identity_and_display_role() {
        let path = temp_profile("identity");
        let first = inspect_profile(&path).unwrap();
        let second = inspect_profile(&path).unwrap();
        assert_eq!(first.identity, second.identity);
        assert_eq!(first.identity.sha256.len(), 64);
        assert_eq!(first.role, IccProfileRole::Display);
        assert!(first.is_display_profile());
        assert!(first.compatible_with_source_model(ColorModel::Rgb));
        assert!(!first.compatible_with_source_model(ColorModel::Cmyk));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn moved_profile_resolves_by_identity_not_filename() {
        let original = temp_profile("move-a");
        let moved = original.with_file_name(format!(
            "shade-profile-registry-move-b-{}.icc",
            std::process::id()
        ));
        fs::copy(&original, &moved).unwrap();
        let expected = inspect_profile(&original).unwrap().identity;
        let moved_record = inspect_profile(&moved).unwrap();
        fs::remove_file(&original).unwrap();

        let resolved = resolve_external_profile_path(
            original.to_str(),
            Some(&expected),
            &[moved_record],
        )
        .unwrap();
        assert_eq!(resolved, moved);
        let _ = fs::remove_file(resolved);
    }

    #[test]
    fn same_size_path_replacement_cannot_reuse_cached_identity() {
        let path = temp_profile("replace");
        let expected = inspect_profile(&path).unwrap().identity;
        let original_len = fs::metadata(&path).unwrap().len() as usize;
        fs::write(&path, vec![0u8; original_len]).unwrap();

        let error = IccProfileRegistry
            .verify_identity(&path, &expected)
            .unwrap_err();
        assert!(error.contains("Cannot open ICC profile") || error.contains("no longer matches"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn query_matches_role_description_filename_and_color_space() {
        let path = temp_profile("query");
        let record = inspect_profile(&path).unwrap();
        assert!(record.matches_query("display"));
        assert!(record.matches_query("rgb"));
        assert!(record.matches_query("query"));
        assert!(record.matches_query(""));
        let _ = fs::remove_file(path);
    }
}
