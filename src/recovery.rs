use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::ShadeProject;

const RECOVERY_FORMAT_VERSION: u32 = 1;
const RECOVERY_STATE_COUNT: usize = 3;

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
    recovery_paths()[0].clone()
}

fn recovery_paths() -> [PathBuf; RECOVERY_STATE_COUNT] {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ShadeEditor");
    [
        base.join("recovery-v9.json"),
        base.join("recovery-v9-1.json"),
        base.join("recovery-v9-2.json"),
    ]
}

pub fn load() -> Result<Option<RecoveryFile>, String> {
    load_from_paths(&recovery_paths())
}

fn load_from_paths(paths: &[PathBuf]) -> Result<Option<RecoveryFile>, String> {
    let mut errors = Vec::new();
    for path in paths {
        if !path.exists() {
            continue;
        }
        match read_recovery(path) {
            Ok(recovery) => return Ok(Some(recovery)),
            Err(err) => errors.push(err),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(format!(
            "No valid recovery state was found. {}",
            errors.join(" | ")
        ))
    }
}

fn read_recovery(path: &Path) -> Result<RecoveryFile, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("Cannot read recovery file {}: {err}", path.display()))?;
    let recovery: RecoveryFile = serde_json::from_str(&text)
        .map_err(|err| format!("Invalid recovery file {}: {err}", path.display()))?;
    if recovery.format_version != RECOVERY_FORMAT_VERSION {
        return Err(format!(
            "Unsupported recovery format {} in {} (expected {}).",
            recovery.format_version,
            path.display(),
            RECOVERY_FORMAT_VERSION
        ));
    }
    if recovery.project.schema_version != crate::model::SHADE_SCHEMA_VERSION {
        return Err(format!(
            "Recovery {} uses .shade schema {}, but this build accepts schema {} only.",
            path.display(),
            recovery.project.schema_version,
            crate::model::SHADE_SCHEMA_VERSION
        ));
    }
    Ok(recovery)
}

pub fn write(recovery: &RecoveryFile) -> Result<PathBuf, String> {
    write_to_paths(recovery, &recovery_paths())
}

fn write_to_paths(recovery: &RecoveryFile, paths: &[PathBuf]) -> Result<PathBuf, String> {
    let latest = paths
        .first()
        .ok_or_else(|| "Recovery path list is empty.".to_owned())?;
    if let Some(parent) = latest.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Cannot create recovery folder {}: {err}", parent.display()))?;
    }

    for index in (1..paths.len()).rev() {
        if paths[index].exists() {
            fs::remove_file(&paths[index]).map_err(|err| {
                format!(
                    "Cannot rotate recovery file {}: {err}",
                    paths[index].display()
                )
            })?;
        }
        if paths[index - 1].exists() {
            fs::rename(&paths[index - 1], &paths[index]).map_err(|err| {
                format!(
                    "Cannot rotate recovery file {} to {}: {err}",
                    paths[index - 1].display(),
                    paths[index].display()
                )
            })?;
        }
    }

    let temp = latest.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(recovery)
        .map_err(|err| format!("Cannot serialize recovery state: {err}"))?;
    fs::write(&temp, text)
        .map_err(|err| format!("Cannot write recovery file {}: {err}", temp.display()))?;
    if latest.exists() {
        fs::remove_file(latest)
            .map_err(|err| format!("Cannot replace recovery file {}: {err}", latest.display()))?;
    }
    fs::rename(&temp, latest)
        .map_err(|err| format!("Cannot finalize recovery file {}: {err}", latest.display()))?;
    Ok(latest.clone())
}

pub fn clear() -> Result<(), String> {
    for path in recovery_paths() {
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|err| format!("Cannot remove recovery file {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

pub fn is_recovery_path(path: &Path) -> bool {
    recovery_paths().iter().any(|candidate| candidate == path)
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

    fn temp_paths(label: &str) -> (PathBuf, [PathBuf; RECOVERY_STATE_COUNT]) {
        let unique = format!(
            "shade-recovery-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let folder = std::env::temp_dir().join(unique);
        let paths = [
            folder.join("recovery-v9.json"),
            folder.join("recovery-v9-1.json"),
            folder.join("recovery-v9-2.json"),
        ];
        (folder, paths)
    }

    #[test]
    fn recovery_file_keeps_absolute_face_references() {
        let project = ShadeProject::default();
        let recovery = RecoveryFile::new(
            project,
            vec![PathBuf::from(r"C:\tiles\face-1.tif")],
            Some(PathBuf::from(r"C:\tiles\test.shade")),
        );
        assert_eq!(
            recovery.resolved_face_paths()[0],
            PathBuf::from(r"C:\tiles\face-1.tif")
        );
        assert_eq!(
            recovery.origin_path(),
            Some(PathBuf::from(r"C:\tiles\test.shade"))
        );
    }

    #[test]
    fn recovery_rotation_keeps_latest_three_states() {
        let (folder, paths) = temp_paths("rotate");
        for state in 1..=4i64 {
            let mut recovery = RecoveryFile::new(ShadeProject::default(), vec![], None);
            recovery.saved_at_unix_ms = state;
            write_to_paths(&recovery, &paths).unwrap();
        }
        assert_eq!(read_recovery(&paths[0]).unwrap().saved_at_unix_ms, 4);
        assert_eq!(read_recovery(&paths[1]).unwrap().saved_at_unix_ms, 3);
        assert_eq!(read_recovery(&paths[2]).unwrap().saved_at_unix_ms, 2);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn recovery_load_falls_back_when_latest_state_is_corrupt() {
        let (folder, paths) = temp_paths("fallback");
        let mut older = RecoveryFile::new(ShadeProject::default(), vec![], None);
        older.saved_at_unix_ms = 20;
        write_to_paths(&older, &paths).unwrap();
        let mut latest = RecoveryFile::new(ShadeProject::default(), vec![], None);
        latest.saved_at_unix_ms = 30;
        write_to_paths(&latest, &paths).unwrap();
        fs::write(&paths[0], "{broken json").unwrap();
        let loaded = load_from_paths(&paths).unwrap().unwrap();
        assert_eq!(loaded.saved_at_unix_ms, 20);
        let _ = fs::remove_dir_all(folder);
    }
}
