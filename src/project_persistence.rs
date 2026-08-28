use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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

fn normalize_existing_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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

fn capture_project_fingerprint(path: &Path) -> Result<ActiveProjectBaselineCandidate, String> {
    let normalized = normalize_existing_path(path);
    let metadata = std::fs::metadata(&normalized)
        .map_err(|err| format!("Cannot inspect project file {}: {err}", normalized.display()))?;
    if !metadata.is_file() {
        return Err(format!("Project path is not a file: {}", normalized.display()));
    }

    let mut file = File::open(&normalized)
        .map_err(|err| format!("Cannot open project file {}: {err}", normalized.display()))?;
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

/// Verify that an existing Source `.shade` file still exactly matches the bytes
/// accepted by the last successful Open or Save.
pub fn verify_active_source_project_save_baseline(path: &Path) -> Result<(), String> {
    if !path.exists() {
        // A brand-new Save As target has no previous bytes to protect.
        return Ok(());
    }

    let current = capture_project_fingerprint(path)?;
    let baseline = active_project_baseline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
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
    Ok(())
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
    verify_active_source_project_save_baseline(path)?;
    project.save(path, resolved_face_paths)?;
    refresh_saved_source_project_baseline(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

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
        *active_project_baseline()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn accept_existing(path: &Path) {
        let candidate = begin_active_source_project_load(path).unwrap();
        let _ = fs::read(path).unwrap();
        accept_loaded_source_project(path, candidate).unwrap();
    }

    #[test]
    fn tracked_external_change_blocks_same_path_save_boundary() {
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
        let path = temp_project_path("accepted");
        cleanup(&path);
        fs::write(&path, b"baseline").unwrap();
        accept_existing(&path);
        assert!(verify_active_source_project_save_baseline(&path).is_ok());
        cleanup(&path);
    }

    #[test]
    fn changed_during_load_is_not_accepted_as_the_active_baseline() {
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
        let path = temp_project_path("untracked-existing");
        cleanup(&path);
        fs::write(&path, b"external project").unwrap();
        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("no accepted baseline"), "{error}");
        cleanup(&path);
    }

    #[test]
    fn existing_different_save_as_target_is_not_silently_overwritten() {
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
        let path = temp_project_path("save-as-new");
        cleanup(&path);
        assert!(verify_active_source_project_save_baseline(&path).is_ok());
        cleanup(&path);
    }

    #[test]
    fn successful_save_establishes_and_refreshes_baseline_for_next_save() {
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
