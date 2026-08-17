use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

pub fn backup_path(path: &Path) -> PathBuf {
    append_suffix(path, ".bak")
}

pub fn temp_path(path: &Path) -> PathBuf {
    append_suffix(path, ".tmp")
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

pub fn atomic_write(path: &Path, bytes: &[u8], backup: Option<&Path>) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|err| format!("Cannot create folder {}: {err}", parent.display()))?;

    let temp = temp_path(path);
    if temp.exists() {
        fs::remove_file(&temp)
            .map_err(|err| format!("Cannot remove stale temp file {}: {err}", temp.display()))?;
    }

    let write_result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp)
            .map_err(|err| format!("Cannot create temp file {}: {err}", temp.display()))?;
        file.write_all(bytes)
            .map_err(|err| format!("Cannot write temp file {}: {err}", temp.display()))?;
        file.flush()
            .map_err(|err| format!("Cannot flush temp file {}: {err}", temp.display()))?;
        file.sync_all()
            .map_err(|err| format!("Cannot sync temp file {}: {err}", temp.display()))?;
        drop(file);

        let persisted_len = fs::metadata(&temp)
            .map_err(|err| format!("Cannot inspect temp file {}: {err}", temp.display()))?
            .len();
        if persisted_len != bytes.len() as u64 {
            return Err(format!(
                "Temp file verification failed for {}: expected {} bytes, found {}.",
                temp.display(),
                bytes.len(),
                persisted_len
            ));
        }

        if let Some(backup) = backup.filter(|_| path.exists()) {
            atomic_copy(path, backup)?;
        }

        replace_path(&temp, path)?;
        Ok(())
    })();

    if write_result.is_err() && temp.exists() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

pub fn atomic_copy(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|err| format!("Cannot create folder {}: {err}", parent.display()))?;
    let temp = temp_path(destination);
    if temp.exists() {
        fs::remove_file(&temp)
            .map_err(|err| format!("Cannot remove stale temp file {}: {err}", temp.display()))?;
    }
    fs::copy(source, &temp).map_err(|err| {
        format!(
            "Cannot stage backup {} from {}: {err}",
            temp.display(),
            source.display()
        )
    })?;
    OpenOptions::new()
        .write(true)
        .open(&temp)
        .and_then(|file| file.sync_all())
        .map_err(|err| format!("Cannot sync staged backup {}: {err}", temp.display()))?;
    replace_path(&temp, destination).map_err(|err| {
        let _ = fs::remove_file(&temp);
        err
    })
}

/// Atomically commit a fully written and validated staged file. The staged file
/// must be beside the destination so the final rename cannot cross volumes.
pub fn commit_staged_file(staged: &Path, destination: &Path) -> Result<(), String> {
    if staged.parent() != destination.parent() {
        return Err("Atomic commit requires the staged file beside its destination.".to_owned());
    }
    if !staged.is_file() {
        return Err(format!("Staged file is missing: {}", staged.display()));
    }
    OpenOptions::new()
        .write(true)
        .open(staged)
        .and_then(|file| file.sync_all())
        .map_err(|err| format!("Cannot sync staged file {}: {err}", staged.display()))?;
    replace_path(staged, destination)
}

#[cfg(windows)]
fn replace_path(source: &Path, destination: &Path) -> Result<(), String> {
    let source_wide = wide_path(source);
    let destination_wide = wide_path(destination);
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(format!(
            "Cannot atomically replace {} with {}: {}",
            destination.display(),
            source.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_path(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|err| {
        format!(
            "Cannot atomically replace {} with {}: {err}",
            destination.display(),
            source.display()
        )
    })
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-safe-fs-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn atomic_write_replaces_target_and_keeps_previous_backup() {
        let folder = temp_folder("replace");
        fs::create_dir_all(&folder).unwrap();
        let target = folder.join("project.shade");
        let backup = backup_path(&target);
        fs::write(&target, b"old-project").unwrap();

        atomic_write(&target, b"new-project", Some(&backup)).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new-project");
        assert_eq!(fs::read(&backup).unwrap(), b"old-project");
        assert!(!temp_path(&target).exists());
        assert!(!temp_path(&backup).exists());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn first_atomic_write_does_not_create_a_fake_backup() {
        let folder = temp_folder("first");
        fs::create_dir_all(&folder).unwrap();
        let target = folder.join("first.shade");
        let backup = backup_path(&target);

        atomic_write(&target, b"first-save", Some(&backup)).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"first-save");
        assert!(!backup.exists());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn staged_file_commit_replaces_destination_atomically() {
        let folder = temp_folder("staged-commit");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        let staged = folder.join("output.tif.conversion.tmp");
        fs::write(&destination, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();

        commit_staged_file(&staged, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!staged.exists());
        let _ = fs::remove_dir_all(folder);
    }
}
