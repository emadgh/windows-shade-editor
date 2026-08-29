use std::fs::File;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};

use sha2::{Digest, Sha256};

use crate::model::ShadeProject;
use windows_shade_editor::file_observer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveProjectBaselineCandidate {
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
}

static ACTIVE_PROJECT_BASELINE: OnceLock<Mutex<Option<ActiveProjectBaselineCandidate>>> =
    OnceLock::new();

fn active_project_baseline() -> &'static Mutex<Option<ActiveProjectBaselineCandidate>> {
    ACTIVE_PROJECT_BASELINE.get_or_init(|| Mutex::new(None))
}

/// Invalidate the accepted Source-project identity whenever the application changes project
/// session (New/Open/Recovery). A successful Open or Save will establish a fresh exact-byte
/// baseline afterwards. Keeping this explicit prevents a previous project's accepted path from
/// authorizing an unrelated later Save As after the active project has changed.
pub(crate) fn clear_active_source_project_baseline() {
    *active_project_baseline()
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

/// Capture the exact `.shade` bytes about to be loaded. The caller must only accept
/// this candidate after its project parse/validation succeeds.
pub(crate) fn begin_active_source_project_load(
    path: &Path,
) -> Result<ActiveProjectBaselineCandidate, String> {
    capture_project_fingerprint(path)
}

/// Accept a baseline only when the file still has the exact bytes captured before
/// the successful load. This closes the load/fingerprint race without coupling the
/// active project to Project View's observer subscription lifecycle.
pub(crate) fn accept_loaded_source_project(
    path: &Path,
    candidate: ActiveProjectBaselineCandidate,
) -> Result<(), String> {
    let current = capture_project_fingerprint(path)?;
    if !same_path(&candidate.path, &current.path) || candidate != current {
        return Err(format!(
            "Project changed while it was being loaded. Reopen it before editing or saving: {}",
            path.display()
        ));
    }
    *active_project_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(current);
    Ok(())
}

fn refresh_saved_source_project_baseline(path: &Path) -> Result<(), String> {
    let current = capture_project_fingerprint(path)?;
    *active_project_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(current);
    // Project View may also be observing this path. Since this write is app-owned,
    // converge that optional UI observer to the same bytes rather than showing a
    // false external-change warning after Save.
    let _ = file_observer::acknowledge(path);
    Ok(())
}

fn expected_active_source_project_save_baseline(
    path: &Path,
) -> Result<Option<ActiveProjectBaselineCandidate>, String> {
    let baseline = active_project_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();

    if !path.exists() {
        // Missing is not automatically equivalent to a brand-new Save As target. If the active
        // accepted project owned this exact path, external deletion is a lost-update event and
        // Ctrl+S must fail closed rather than silently recreating the file. A different absent
        // path remains a legitimate new Save As / Quick Save destination.
        if baseline.as_ref().is_some_and(|accepted| {
            same_path(&accepted.path, &normalize_identity_path(path))
        }) {
            return Err(format!(
                "Project file was deleted or moved outside Shade Editor. Use Save As with a new filename or reopen/relink the project before saving: {}",
                path.display()
            ));
        }
        return Ok(None);
    }

    let current = capture_project_fingerprint(path)?;
    let Some(baseline) = baseline else {
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

#[cfg(windows)]
fn open_existing_project_write_exclusion(path: &Path) -> Result<(PathBuf, File), String> {
    let normalized = normalize_existing_path(path);
    let file = OpenOptions::new()
        .read(true)
        // Keep rename/delete sharing so Shade Editor can atomically replace the accepted file,
        // but deliberately exclude FILE_SHARE_WRITE. Windows sharing checks are symmetric: an
        // already-open writer prevents this guard from opening, and a new writer cannot open
        // while this handle remains alive through the final MoveFileExW replacement.
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

fn persist_active_source_project(
    project: &ShadeProject,
    path: &Path,
    resolved_face_paths: &[PathBuf],
    expected: Option<ActiveProjectBaselineCandidate>,
) -> Result<(), String> {
    let Some(expected) = expected else {
        // This is intentionally no-replace publication. If another process creates the chosen
        // Save As / Quick Save target after classification, atomic_write_if_absent fails closed.
        return project.save_new(path, resolved_face_paths);
    };

    #[cfg(windows)]
    {
        let (normalized, mut guard) = open_existing_project_write_exclusion(path)?;
        let current = capture_project_fingerprint_from_open_file(normalized, &mut guard)?;
        ensure_expected_project_baseline(path, &expected, &current)?;

        // Keep `guard` alive for the entire serialization/staging/backup/replace sequence. Its
        // share mode allows our atomic rename but prevents a concurrent writer from entering the
        // final verified-baseline -> replacement boundary.
        let result = project.save(path, resolved_face_paths);
        drop(guard);
        return result;
    }

    #[cfg(not(windows))]
    {
        // Shade Editor ships on Windows. Keep non-Windows development/tests fail-closed against
        // changes that happen after initial classification by revalidating immediately before the
        // existing atomic replacement boundary.
        let current = capture_project_fingerprint(path)?;
        ensure_expected_project_baseline(path, &expected, &current)?;
        project.save(path, resolved_face_paths)
    }
}

/// Verify that an existing Source `.shade` file still exactly matches the bytes
/// accepted by the last successful Open or Save.
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
    persist_active_source_project(project, path, resolved_face_paths, expected)?;
    refresh_saved_source_project_baseline(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(windows)]
    use std::fs::OpenOptions;
    #[cfg(windows)]
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::MutexGuard;

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
        clear_active_source_project_baseline();
    }

    fn accept_existing(path: &Path) {
        let candidate = begin_active_source_project_load(path).unwrap();
        let _ = fs::read(path).unwrap();
        accept_loaded_source_project(path, candidate).unwrap();
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
    fn accepted_baseline_allows_save_boundary() {
        let _serial = serial_guard();
        let path = temp_project_path("accepted");
        cleanup(&path);
        fs::write(&path, b"baseline").unwrap();
        accept_existing(&path);
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
        assert!(!path.exists(), "rejected Ctrl+S must not recreate the deleted project");

        cleanup(&path);
    }

    #[test]
    fn changed_during_load_is_not_accepted_as_the_active_baseline() {
        let _serial = serial_guard();
        let path = temp_project_path("load-race");
        cleanup(&path);
        fs::write(&path, b"version a").unwrap();
        let candidate = begin_active_source_project_load(&path).unwrap();
        fs::write(&path, b"version b with different bytes").unwrap();
        let error = accept_loaded_source_project(&path, candidate).unwrap_err();
        assert!(error.contains("changed while it was being loaded"), "{error}");
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
        assert!(error.contains("Cannot safely create new .shade file"), "{error}");
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
