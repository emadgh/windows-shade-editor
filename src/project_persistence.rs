use std::fs::File;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
#[cfg(not(windows))]
use std::time::SystemTime;

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_BASIC_INFO, FILE_RENAME_INFO, FILE_RENAME_INFO_0,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileBasicInfo, FileRenameInfoEx,
    GetFileInformationByHandle, GetFileInformationByHandleEx, SetFileInformationByHandle,
};

use sha2::{Digest, Sha256};

use crate::model::ShadeProject;
use windows_shade_editor::file_observer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveProjectBaselineCandidate {
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
}

/// Evidence that the file object visible at Open-transition entry remained the same object and
/// generation until the GUI accepted the loaded project session. SHA-256 proves the bytes; this
/// generation token detects the otherwise-undetectable A -> B -> A race where the path ends with
/// the original bytes after a different version was visible to the background Open worker.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenFileGeneration {
    volume_serial_number: u32,
    file_index: u64,
    creation_time: i64,
    last_write_time: i64,
    change_time: i64,
}

#[cfg(not(windows))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenFileGeneration {
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedOpenBaseline {
    candidate: ActiveProjectBaselineCandidate,
    generation: OpenFileGeneration,
}

#[derive(Clone, Debug)]
struct OpenBaselineFailure {
    path: PathBuf,
    message: String,
}

static ACTIVE_PROJECT_BASELINE: OnceLock<Mutex<Option<ActiveProjectBaselineCandidate>>> =
    OnceLock::new();
static PREPARED_OPEN_BASELINE: OnceLock<Mutex<Option<PreparedOpenBaseline>>> = OnceLock::new();
static OPEN_BASELINE_FAILURE: OnceLock<Mutex<Option<OpenBaselineFailure>>> = OnceLock::new();

fn active_project_baseline() -> &'static Mutex<Option<ActiveProjectBaselineCandidate>> {
    ACTIVE_PROJECT_BASELINE.get_or_init(|| Mutex::new(None))
}

fn prepared_open_baseline() -> &'static Mutex<Option<PreparedOpenBaseline>> {
    PREPARED_OPEN_BASELINE.get_or_init(|| Mutex::new(None))
}

fn open_baseline_failure() -> &'static Mutex<Option<OpenBaselineFailure>> {
    OPEN_BASELINE_FAILURE.get_or_init(|| Mutex::new(None))
}

/// Invalidate only the accepted Source-project identity. Pending Open evidence has its own
/// lifecycle because a successful Open calls the session bump before Project View/history work.
pub(crate) fn clear_active_source_project_baseline() {
    *active_project_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Disarm any not-yet-accepted Open evidence. New/Recovery/Exit use this so an earlier cancelled
/// or failed Open can never authorize a later unrelated project session.
pub(crate) fn disarm_prepared_active_source_project_open() {
    *prepared_open_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *open_baseline_failure()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn normalize_existing_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Produce a stable identity path even when the final file is currently missing. Canonicalizing
/// the parent keeps an active baseline comparable after external deletion while still resolving
/// normal existing files through the filesystem first.
fn normalize_identity_path(path: &Path) -> PathBuf {
    if let Ok(normalized) = std::fs::canonicalize(path) {
        return normalized;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if let (Ok(parent), Some(name)) = (std::fs::canonicalize(parent), path.file_name()) {
        return parent.join(name);
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn capture_project_fingerprint_from_open_file(
    normalized: PathBuf,
    file: &mut File,
) -> Result<ActiveProjectBaselineCandidate, String> {
    let metadata = file
        .metadata()
        .map_err(|err| format!("Cannot inspect project file {}: {err}", normalized.display()))?;
    if !metadata.is_file() {
        return Err(format!("Project path is not a file: {}", normalized.display()));
    }

    file.seek(SeekFrom::Start(0))
        .map_err(|err| format!("Cannot seek project file {}: {err}", normalized.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("Cannot hash project file {}: {err}", normalized.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(ActiveProjectBaselineCandidate {
        path: normalized,
        size_bytes: metadata.len(),
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn capture_project_fingerprint(path: &Path) -> Result<ActiveProjectBaselineCandidate, String> {
    let normalized = normalize_existing_path(path);
    let mut file = File::open(&normalized)
        .map_err(|err| format!("Cannot open project file {}: {err}", normalized.display()))?;
    capture_project_fingerprint_from_open_file(normalized, &mut file)
}

#[cfg(windows)]
fn capture_open_file_generation(file: &File, path: &Path) -> Result<OpenFileGeneration, String> {
    let handle = file.as_raw_handle() as _;
    let mut identity = BY_HANDLE_FILE_INFORMATION::default();
    let identity_ok = unsafe { GetFileInformationByHandle(handle, &raw mut identity) };
    if identity_ok == 0 {
        return Err(format!(
            "Cannot identify project file generation {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }

    let mut basic = FILE_BASIC_INFO::default();
    let basic_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            std::ptr::addr_of_mut!(basic).cast(),
            u32::try_from(std::mem::size_of::<FILE_BASIC_INFO>())
                .map_err(|_| "Project file generation buffer is too large.".to_owned())?,
        )
    };
    if basic_ok == 0 {
        return Err(format!(
            "Cannot inspect project file generation {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }

    Ok(OpenFileGeneration {
        volume_serial_number: identity.dwVolumeSerialNumber,
        file_index: (u64::from(identity.nFileIndexHigh) << 32) | u64::from(identity.nFileIndexLow),
        creation_time: basic.CreationTime,
        last_write_time: basic.LastWriteTime,
        change_time: basic.ChangeTime,
    })
}

#[cfg(not(windows))]
fn capture_open_file_generation(file: &File, path: &Path) -> Result<OpenFileGeneration, String> {
    let metadata = file
        .metadata()
        .map_err(|err| format!("Cannot inspect project file generation {}: {err}", path.display()))?;
    Ok(OpenFileGeneration {
        created: metadata.created().ok(),
        modified: metadata.modified().ok(),
    })
}

fn capture_prepared_open_baseline(path: &Path) -> Result<PreparedOpenBaseline, String> {
    let normalized = normalize_existing_path(path);
    let mut file = File::open(&normalized)
        .map_err(|err| format!("Cannot open project file {}: {err}", normalized.display()))?;
    let generation = capture_open_file_generation(&file, &normalized)?;
    let candidate = capture_project_fingerprint_from_open_file(normalized, &mut file)?;
    Ok(PreparedOpenBaseline {
        candidate,
        generation,
    })
}

fn set_open_baseline_failure(path: PathBuf, message: String) {
    *open_baseline_failure()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(OpenBaselineFailure { path, message });
}

/// Capture immutable evidence before the Open worker reads the `.shade` project. Lifecycle owns
/// this operation; Project View/history is deliberately excluded from this authority boundary.
pub(crate) fn prepare_active_source_project_open(path: &Path) -> Result<(), String> {
    match capture_prepared_open_baseline(path) {
        Ok(prepared) => {
            *prepared_open_baseline()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prepared);
            *open_baseline_failure()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            Ok(())
        }
        Err(err) => {
            *prepared_open_baseline()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            set_open_baseline_failure(
                normalize_identity_path(path),
                format!(
                    "Shade Editor could not establish exact-byte authority before opening this project. Same-path Save is blocked until the project is reopened or saved to a new filename: {err}"
                ),
            );
            Err(err)
        }
    }
}

/// Rotate project-session authority. New/Recovery call this with no prepared Open and therefore
/// only disarm the previous baseline. A successful Open has a prepared candidate; it is accepted
/// only if both exact bytes and the underlying file generation still match after the background
/// Open/preview load completes.
pub(crate) fn rotate_active_source_project_session() -> Result<(), String> {
    clear_active_source_project_baseline();
    let prepared = prepared_open_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let Some(prepared) = prepared else {
        return Ok(());
    };

    let expected_path = prepared.candidate.path.clone();
    match capture_prepared_open_baseline(&expected_path) {
        Ok(current) if current == prepared => {
            *active_project_baseline()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(current.candidate);
            *open_baseline_failure()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            Ok(())
        }
        Ok(_) => {
            let message = format!(
                "Project changed while it was opening. The loaded editor state is not authorized to overwrite the current file; reopen it or use Save As: {}",
                expected_path.display()
            );
            set_open_baseline_failure(expected_path, message.clone());
            Err(message)
        }
        Err(err) => {
            let message = format!(
                "Project became unavailable while it was opening. The loaded editor state is not authorized to recreate or overwrite it; reopen it or use Save As: {} ({err})",
                expected_path.display()
            );
            set_open_baseline_failure(expected_path, message.clone());
            Err(message)
        }
    }
}

/// Legacy Project View hook retained until its cache code is simplified. Active project
/// authority is lifecycle-owned now, so history/indexing is never allowed to mint a baseline.
pub(crate) fn begin_active_source_project_load(
    _path: &Path,
) -> Result<ActiveProjectBaselineCandidate, String> {
    Err("Active .shade Open baselines are lifecycle-owned; Project View/history cannot capture save authority.".to_owned())
}

/// Legacy Project View hook retained as a fail-closed compatibility shim. Only lifecycle Open and
/// the explicit Save boundary can establish active same-path overwrite authority.
pub(crate) fn accept_loaded_source_project(
    _path: &Path,
    _candidate: ActiveProjectBaselineCandidate,
) -> Result<(), String> {
    Err("Project View/history cannot establish active .shade save authority.".to_owned())
}

/// Accept only the exact bytes staged by the successful Save. Re-reading the path and accepting
/// whatever happens to be there would let an external writer race immediately after publication
/// and accidentally become the new trusted baseline.
fn accept_saved_source_project(
    path: &Path,
    committed: ActiveProjectBaselineCandidate,
) -> Result<(), String> {
    let current = capture_project_fingerprint(path)?;
    if !same_path(&committed.path, &current.path) || committed != current {
        clear_active_source_project_baseline();
        return Err(format!(
            "Project was saved, but the file changed again outside Shade Editor before the saved bytes could be accepted. Local edits remain protected; reopen/inspect the project before saving again: {}",
            path.display()
        ));
    }
    *active_project_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(current);
    *open_baseline_failure()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    // Project View may also be observing this path. Since these exact bytes were app-owned and
    // reverified, converge that optional UI observer rather than showing a false external-change
    // warning after Save.
    let _ = file_observer::acknowledge(path);
    Ok(())
}

fn active_open_failure_for(path: &Path) -> Option<String> {
    let identity = normalize_identity_path(path);
    open_baseline_failure()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|failure| same_path(&failure.path, &identity))
        .map(|failure| failure.message.clone())
}

fn expected_active_source_project_save_baseline(
    path: &Path,
) -> Result<Option<ActiveProjectBaselineCandidate>, String> {
    let baseline = active_project_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let open_failure = active_open_failure_for(path);

    if !path.exists() {
        if let Some(message) = open_failure {
            return Err(message);
        }
        // Missing is not automatically equivalent to a brand-new Save As target. If the active
        // accepted project owned this exact path, external deletion is a lost-update event and
        // Ctrl+S must fail closed rather than silently recreating the file. A different absent
        // path remains a legitimate new Save As / Quick Save destination.
        if baseline
            .as_ref()
            .is_some_and(|accepted| same_path(&accepted.path, &normalize_identity_path(path)))
        {
            return Err(format!(
                "Project file was deleted or moved outside Shade Editor. Use Save As with a new filename or reopen/relink the project before saving: {}",
                path.display()
            ));
        }
        return Ok(None);
    }

    let current = capture_project_fingerprint(path)?;
    let Some(baseline) = baseline else {
        if let Some(message) = open_failure {
            return Err(message);
        }
        return Err(format!(
            "Project overwrite blocked because Shade Editor has no accepted baseline for the existing file. Reopen/inspect it or use Save As with a new filename: {}",
            path.display()
        ));
    };
    if !same_path(&baseline.path, &current.path) {
        return Err(format!(
            "Project overwrite blocked because the existing Save target is not the active accepted project. Choose a new Save As filename or reopen that project first: {}",
            path.display()
        ));
    }
    if baseline != current {
        return Err(format!(
            "Project changed outside Shade Editor. Reload/inspect the external version or use Save As before overwriting: {}",
            path.display()
        ));
    }
    Ok(Some(baseline))
}

fn ensure_expected_project_baseline(
    path: &Path,
    expected: &ActiveProjectBaselineCandidate,
    current: &ActiveProjectBaselineCandidate,
) -> Result<(), String> {
    if !same_path(&expected.path, &current.path) || expected != current {
        return Err(format!(
            "Project changed outside Shade Editor before Save could be committed. Reload/inspect the external version or use Save As before overwriting: {}",
            path.display()
        ));
    }
    Ok(())
}

fn stage_active_source_project(
    project: &ShadeProject,
    path: &Path,
    resolved_face_paths: &[PathBuf],
) -> Result<(PathBuf, ActiveProjectBaselineCandidate), String> {
    let staged = crate::safe_fs::temp_path(path);
    if staged.exists() {
        std::fs::remove_file(&staged).map_err(|err| {
            format!(
                "Cannot remove stale project staging file {}: {err}",
                staged.display()
            )
        })?;
    }

    if let Err(err) = project.save_new(&staged, resolved_face_paths) {
        let _ = std::fs::remove_file(&staged);
        return Err(err);
    }

    let mut committed = match capture_project_fingerprint(&staged) {
        Ok(candidate) => candidate,
        Err(err) => {
            let _ = std::fs::remove_file(&staged);
            return Err(err);
        }
    };
    committed.path = normalize_identity_path(path);
    Ok((staged, committed))
}

#[cfg(windows)]
fn open_existing_project_write_exclusion(path: &Path) -> Result<(PathBuf, File), String> {
    let normalized = normalize_existing_path(path);
    let file = OpenOptions::new()
        .read(true)
        // Keep rename/delete sharing so a POSIX-semantics rename can atomically publish our staged
        // file, but deliberately exclude FILE_SHARE_WRITE. Windows sharing checks are symmetric:
        // an already-open writer prevents this guard from opening, and a new writer cannot open
        // this file object while the verified baseline remains guarded.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(&normalized)
        .map_err(|err| {
            format!(
                "Project overwrite blocked because the accepted file cannot be exclusively guarded against concurrent writers: {}: {err}",
                path.display()
            )
        })?;
    Ok((normalized, file))
}

#[cfg(windows)]
fn windows_file_rename_destination_wide(destination: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    const VERBATIM_PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const VERBATIM_UNC_PREFIX: &[u16] = &[
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];

    let wide = normalize_identity_path(destination)
        .as_os_str()
        .encode_wide()
        .collect::<Vec<_>>();
    if let Some(rest) = wide.strip_prefix(VERBATIM_UNC_PREFIX) {
        let mut dos_unc = Vec::with_capacity(rest.len() + 2);
        dos_unc.extend_from_slice(&[b'\\' as u16, b'\\' as u16]);
        dos_unc.extend_from_slice(rest);
        dos_unc
    } else if let Some(rest) = wide.strip_prefix(VERBATIM_PREFIX) {
        rest.to_vec()
    } else {
        wide
    }
}

#[cfg(windows)]
fn replace_staged_project_while_guarded(staged: &Path, destination: &Path) -> Result<(), String> {
    // MoveFileExW cannot replace a destination while that destination remains open, even when the
    // handle grants FILE_SHARE_DELETE. FileRenameInfoEx with POSIX semantics is the Windows 10+
    // primitive specifically intended to replace a name while existing delete-sharing handles
    // remain valid. The guard therefore stays live through the exact rename boundary.
    const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 0x0000_0001;
    const FILE_RENAME_FLAG_POSIX_SEMANTICS: u32 = 0x0000_0002;

    let destination_name = windows_file_rename_destination_wide(destination);
    let file_name_bytes = destination_name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| "Project destination path is too long to rename safely.".to_owned())?;
    let header_bytes = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let buffer_bytes = header_bytes
        .checked_add(file_name_bytes)
        .ok_or_else(|| "Project rename buffer size overflow.".to_owned())?;
    let word_bytes = std::mem::size_of::<usize>();
    let word_count = buffer_bytes.div_ceil(word_bytes);
    let mut storage = vec![0usize; word_count];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();

    unsafe {
        (*info).Anonymous = FILE_RENAME_INFO_0 {
            Flags: FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS,
        };
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = u32::try_from(file_name_bytes)
            .map_err(|_| "Project destination path is too long to rename safely.".to_owned())?;
        std::ptr::copy_nonoverlapping(
            destination_name.as_ptr(),
            (*info).FileName.as_mut_ptr(),
            destination_name.len(),
        );
    }

    let staged_file = OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(staged)
        .map_err(|err| {
            format!(
                "Cannot open staged project for atomic publication {}: {err}",
                staged.display()
            )
        })?;

    let renamed = unsafe {
        SetFileInformationByHandle(
            staged_file.as_raw_handle() as _,
            FileRenameInfoEx,
            info.cast(),
            u32::try_from(buffer_bytes)
                .map_err(|_| "Project rename buffer is too large.".to_owned())?,
        )
    };
    if renamed == 0 {
        return Err(format!(
            "Cannot atomically publish guarded .shade file {} from {}: {}",
            destination.display(),
            staged.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn persist_active_source_project(
    project: &ShadeProject,
    path: &Path,
    resolved_face_paths: &[PathBuf],
    expected: Option<ActiveProjectBaselineCandidate>,
) -> Result<ActiveProjectBaselineCandidate, String> {
    // Serialize once into a same-directory staged file, then identify those exact bytes. The
    // returned candidate is the only identity that Save is allowed to accept after publication.
    let (staged, committed) = stage_active_source_project(project, path, resolved_face_paths)?;

    let publish_result = if let Some(expected) = expected {
        #[cfg(windows)]
        {
            (|| -> Result<(), String> {
                let (normalized, mut guard) = open_existing_project_write_exclusion(path)?;
                let current = capture_project_fingerprint_from_open_file(normalized, &mut guard)?;
                ensure_expected_project_baseline(path, &expected, &current)?;

                // Preserve the existing `.shade.bak` contract while the verified destination is
                // still write-excluded. The backup therefore captures the same accepted bytes we
                // just revalidated, never a later external write.
                let backup = crate::safe_fs::backup_path(path);
                crate::safe_fs::atomic_copy(path, &backup)?;

                // FileRenameInfoEx + POSIX semantics can replace the visible name while `guard`
                // remains open with FILE_SHARE_DELETE. This closes the final verify -> publication
                // TOCTOU window without weakening writer exclusion.
                replace_staged_project_while_guarded(&staged, path)?;
                drop(guard);
                Ok(())
            })()
        }

        #[cfg(not(windows))]
        {
            let current = capture_project_fingerprint(path)?;
            ensure_expected_project_baseline(path, &expected, &current)?;
            let backup = crate::safe_fs::backup_path(path);
            crate::safe_fs::atomic_copy(path, &backup)?;
            crate::safe_fs::commit_staged_file(&staged, path)
        }
    } else {
        // This is intentionally no-replace publication. If another process creates the chosen
        // Save As / Quick Save target after classification or while serialization is running,
        // commit_staged_file_if_absent fails closed and preserves that external file.
        crate::safe_fs::commit_staged_file_if_absent(&staged, path)
    };

    if let Err(err) = publish_result {
        if staged.exists() {
            let _ = std::fs::remove_file(&staged);
        }
        return Err(err);
    }
    Ok(committed)
}

/// Verify that an existing Source `.shade` file still exactly matches the bytes
/// accepted by the last successful lifecycle Open or explicit Save.
pub fn verify_active_source_project_save_baseline(path: &Path) -> Result<(), String> {
    expected_active_source_project_save_baseline(path).map(|_| ())
}

/// The only write boundary for the currently opened Source `.shade` project.
///
/// Callers must reach this function only from an explicit Save / Save As /
/// Quick Save / Save-and-continue user action. Crash recovery, application
/// settings, queues, Project View caches and generated Production projects use
/// separate persistence domains and must never route through this boundary.
pub fn save_active_source_project(
    project: &ShadeProject,
    path: &Path,
    resolved_face_paths: &[PathBuf],
) -> Result<(), String> {
    let expected = expected_active_source_project_save_baseline(path)?;
    let committed = persist_active_source_project(project, path, resolved_face_paths, expected)?;
    accept_saved_source_project(path, committed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(windows)]
    use std::fs::OpenOptions;
    #[cfg(windows)]
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::MutexGuard;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);
    static TEST_BASELINE_LOCK: Mutex<()> = Mutex::new(());

    fn serial_guard() -> MutexGuard<'static, ()> {
        TEST_BASELINE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn temp_project_path(label: &str) -> PathBuf {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "shade-project-save-guard-{label}-{}-{id}.shade",
            std::process::id()
        ))
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_file(path);
        let backup = path.with_extension("shade.bak");
        let _ = fs::remove_file(backup);
        let _ = fs::remove_file(crate::safe_fs::temp_path(path));
        clear_active_source_project_baseline();
        disarm_prepared_active_source_project_open();
    }

    fn accept_existing(path: &Path) {
        prepare_active_source_project_open(path).unwrap();
        rotate_active_source_project_session().unwrap();
    }

    #[test]
    fn tracked_external_change_blocks_same_path_save_boundary() {
        let _serial = serial_guard();
        let path = temp_project_path("changed");
        cleanup(&path);
        fs::write(&path, b"baseline").unwrap();
        accept_existing(&path);

        fs::write(&path, b"externally changed and longer").unwrap();
        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("changed outside Shade Editor"));

        cleanup(&path);
    }

    #[test]
    fn unchanged_lifecycle_open_accepts_exact_baseline() {
        let _serial = serial_guard();
        let path = temp_project_path("accepted-open");
        cleanup(&path);
        fs::write(&path, b"baseline").unwrap();
        prepare_active_source_project_open(&path).unwrap();
        let _loaded_bytes = fs::read(&path).unwrap();
        rotate_active_source_project_session().unwrap();
        assert!(verify_active_source_project_save_baseline(&path).is_ok());
        cleanup(&path);
    }

    #[test]
    fn clearing_project_session_baseline_disarms_previous_existing_path() {
        let _serial = serial_guard();
        let path = temp_project_path("clear-session");
        cleanup(&path);
        fs::write(&path, b"accepted previous project").unwrap();
        accept_existing(&path);
        assert!(verify_active_source_project_save_baseline(&path).is_ok());

        clear_active_source_project_baseline();
        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("no accepted baseline"), "{error}");

        cleanup(&path);
    }

    #[test]
    fn deleted_active_project_is_not_reclassified_as_a_new_save_target() {
        let _serial = serial_guard();
        let path = temp_project_path("deleted-active");
        cleanup(&path);
        fs::write(&path, b"accepted active project").unwrap();
        accept_existing(&path);
        fs::remove_file(&path).unwrap();

        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("deleted or moved outside Shade Editor"), "{error}");

        let mut project = ShadeProject::default();
        project.name = "must not recreate deleted active project".to_owned();
        let error = save_active_source_project(&project, &path, &[]).unwrap_err();
        assert!(error.contains("deleted or moved outside Shade Editor"), "{error}");
        assert!(
            !path.exists(),
            "rejected Ctrl+S must not recreate the deleted project"
        );

        cleanup(&path);
    }

    #[test]
    fn changed_during_open_is_not_accepted_as_active_baseline() {
        let _serial = serial_guard();
        let path = temp_project_path("open-race");
        cleanup(&path);
        fs::write(&path, b"version a").unwrap();
        prepare_active_source_project_open(&path).unwrap();
        let _loaded_a = fs::read(&path).unwrap();
        fs::write(&path, b"version b with different bytes").unwrap();

        let error = rotate_active_source_project_session().unwrap_err();
        assert!(error.contains("changed while it was opening"), "{error}");
        let save_error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(save_error.contains("changed while it was opening"), "{save_error}");
        cleanup(&path);
    }

    #[cfg(windows)]
    #[test]
    fn windows_open_generation_detects_a_to_b_to_a_rewrite() {
        let _serial = serial_guard();
        let path = temp_project_path("open-generation-race");
        cleanup(&path);
        let a = b"version a exact bytes";
        fs::write(&path, a).unwrap();
        prepare_active_source_project_open(&path).unwrap();
        let _loaded_a = fs::read(&path).unwrap();
        fs::write(&path, b"temporary external version b").unwrap();
        fs::write(&path, a).unwrap();

        let error = rotate_active_source_project_session().unwrap_err();
        assert!(error.contains("changed while it was opening"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), a);
        cleanup(&path);
    }

    #[test]
    fn deletion_during_open_blocks_same_path_recreation() {
        let _serial = serial_guard();
        let path = temp_project_path("open-delete-race");
        cleanup(&path);
        fs::write(&path, b"version a").unwrap();
        prepare_active_source_project_open(&path).unwrap();
        let _loaded_a = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let error = rotate_active_source_project_session().unwrap_err();
        assert!(error.contains("became unavailable while it was opening"), "{error}");

        let project = ShadeProject::default();
        let error = save_active_source_project(&project, &path, &[]).unwrap_err();
        assert!(error.contains("not authorized to recreate"), "{error}");
        assert!(!path.exists());
        cleanup(&path);
    }

    #[test]
    fn project_view_history_hooks_cannot_arm_save_authority() {
        let _serial = serial_guard();
        let path = temp_project_path("history-no-authority");
        cleanup(&path);
        fs::write(&path, b"history bytes").unwrap();
        assert!(begin_active_source_project_load(&path).is_err());
        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("no accepted baseline"), "{error}");
        cleanup(&path);
    }

    #[test]
    fn untracked_existing_target_fails_closed() {
        let _serial = serial_guard();
        let path = temp_project_path("untracked-existing");
        cleanup(&path);
        fs::write(&path, b"external project").unwrap();
        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("no accepted baseline"), "{error}");
        cleanup(&path);
    }

    #[test]
    fn existing_different_save_as_target_is_not_silently_overwritten() {
        let _serial = serial_guard();
        let active = temp_project_path("active");
        let other = temp_project_path("other");
        cleanup(&active);
        let _ = fs::remove_file(&other);
        fs::write(&active, b"active baseline").unwrap();
        fs::write(&other, b"someone else's project").unwrap();
        accept_existing(&active);
        let error = verify_active_source_project_save_baseline(&other).unwrap_err();
        assert!(error.contains("not the active accepted project"), "{error}");
        cleanup(&active);
        let _ = fs::remove_file(other);
    }

    #[test]
    fn untracked_new_save_as_target_remains_allowed() {
        let _serial = serial_guard();
        let path = temp_project_path("save-as-new");
        cleanup(&path);
        assert!(verify_active_source_project_save_baseline(&path).is_ok());
        cleanup(&path);
    }

    #[test]
    fn target_created_after_new_save_classification_is_preserved_at_publication() {
        let _serial = serial_guard();
        let path = temp_project_path("new-target-race");
        cleanup(&path);
        let expected = expected_active_source_project_save_baseline(&path).unwrap();
        assert!(expected.is_none());

        let external_bytes = b"created by another process after Save As classification";
        fs::write(&path, external_bytes).unwrap();
        let mut project = ShadeProject::default();
        project.name = "local project".to_owned();
        let error = persist_active_source_project(&project, &path, &[], expected).unwrap_err();
        assert!(
            error.contains("Cannot commit new destination"),
            "unexpected error: {error}"
        );
        assert_eq!(fs::read(&path).unwrap(), external_bytes);

        cleanup(&path);
    }

    #[test]
    fn accepted_target_changed_after_initial_classification_is_preserved_at_publication() {
        let _serial = serial_guard();
        let path = temp_project_path("existing-target-race");
        cleanup(&path);
        fs::write(&path, b"accepted bytes").unwrap();
        accept_existing(&path);
        let expected = expected_active_source_project_save_baseline(&path).unwrap();
        assert!(expected.is_some());

        let external_bytes = b"external bytes written after initial save classification";
        fs::write(&path, external_bytes).unwrap();
        let mut project = ShadeProject::default();
        project.name = "local unsaved edit".to_owned();
        let error = persist_active_source_project(&project, &path, &[], expected).unwrap_err();
        assert!(error.contains("changed outside Shade Editor"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), external_bytes);

        cleanup(&path);
    }

    #[test]
    fn bytes_changed_immediately_after_publication_are_not_accepted_as_saved_baseline() {
        let _serial = serial_guard();
        let path = temp_project_path("post-publication-race");
        cleanup(&path);
        let mut project = ShadeProject::default();
        project.name = "committed local bytes".to_owned();
        let expected = expected_active_source_project_save_baseline(&path).unwrap();
        let committed = persist_active_source_project(&project, &path, &[], expected).unwrap();

        let external_bytes = b"external writer changed the just-published project";
        fs::write(&path, external_bytes).unwrap();
        let error = accept_saved_source_project(&path, committed).unwrap_err();
        assert!(error.contains("changed again outside Shade Editor"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), external_bytes);
        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("no accepted baseline"), "{error}");

        cleanup(&path);
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_rename_info_uses_dos_or_unc_absolute_paths() {
        use std::path::Path;

        let drive = windows_file_rename_destination_wide(Path::new(r"\\?\C:\factory\project.shade"));
        assert_eq!(String::from_utf16(&drive).unwrap(), r"C:\factory\project.shade");

        let unc = windows_file_rename_destination_wide(Path::new(
            r"\\?\UNC\server\share\project.shade",
        ));
        assert_eq!(
            String::from_utf16(&unc).unwrap(),
            r"\\server\share\project.shade"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_target_guard_blocks_new_concurrent_writer() {
        let _serial = serial_guard();
        let path = temp_project_path("windows-write-exclusion");
        cleanup(&path);
        fs::write(&path, b"accepted bytes").unwrap();

        let (_normalized, guard) = open_existing_project_write_exclusion(&path).unwrap();
        let writer = OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .open(&path);
        assert!(
            writer.is_err(),
            "a writer must not enter while the final accepted-project guard is held"
        );
        drop(guard);

        cleanup(&path);
    }

    #[test]
    fn successful_save_establishes_and_refreshes_baseline_for_next_save() {
        let _serial = serial_guard();
        let path = temp_project_path("refresh");
        cleanup(&path);
        let mut project = ShadeProject::default();
        project.name = "first".to_owned();
        save_active_source_project(&project, &path, &[]).unwrap();
        assert!(verify_active_source_project_save_baseline(&path).is_ok());

        project.name = "second".to_owned();
        save_active_source_project(&project, &path, &[]).unwrap();
        assert!(verify_active_source_project_save_baseline(&path).is_ok());
        let backup = ShadeProject::load(&crate::safe_fs::backup_path(&path)).unwrap();
        assert_eq!(backup.name, "first");

        cleanup(&path);
    }

    #[test]
    fn external_write_after_accepted_save_is_preserved_when_next_save_is_rejected() {
        let _serial = serial_guard();
        let path = temp_project_path("preserve-external");
        cleanup(&path);
        let mut project = ShadeProject::default();
        project.name = "accepted".to_owned();
        save_active_source_project(&project, &path, &[]).unwrap();

        let external_bytes = b"external writer owns these bytes";
        fs::write(&path, external_bytes).unwrap();
        project.name = "local unsaved edit".to_owned();
        let error = save_active_source_project(&project, &path, &[]).unwrap_err();
        assert!(error.contains("changed outside Shade Editor"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), external_bytes);

        cleanup(&path);
    }
}
