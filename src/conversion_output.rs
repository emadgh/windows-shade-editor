use std::path::{Component, Path, PathBuf};

use crate::tiff_output;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::path::Prefix;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputCollisionPolicy {
    /// Existing output is never replaced implicitly. Generate a versioned path.
    Versioned,
    /// Explicit transactional replacement requested by the operator. The caller
    /// must still write/validate a temporary file and atomically commit it.
    TransactionalReplace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputPathError {
    SameAsSource,
    UnsupportedExtension,
    MissingFileName,
}

impl std::fmt::Display for OutputPathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SameAsSource => "Conversion output cannot replace the source file.",
            Self::UnsupportedExtension => "Conversion output must use .tif or .tiff.",
            Self::MissingFileName => {
                "Conversion output must use a valid absolute Windows path and file name."
            }
        })
    }
}

/// Validate the non-destructive conversion boundary.
///
/// Production conversion output is TIFF/BigTIFF and can never be the source path.
/// On Windows, the destination must also be a safe absolute drive/UNC path whose
/// components and staged-write path fit the Win32 naming contract used by the
/// atomic conversion writer.
pub fn validate_conversion_output_path(
    source: &Path,
    destination: &Path,
) -> Result<(), OutputPathError> {
    if windows_paths_equivalent(source, destination) {
        return Err(OutputPathError::SameAsSource);
    }
    validate_conversion_destination_path(destination)
}

/// Validate a conversion TIFF destination independently from its source path.
/// This is also used by versioned-path generation so `_vN` cannot push an
/// otherwise valid destination beyond the Windows/staging bounds.
pub fn validate_conversion_destination_path(destination: &Path) -> Result<(), OutputPathError> {
    let extension = destination
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or(OutputPathError::UnsupportedExtension)?;
    if extension != "tif" && extension != "tiff" {
        return Err(OutputPathError::UnsupportedExtension);
    }
    if destination.file_stem().is_none() {
        return Err(OutputPathError::MissingFileName);
    }
    validate_windows_destination_path(destination)?;
    Ok(())
}

/// Build a recommended production output name beside or inside a caller-selected
/// directory. Target suffixes are presentation identifiers only; provenance keeps
/// the exact target/profile identity separately.
pub fn default_converted_filename(
    source: &Path,
    target_suffix: &str,
) -> Result<PathBuf, OutputPathError> {
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or(OutputPathError::MissingFileName)?;
    let suffix = sanitize_suffix(target_suffix);
    let suffix = if suffix.is_empty() {
        "converted"
    } else {
        suffix.as_str()
    };
    Ok(PathBuf::from(format!("{stem}_{suffix}.tif")))
}

/// Build the canonical Production TIFF filename for one Source Face.
///
/// The name deliberately excludes target/profile labels so Current / Selected / All scopes map
/// the same Source Face to the same path. `face_disambiguator` is supplied only when the Source
/// project contains duplicate file stems, and must therefore come from stable Source Face identity.
pub fn deterministic_converted_filename(
    source: &Path,
    face_disambiguator: Option<usize>,
) -> Result<PathBuf, OutputPathError> {
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or(OutputPathError::MissingFileName)?;
    let name = match face_disambiguator {
        Some(index) => format!("{stem}_F{index:02}.tif"),
        None => format!("{stem}.tif"),
    };
    Ok(PathBuf::from(name))
}

/// Return the first free `_vN` path when `preferred` already exists. This remains available for
/// legacy workflows. Unified Production Color Conversion intentionally does not call it because
/// versioned names break deterministic Source↔converted Face mapping.
pub fn next_versioned_output_path(preferred: &Path) -> Result<PathBuf, OutputPathError> {
    validate_conversion_destination_path(preferred)?;
    if !preferred.exists() {
        return Ok(preferred.to_path_buf());
    }
    let parent = preferred.parent().unwrap_or_else(|| Path::new(""));
    let stem = preferred
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or(OutputPathError::MissingFileName)?;
    let extension = preferred
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("tif");

    for version in 2u32..=u32::MAX {
        let candidate = parent.join(format!("{stem}_v{version}.{extension}"));
        validate_conversion_destination_path(&candidate)?;
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    unreachable!("u32 version namespace exhausted")
}

fn sanitize_suffix(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_owned()
}

#[cfg(windows)]
const MAX_WIN32_PATH_UTF16_WITHOUT_NUL: usize = 259;
#[cfg(windows)]
const MAX_VERBATIM_PATH_UTF16_WITHOUT_NUL: usize = 32_766;
#[cfg(windows)]
const MAX_WINDOWS_COMPONENT_UTF16: usize = 255;
const CONVERSION_STAGING_SUFFIX: &str = ".conversion.tmp";

#[cfg(windows)]
fn validate_windows_destination_path(destination: &Path) -> Result<(), OutputPathError> {
    if !destination.is_absolute() {
        return Err(OutputPathError::MissingFileName);
    }

    let prefix = match destination.components().next() {
        Some(Component::Prefix(prefix)) => prefix.kind(),
        _ => return Err(OutputPathError::MissingFileName),
    };
    let verbatim = match prefix {
        Prefix::Disk(_) => false,
        Prefix::UNC(server, share) => {
            validate_unc_authority(server, share)?;
            false
        }
        Prefix::VerbatimDisk(_) => true,
        Prefix::VerbatimUNC(server, share) => {
            validate_unc_authority(server, share)?;
            true
        }
        Prefix::Verbatim(_) | Prefix::DeviceNS(_) => {
            return Err(OutputPathError::MissingFileName);
        }
    };

    for component in destination.components() {
        if let Component::Normal(name) = component {
            validate_windows_component(name, true)?;
        }
    }

    let file_name = destination
        .file_name()
        .ok_or(OutputPathError::MissingFileName)?;
    let staging_reserve = tiff_output::staging_suffix_utf16_reserve(CONVERSION_STAGING_SUFFIX);
    let staged_file_units = windows_utf16_len(file_name)
        .checked_add(staging_reserve)
        .ok_or(OutputPathError::MissingFileName)?;
    if staged_file_units > MAX_WINDOWS_COMPONENT_UTF16 {
        return Err(OutputPathError::MissingFileName);
    }

    let staged_path_units = windows_utf16_len(destination.as_os_str())
        .checked_add(staging_reserve)
        .ok_or(OutputPathError::MissingFileName)?;
    let max_path_units = if verbatim {
        MAX_VERBATIM_PATH_UTF16_WITHOUT_NUL
    } else {
        MAX_WIN32_PATH_UTF16_WITHOUT_NUL
    };
    if staged_path_units > max_path_units {
        return Err(OutputPathError::MissingFileName);
    }

    Ok(())
}

#[cfg(not(windows))]
fn validate_windows_destination_path(_destination: &Path) -> Result<(), OutputPathError> {
    // Shade Editor production builds are Windows-native. Keep the library
    // portable for non-Windows tooling while enforcing Win32 syntax in the
    // Windows build/test target where these paths are executable.
    Ok(())
}

#[cfg(windows)]
fn validate_unc_authority(server: &OsStr, share: &OsStr) -> Result<(), OutputPathError> {
    if server.is_empty() || share.is_empty() {
        return Err(OutputPathError::MissingFileName);
    }
    validate_windows_component(server, false)?;
    validate_windows_component(share, false)
}

#[cfg(windows)]
fn validate_windows_component(name: &OsStr, reject_reserved: bool) -> Result<(), OutputPathError> {
    let units = name.encode_wide().collect::<Vec<_>>();
    if units.is_empty() || units.len() > MAX_WINDOWS_COMPONENT_UTF16 {
        return Err(OutputPathError::MissingFileName);
    }
    const INVALID_ASCII: &[u16] = &[
        b'<' as u16,
        b'>' as u16,
        b':' as u16,
        b'"' as u16,
        b'/' as u16,
        b'\\' as u16,
        b'|' as u16,
        b'?' as u16,
        b'*' as u16,
    ];
    if units
        .iter()
        .copied()
        .any(|unit| unit < 32 || INVALID_ASCII.contains(&unit))
        || matches!(units.last(), Some(32 | 46))
    {
        return Err(OutputPathError::MissingFileName);
    }
    if reject_reserved && is_reserved_windows_device_name(&name.to_string_lossy()) {
        return Err(OutputPathError::MissingFileName);
    }
    Ok(())
}

#[cfg(windows)]
fn windows_utf16_len(value: &OsStr) -> usize {
    value.encode_wide().count()
}

#[cfg(windows)]
fn is_reserved_windows_device_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || base
            .strip_prefix("COM")
            .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
        || base
            .strip_prefix("LPT")
            .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
}

/// Lexical Windows comparison used even when the destination does not exist yet.
/// Existing paths are canonicalized first when possible; fallback comparison is
/// case-insensitive and normalizes `.` / `..` components without touching disk.
fn windows_paths_equivalent(first: &Path, second: &Path) -> bool {
    match (std::fs::canonicalize(first), std::fs::canonicalize(second)) {
        (Ok(first), Ok(second)) => first
            .to_string_lossy()
            .eq_ignore_ascii_case(&second.to_string_lossy()),
        _ => lexical_windows_key(first) == lexical_windows_key(second),
    }
}

fn lexical_windows_key(path: &Path) -> String {
    let mut stack = Vec::<String>::new();
    let mut prefix = String::new();

    for component in path.components() {
        match component {
            Component::Prefix(value) => {
                prefix = value.as_os_str().to_string_lossy().to_ascii_lowercase();
            }
            Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = stack.pop();
            }
            Component::Normal(value) => stack.push(value.to_string_lossy().to_ascii_lowercase()),
        }
    }

    format!("{}\\{}", prefix, stack.join("\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_path_can_never_be_conversion_destination() {
        let source = Path::new(r"C:\Designs\Face01.tif");
        assert_eq!(
            validate_conversion_output_path(source, Path::new(r"c:\designs\.\Face01.tif")),
            Err(OutputPathError::SameAsSource)
        );
    }

    #[test]
    fn production_output_is_tiff_only() {
        let source = Path::new(r"C:\Designs\Face01.png");
        assert!(
            validate_conversion_output_path(source, Path::new(r"C:\Production\Face01_7C.tif"))
                .is_ok()
        );
        assert_eq!(
            validate_conversion_output_path(source, Path::new(r"C:\Production\Face01_7C.png")),
            Err(OutputPathError::UnsupportedExtension)
        );
    }

    #[test]
    fn deterministic_name_is_scope_and_target_independent() {
        assert_eq!(
            deterministic_converted_filename(Path::new(r"C:\Design\Face01.png"), None).unwrap(),
            PathBuf::from("Face01.tif")
        );
        assert_eq!(
            deterministic_converted_filename(Path::new(r"C:\A\Face01.tif"), Some(3)).unwrap(),
            PathBuf::from("Face01_F03.tif")
        );
    }

    #[test]
    fn default_filename_uses_sanitized_target_suffix() {
        let name =
            default_converted_filename(Path::new("Face01.png"), "Durst 7C / Nano").unwrap();
        assert_eq!(name, PathBuf::from("Face01_Durst_7C___Nano.tif"));
    }

    #[test]
    fn lexical_comparison_normalizes_parent_components() {
        assert!(windows_paths_equivalent(
            Path::new(r"D:\A\B\..\Face.tif"),
            Path::new(r"d:\a\Face.tif")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn local_drive_unc_and_verbatim_unc_destinations_are_supported() {
        let source = Path::new(r"C:\Designs\Face01.tif");
        for destination in [
            r"D:\Production\Face01_7C.tif",
            r"\\print-server\ceramic-rip\Face01_7C.tif",
            r"\\?\UNC\print-server\ceramic-rip\Face01_7C.tif",
        ] {
            assert!(
                validate_conversion_output_path(source, Path::new(destination)).is_ok(),
                "expected valid production destination: {destination}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn relative_and_device_namespace_destinations_are_rejected() {
        let source = Path::new(r"C:\Designs\Face01.tif");
        for destination in [r"Production\Face01.tif", r"C:Face01.tif", r"\\.\C:\Face01.tif"] {
            assert_eq!(
                validate_conversion_output_path(source, Path::new(destination)),
                Err(OutputPathError::MissingFileName),
                "expected unsafe destination rejection: {destination}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn invalid_reserved_and_trailing_windows_names_are_rejected() {
        let source = Path::new(r"C:\Designs\Face01.tif");
        for destination in [
            r"C:\Production\bad?.tif",
            r"C:\Production\bad|name.tif",
            r"C:\Production\CON.tif",
            r"C:\Production\com1.output.tif",
            r"C:\Production\COM¹.tif",
            r"C:\Production\lpt².output.tif",
            r"C:\Production\folder.\Face01.tif",
            "C:\\Production\\trailing-space \\Face01.tif",
        ] {
            assert_eq!(
                validate_conversion_output_path(source, Path::new(destination)),
                Err(OutputPathError::MissingFileName),
                "expected invalid Windows-name rejection: {destination}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn non_verbatim_path_reserves_space_for_atomic_staging_name() {
        let source = Path::new(r"C:\Designs\Face01.tif");
        let destination = PathBuf::from(format!(
            r"C:\{}\{}\Face01.tif",
            "a".repeat(120),
            "b".repeat(120)
        ));
        assert_eq!(
            validate_conversion_output_path(source, &destination),
            Err(OutputPathError::MissingFileName)
        );
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_path_can_exceed_classic_max_path_but_not_component_limit() {
        let source = Path::new(r"C:\Designs\Face01.tif");
        let destination = PathBuf::from(format!(
            r"\\?\C:\{}\{}\Face01.tif",
            "a".repeat(120),
            "b".repeat(120)
        ));
        assert!(validate_conversion_output_path(source, &destination).is_ok());

        let oversized_file = PathBuf::from(format!(
            r"\\?\C:\Production\{}.tif",
            "x".repeat(237)
        ));
        assert_eq!(
            validate_conversion_output_path(source, &oversized_file),
            Err(OutputPathError::MissingFileName)
        );
    }
}
