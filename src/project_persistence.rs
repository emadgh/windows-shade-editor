use std::path::{Path, PathBuf};

use crate::model::ShadeProject;

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
    project.save(path, resolved_face_paths)
}
