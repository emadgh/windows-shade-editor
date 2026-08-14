use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{model::ShadeProject, safe_fs};

const RECOVERY_FORMAT_VERSION: u32 = 2;
const LEGACY_RECOVERY_FORMAT_VERSION: u32 = 1;
const RECOVERY_STATE_COUNT: usize = 3;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryFile {
    pub format_version: u32,
    pub saved_at_unix_ms: i64,
    pub origin_project_path: Option<String>,
    pub face_paths: Vec<String>,
    pub project: ShadeProject,
    #[serde(default)]
    pub checksum_sha256: String,
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
            checksum_sha256: String::new(),
        }
    }

    pub fn origin_path(&self) -> Option<PathBuf> {
        self.origin_project_path.as_deref().map(PathBuf::from)
    }

    pub fn resolved_face_paths(&self) -> Vec<PathBuf> {
        self.face_paths.iter().map(PathBuf::from).collect()
    }
}

#[derive(Serialize)]
struct RecoveryChecksumPayload<'a> {
    format_version: u32,
    saved_at_unix_ms: i64,
    origin_project_path: &'a Option<String>,
    face_paths: &'a [String],
    project: &'a ShadeProject,
}

fn recovery_checksum(recovery: &RecoveryFile) -> Result<String, String> {
    let payload = RecoveryChecksumPayload {
        format_version: recovery.format_version,
        saved_at_unix_ms: recovery.saved_at_unix_ms,
        origin_project_path: &recovery.origin_project_path,
        face_paths: &recovery.face_paths,
        project: &recovery.project,
    };
    let bytes = serde_json::to_vec(&payload)
        .map_err(|err| format!("Cannot serialize recovery checksum payload: {err}"))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn stamped_recovery(recovery: &RecoveryFile) -> Result<RecoveryFile, String> {
    let mut stamped = recovery.clone();
    stamped.format_version = RECOVERY_FORMAT_VERSION;
    stamped.checksum_sha256.clear();
    stamped.checksum_sha256 = recovery_checksum(&stamped)?;
    Ok(stamped)
}

fn verify_recovery_checksum(recovery: &RecoveryFile) -> Result<(), String> {
    if recovery.checksum_sha256.trim().is_empty() {
        return Err("Recovery integrity checksum is missing.".to_owned());
    }
    let expected = recovery_checksum(recovery)?;
    if !recovery.checksum_sha256.eq_ignore_ascii_case(&expected) {
        return Err("Recovery integrity checksum does not match the saved payload.".to_owned());
    }
    Ok(())
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
    let mut recovery: RecoveryFile = serde_json::from_str(&text)
        .map_err(|err| format!("Invalid recovery file {}: {err}", path.display()))?;
    match recovery.format_version {
        LEGACY_RECOVERY_FORMAT_VERSION => {}
        RECOVERY_FORMAT_VERSION => verify_recovery_checksum(&recovery)
            .map_err(|err| format!("Invalid recovery integrity in {}: {err}", path.display()))?,
        other => {
            return Err(format!(
                "Unsupported recovery format {other} in {} (expected {} or legacy {}).",
                path.display(),
                RECOVERY_FORMAT_VERSION,
                LEGACY_RECOVERY_FORMAT_VERSION
            ));
        }
    }
    if recovery.project.schema_version != crate::model::SHADE_SCHEMA_VERSION {
        return Err(format!(
            "Recovery {} uses .shade schema {}, but this build accepts schema {} only.",
            path.display(),
            recovery.project.schema_version,
            crate::model::SHADE_SCHEMA_VERSION
        ));
    }
    recovery.project.ensure_snapshot_histories();
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

    let stamped = stamped_recovery(recovery)?;
    let bytes = serde_json::to_vec_pretty(&stamped)
        .map_err(|err| format!("Cannot serialize recovery state: {err}"))?;
    let verify: RecoveryFile = serde_json::from_slice(&bytes)
        .map_err(|err| format!("Cannot verify serialized recovery state: {err}"))?;
    verify_recovery_checksum(&verify)?;

    // Keep the older generations intact until the new state has been fully
    // serialized and verified. Rotate generation 1 -> 2 first, then let the
    // atomic latest write create generation 1 as its backup.
    for index in (2..paths.len()).rev() {
        let source = &paths[index - 1];
        if source.exists() {
            safe_fs::atomic_copy(source, &paths[index])?;
        }
    }

    safe_fs::atomic_write(latest, &bytes, paths.get(1).map(PathBuf::as_path))?;
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

    #[test]
    fn recovery_v2_rejects_valid_json_with_tampered_payload() {
        let (folder, paths) = temp_paths("checksum");
        let mut recovery = RecoveryFile::new(ShadeProject::default(), vec![], None);
        recovery.saved_at_unix_ms = 100;
        write_to_paths(&recovery, &paths).unwrap();

        let text = fs::read_to_string(&paths[0]).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["saved_at_unix_ms"] = serde_json::Value::from(101);
        fs::write(&paths[0], serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let err = read_recovery(&paths[0]).unwrap_err();
        assert!(err.contains("checksum"), "unexpected error: {err}");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn legacy_v1_recovery_remains_readable() {
        let (folder, paths) = temp_paths("legacy");
        fs::create_dir_all(&folder).unwrap();
        let mut recovery = RecoveryFile::new(ShadeProject::default(), vec![], None);
        recovery.format_version = LEGACY_RECOVERY_FORMAT_VERSION;
        recovery.checksum_sha256.clear();
        fs::write(&paths[0], serde_json::to_vec_pretty(&recovery).unwrap()).unwrap();

        let loaded = read_recovery(&paths[0]).unwrap();
        assert_eq!(loaded.format_version, LEGACY_RECOVERY_FORMAT_VERSION);
        let _ = fs::remove_dir_all(folder);
    }
}
