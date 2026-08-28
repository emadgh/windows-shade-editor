use std::path::{Path, PathBuf};

use crate::model::ShadeProject;
use windows_shade_editor::file_observer::{self, ExternalFileRole};

/// Verify that an existing Source `.shade` file still matches the filesystem
/// baseline accepted by Shade Editor.
///
/// Existing-but-untracked targets fail closed. This intentionally makes a narrow
/// Open/Save race safe and prevents a recovered/unknown project path from being
/// overwritten without an accepted baseline. New Save As targets are still allowed.
pub fn verify_active_source_project_save_baseline(path: &Path) -> Result<(), String> {
    let Some(snapshot) = file_observer::rescan(path) else {
        if path.exists() {
            return Err(format!(
                "Project overwrite blocked because Shade Editor has no accepted baseline for the existing file. Reopen/inspect it or use Save As with a new filename: {}",
                path.display()
            ));
        }
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

    // A successful app-owned write becomes an accepted baseline immediately,
    // including first Save / Save As targets that were not observed before the
    // write. This also closes the same-frame Save-after-Save window before the
    // Recent Projects UI gets a chance to observe the new path.
    file_observer::observe(path, ExternalFileRole::Project);
    let _ = file_observer::acknowledge(path);
    Ok(())
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
        file_observer::release(path, ExternalFileRole::Project);
        let _ = fs::remove_file(path);
        let backup = path.with_extension("shade.bak");
        let _ = fs::remove_file(backup);
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

        cleanup(&path);
    }

    #[test]
    fn accepted_tracked_baseline_allows_save_boundary() {
        let path = temp_project_path("accepted");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"baseline").unwrap();
        file_observer::observe(&path, ExternalFileRole::Project);

        assert!(verify_active_source_project_save_baseline(&path).is_ok());

        cleanup(&path);
    }

    #[test]
    fn untracked_existing_target_fails_closed() {
        let path = temp_project_path("untracked-existing");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"external project").unwrap();
        let error = verify_active_source_project_save_baseline(&path).unwrap_err();
        assert!(error.contains("no accepted baseline"), "{error}");
        cleanup(&path);
    }

    #[test]
    fn untracked_new_save_as_target_remains_allowed() {
        let path = temp_project_path("save-as-new");
        let _ = fs::remove_file(&path);
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
