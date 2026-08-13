use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::ShadeProject;

const RECOVERY_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryFile {
    pub format_version: u32,
    pub saved_at_unix_ms: i64,
    pub origin_project_path: Option<String>,
    pub face_paths: Vec<String>,
    pub project: ShadeProject,
}

impl RecoveryFile {
    pub fn new(
        project: ShadeProject,
        face_paths: Vec<PathBuf>,
        origin_project_path: Option<PathBuf>,
    ) -> Self {
        Self {
            format_version: RECOVERY_FORMAT_VERSION,
            saved_at_unix_ms: unix_ms_now(),
            origin_project_path: origin_project_path
                .map(|path| path.to_string_lossy().into_owned()),
            face_paths: face_paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            project,
        }
    }

    pub fn origin_path(&self) -> Option<PathBuf> {
        self.origin_project_path.as_deref().map(PathBuf::from)
    }

    pub fn resolved_face_paths(&self) -> Vec<PathBuf> {
        self.face_paths.iter().map(PathBuf::from).collect()
    }
}

pub fn recovery_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShadeEditor").join("recovery-v9.json")
}

pub fn load() -> Result<Option<RecoveryFile>, String> {
    let path = recovery_path();
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .map_err(|err| format!("Cannot read recovery file {}: {err}", path.display()))?;
    let recovery: RecoveryFile = serde_json::from_str(&text)
        .map_err(|err| format!("Invalid recovery file {}: {err}", path.display()))?;
    if recovery.format_version != RECOVERY_FORMAT_VERSION {
        return Err(format!(
            "Unsupported recovery format {} (expected {}).",
            recovery.format_version, RECOVERY_FORMAT_VERSION
        ));
    }
    if recovery.project.schema_version != crate::model::SHADE_SCHEMA_VERSION {
        return Err(format!(
            "Recovery uses .shade schema {}, but this build accepts schema {} only.",
            recovery.project.schema_version,
            crate::model::SHADE_SCHEMA_VERSION
        ));
    }
    Ok(Some(recovery))
}

pub fn write(recovery: &RecoveryFile) -> Result<PathBuf, String> {
    let path = recovery_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Cannot create recovery folder {}: {err}", parent.display()))?;
    }
    let temp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(recovery)
        .map_err(|err| format!("Cannot serialize recovery state: {err}"))?;
    fs::write(&temp, text)
        .map_err(|err| format!("Cannot write recovery file {}: {err}", temp.display()))?;
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|err| format!("Cannot replace recovery file {}: {err}", path.display()))?;
    }
    fs::rename(&temp, &path)
        .map_err(|err| format!("Cannot finalize recovery file {}: {err}", path.display()))?;
    Ok(path)
}

pub fn clear() -> Result<(), String> {
    let path = recovery_path();
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|err| format!("Cannot remove recovery file {}: {err}", path.display()))?;
    }
    Ok(())
}

pub fn is_recovery_path(path: &Path) -> bool {
    path == recovery_path()
}

fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_file_keeps_absolute_face_references() {
        let project = ShadeProject::default();
        let recovery = RecoveryFile::new(
            project,
            vec![PathBuf::from(r"C:\tiles\face-1.tif")],
            Some(PathBuf::from(r"C:\tiles\test.shade")),
        );
        assert_eq!(recovery.resolved_face_paths()[0], PathBuf::from(r"C:\tiles\face-1.tif"));
        assert_eq!(recovery.origin_path(), Some(PathBuf::from(r"C:\tiles\test.shade")));
    }
}
