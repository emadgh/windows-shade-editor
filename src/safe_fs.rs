use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

use crate::tiff_performance::{self, TiffPerfPhase};

#[path = "staging.rs"]
pub mod staging;

pub fn backup_path(path: &Path) -> PathBuf {
    append_suffix(path, staging::BACKUP_SUFFIX)
}

pub fn temp_path(path: &Path) -> PathBuf {
    append_suffix(path, staging::SAFE_FS_TEMP_SUFFIX)
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

pub fn atomic_write_if_absent(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|err| format!("Cannot create folder {}: {err}", parent.display()))?;
    let temp = temp_path(path);
    if temp.exists() {
        fs::remove_file(&temp)
            .map_err(|err| format!("Cannot remove stale temp file {}: {err}", temp.display()))?;
    }
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(|err| format!("Cannot create temp file {}: {err}", temp.display()))?;
        file.write_all(bytes)
            .map_err(|err| format!("Cannot write temp file {}: {err}", temp.display()))?;
        file.flush()
            .and_then(|_| file.sync_all())
            .map_err(|err| format!("Cannot sync temp file {}: {err}", temp.display()))?;
        drop(file);
        commit_staged_file_if_absent(&temp, path)
    })();
    if result.is_err() && temp.exists() {
        let _ = fs::remove_file(&temp);
    }
    result
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

/// Atomically publish a fully written and validated staged file, replacing the destination.
///
/// # Durability contract
///
/// Callers must finish writing and validating `staged` before entering this boundary. This
/// function then:
///
/// 1. requires `staged` to be a sibling of `destination`, so the commit cannot cross volumes;
/// 2. calls `sync_all` on the staged file so its file contents/metadata reach the OS durability
///    boundary before the name is published; and
/// 3. atomically replaces the destination and requests durable directory/name metadata.
///
/// On Windows the final operation is `MoveFileExW(..., MOVEFILE_REPLACE_EXISTING |
/// MOVEFILE_WRITE_THROUGH)`. `MOVEFILE_WRITE_THROUGH` is the Win32 equivalent used here for the
/// final rename/name-metadata durability barrier. On non-Windows builds the rename is followed by
/// an explicit parent-directory `sync_all`.
///
/// A successful return means Shade Editor has completed the strongest local-filesystem durability
/// sequence available through these platform APIs. It does **not** override storage-controller,
/// remote-server or network-share caching policy. In particular, UNC/SMB durability ultimately
/// depends on the server/filesystem honoring the write-through request; transport loss or a remote
/// server crash can therefore have semantics outside the application's control.
///
/// This function does not add a Windows verbatim (`\\?\\`) prefix or shorten an invalid final file
/// name. Callers creating large TIFF staging names should use `tiff_output`, whose compact sibling
/// fallback avoids exceeding the normal 255 UTF-16 component limit while keeping the same volume.
pub fn commit_staged_file(staged: &Path, destination: &Path) -> Result<(), String> {
    validate_staged_sibling(staged, destination)?;
    let tracked_bytes = tracked_tiff_bytes(staged, destination);

    let sync_started = Instant::now();
    let sync_result = sync_staged_file(staged);
    if let Some(bytes) = tracked_bytes {
        tiff_performance::emit_phase_if_enabled(
            "tiff_commit_replace",
            TiffPerfPhase::FinalDurability,
            sync_started.elapsed(),
            Some(bytes),
        );
    }
    sync_result?;

    let publish_started = Instant::now();
    let publish_result = replace_path(staged, destination);
    if let Some(bytes) = tracked_bytes {
        tiff_performance::emit_phase_if_enabled(
            "tiff_commit_replace",
            TiffPerfPhase::AtomicPublication,
            publish_started.elapsed(),
            Some(bytes),
        );
    }
    publish_result
}

/// Atomically publish a fully written staged file only if the destination is still absent.
///
/// The durability contract is the same as [`commit_staged_file`], but this variant preserves the
/// queue/new-only invariant: if another process creates `destination` after reservation, commit
/// fails rather than replacing it. On Windows this uses `MoveFileExW` with
/// `MOVEFILE_WRITE_THROUGH` and **without** `MOVEFILE_REPLACE_EXISTING`, so the no-replace check and
/// final rename happen in one filesystem operation. The non-Windows fallback uses a same-volume
/// hard-link create boundary, removes the staging name, and fsyncs the parent directory.
pub fn commit_staged_file_if_absent(staged: &Path, destination: &Path) -> Result<(), String> {
    validate_staged_sibling(staged, destination)?;
    let tracked_bytes = tracked_tiff_bytes(staged, destination);

    let sync_started = Instant::now();
    let sync_result = sync_staged_file(staged);
    if let Some(bytes) = tracked_bytes {
        tiff_performance::emit_phase_if_enabled(
            "tiff_commit_new",
            TiffPerfPhase::FinalDurability,
            sync_started.elapsed(),
            Some(bytes),
        );
    }
    sync_result?;

    let publish_started = Instant::now();
    let publish_result = move_path_if_absent(staged, destination);
    if let Some(bytes) = tracked_bytes {
        tiff_performance::emit_phase_if_enabled(
            "tiff_commit_new",
            TiffPerfPhase::AtomicPublication,
            publish_started.elapsed(),
            Some(bytes),
        );
    }
    publish_result
}

fn tracked_tiff_bytes(staged: &Path, destination: &Path) -> Option<u64> {
    if !tiff_performance::enabled() || !is_tiff_path(destination) {
        return None;
    }
    fs::metadata(staged).ok().map(|metadata| metadata.len())
}

fn is_tiff_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("tif") || extension.eq_ignore_ascii_case("tiff")
        })
}

fn validate_staged_sibling(staged: &Path, destination: &Path) -> Result<(), String> {
    if staged.parent() != destination.parent() {
        return Err("Atomic commit requires the staged file beside its destination.".to_owned());
    }
    if !staged.is_file() {
        return Err(format!("Staged file is missing: {}", staged.display()));
    }
    Ok(())
}

fn sync_staged_file(staged: &Path) -> Result<(), String> {
    OpenOptions::new()
        .write(true)
        .open(staged)
        .and_then(|file| file.sync_all())
        .map_err(|err| format!("Cannot sync staged file {}: {err}", staged.display()))
}

#[cfg(windows)]
fn replace_path(source: &Path, destination: &Path) -> Result<(), String> {
    move_path_windows(
        source,
        destination,
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        "atomically replace",
    )
}

#[cfg(windows)]
fn move_path_if_absent(source: &Path, destination: &Path) -> Result<(), String> {
    let source_wide = wide_path(source);
    let destination_wide = wide_path(destination);
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(format!(
            "Cannot commit new destination {} because it exists or cannot be created: {}",
            destination.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn move_path_windows(
    source: &Path,
    destination: &Path,
    flags: u32,
    operation: &str,
) -> Result<(), String> {
    let source_wide = wide_path(source);
    let destination_wide = wide_path(destination);
    let result = unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), flags) };
    if result == 0 {
        return Err(format!(
            "Cannot {operation} {} with {}: {}",
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
    })?;
    sync_parent_directory(destination)
}

#[cfg(not(windows))]
fn move_path_if_absent(source: &Path, destination: &Path) -> Result<(), String> {
    fs::hard_link(source, destination).map_err(|err| {
        format!(
            "Cannot commit new destination {} because it exists or cannot be created: {err}",
            destination.display()
        )
    })?;
    if let Err(err) = fs::remove_file(source) {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "Cannot remove staging name {} after new-only commit: {err}",
            source.display()
        ));
    }
    sync_parent_directory(destination)
}

#[cfg(not(windows))]
fn sync_parent_directory(path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| format!("Cannot sync parent directory {}: {err}", parent.display()))
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
    fn atomic_new_write_preserves_an_existing_file() {
        let folder = temp_folder("new-write");
        fs::create_dir_all(&folder).unwrap();
        let target = folder.join("project.shade");
        fs::write(&target, b"existing").unwrap();

        assert!(atomic_write_if_absent(&target, b"new").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"existing");
        assert!(!temp_path(&target).exists());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn staged_file_commit_replaces_destination_atomically() {
        let folder = temp_folder("staged-commit");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        let staged = folder.join(format!("output.tif{}", staging::CONVERSION_STAGED_SUFFIX));
        fs::write(&destination, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();

        commit_staged_file(&staged, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!staged.exists());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn new_only_staged_commit_preserves_an_existing_destination() {
        let folder = temp_folder("staged-new-only");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        let staged = folder.join(format!("output.tif{}", staging::CONVERSION_STAGED_SUFFIX));
        fs::write(&destination, b"existing").unwrap();
        fs::write(&staged, b"new").unwrap();

        assert!(commit_staged_file_if_absent(&staged, &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"existing");
        assert_eq!(fs::read(&staged).unwrap(), b"new");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn new_only_staged_commit_moves_stage_when_destination_is_absent() {
        let folder = temp_folder("staged-new-only-success");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        let staged = folder.join(format!("output.tif{}", staging::CONVERSION_STAGED_SUFFIX));
        fs::write(&staged, b"new").unwrap();

        commit_staged_file_if_absent(&staged, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!staged.exists());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn tiff_tracking_is_scoped_to_tiff_destinations() {
        assert!(is_tiff_path(Path::new("output.tif")));
        assert!(is_tiff_path(Path::new("OUTPUT.TIFF")));
        assert!(!is_tiff_path(Path::new("project.shade")));
    }
}
