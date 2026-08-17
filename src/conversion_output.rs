use std::path::{Component, Path, PathBuf};

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
            Self::MissingFileName => "Conversion output must include a file name.",
        })
    }
}

/// Validate the non-destructive conversion boundary.
///
/// Production conversion output is TIFF/BigTIFF and can never be the source path.
pub fn validate_conversion_output_path(
    source: &Path,
    destination: &Path,
) -> Result<(), OutputPathError> {
    if windows_paths_equivalent(source, destination) {
        return Err(OutputPathError::SameAsSource);
    }
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

/// Return the first free `_vN` path when `preferred` already exists. This never
/// deletes or replaces anything and is therefore safe as the default reconversion policy.
pub fn next_versioned_output_path(preferred: &Path) -> Result<PathBuf, OutputPathError> {
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
}
