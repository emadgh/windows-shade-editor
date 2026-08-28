use std::path::{Path, PathBuf};

use crate::model::ShadeProject;
use windows_shade_editor::file_observer;

/// Verify that a tracked active Source project still matches the baseline accepted by
/// the File Observer. This is deliberately a persistence-boundary check rather than a
/// UI-only warning: every same-path Save path that reaches `save_active_source_project`
/// must fail closed after an unacknowledged external change.
pub fn verify_active_source_project_save_baseline(path: &Path) -> Result<(), String> {
    let Some(snapshot) = file_observer::rescan(path) else {
        // The open-project lifecycle establishes the observer baseline. Keep this
        // function backward-compatible for untracked Save As targets; the caller that
        // owns the active path is responsible for registering that baseline on open.
        return Ok(());
    };

    if snapshot.is_changed() {
        return Err(format!(
            "Project changed outside Shade Editor. Reload/inspect the external version or use Save As before overwriting: {}",
            path.display()
        ));
    }
    if !snapshot.is_available() {
        let detail = snapshot
            .last_error
            .as_deref()
            .map(|error| format!(" ({error})"))
            .unwrap_or_default();
        return Err(format!(
            "Project cannot be safely overwritten because its on-disk source is unavailable{detail}: {}",
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

    // A successful app-owned write becomes the new accepted baseline. A pending
    // native filesystem notification then converges to the same fingerprint instead
    // of being mistaken for an external conflict on the next Ctrl+S.
    let _ = file_observer::acknowledge(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows_shade_editor::file_observer::ExternalFileRole;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_project_path(label: &str) -> PathBuf {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "shade-project-save-guard-{label}-{}-{id}.shade",
            std::process::id()
        ))
    }

    #[test]
    fn tracked_external_change_blocks_same_path_save_boundary() {
        let path = temp_project_path("changed");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"baseline").unwrap();
        file_observer::observe(&path, ExternalFileRole::Project);

        fs::write(&path, b"externally changed and longer").unwrap();
        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("changed outside Shade Editor"));

        file_observer::release(&path, ExternalFileRole::Project);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn accepted_tracked_baseline_allows_save_boundary() {
        let path = temp_project_path("accepted");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"baseline").unwrap();
        file_observer::observe(&path, ExternalFileRole::Project);

        assert!(verify_active_source_project_save_baseline(&path).is_ok());

        file_observer::release(&path, ExternalFileRole::Project);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn untracked_save_as_target_remains_allowed_by_baseline_guard() {
        let path = temp_project_path("save-as");
        let _ = fs::remove_file(&path);
        assert!(verify_active_source_project_save_baseline(&path).is_ok());
    }
}
