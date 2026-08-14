use std::fs;
use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
use std::fs::File;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};

/// Stable comparison key for Windows-oriented destination reservation.
/// Existing ancestors are canonicalized so aliases, `..` and symlinked folders
/// collapse where the OS can resolve them. Windows comparisons are case-insensitive.
pub fn path_key(path: &Path) -> String {
    normalized_absolute(path)
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase()
}

pub fn same_path_or_file(left: &Path, right: &Path) -> bool {
    if path_key(left) == path_key(right) {
        return true;
    }

    #[cfg(windows)]
    {
        if left.exists() && right.exists() {
            if let (Ok(left_id), Ok(right_id)) = (windows_file_id(left), windows_file_id(right)) {
                return left_id == right_id;
            }
        }
    }

    false
}

pub fn conflicts_with_any_source(destination: &Path, sources: &[PathBuf]) -> Option<PathBuf> {
    sources
        .iter()
        .find(|source| same_path_or_file(destination, source))
        .cloned()
}

fn normalized_absolute(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }

    // Destination files commonly do not exist yet. Canonicalize the nearest
    // existing parent and append the unresolved tail, then normalize dot segments.
    let mut ancestor = path.parent();
    while let Some(parent) = ancestor {
        if let Ok(canonical_parent) = fs::canonicalize(parent) {
            if let Ok(tail) = path.strip_prefix(parent) {
                return clean_components(canonical_parent.join(tail));
            }
        }
        ancestor = parent.parent();
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    clean_components(absolute)
}

fn clean_components(path: PathBuf) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

#[cfg(windows)]
fn windows_file_id(path: &Path) -> Result<(u32, u32, u32), String> {
    let file = File::open(path)
        .map_err(|err| format!("Cannot open {} for identity check: {err}", path.display()))?;
    let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if ok == 0 {
        return Err(format!(
            "Cannot read file identity for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_key_collapses_dot_segments() {
        let base = std::env::temp_dir().join("shade-path-safety");
        let left = base.join("folder").join("..").join("file.tif");
        let right = base.join("file.tif");
        assert_eq!(path_key(&left), path_key(&right));
    }

    #[cfg(windows)]
    #[test]
    fn windows_key_is_case_insensitive() {
        assert_eq!(
            path_key(Path::new(r"C:\Tiles\Face.TIF")),
            path_key(Path::new(r"c:\tiles\face.tif"))
        );
    }
}