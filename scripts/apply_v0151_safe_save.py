from pathlib import Path
import re

root = Path('.')

# Version bump.
cargo = root.joinpath('Cargo.toml').read_text(encoding='utf-8')
cargo = cargo.replace('version = "0.15.0"', 'version = "0.15.1"', 1)
root.joinpath('Cargo.toml').write_text(cargo, encoding='utf-8')

lock = root.joinpath('Cargo.lock').read_text(encoding='utf-8')
needle = 'name = "windows-shade-editor"\nversion = "0.15.0"'
if needle not in lock:
    raise SystemExit('Cargo.lock package version marker not found')
lock = lock.replace(needle, 'name = "windows-shade-editor"\nversion = "0.15.1"', 1)
root.joinpath('Cargo.lock').write_text(lock, encoding='utf-8')

# Shared safe filesystem primitive.
safe_fs = r'''use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

pub fn backup_path(path: &Path) -> PathBuf {
    append_suffix(path, ".bak")
}

pub fn temp_path(path: &Path) -> PathBuf {
    append_suffix(path, ".tmp")
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
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|err| format!("Cannot sync saved file {}: {err}", path.display()))?;
        Ok(())
    })();

    if write_result.is_err() && temp.exists() {
        let _ = fs::remove_file(&temp);
    }
    write_result
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
    File::open(&temp)
        .and_then(|file| file.sync_all())
        .map_err(|err| format!("Cannot sync staged backup {}: {err}", temp.display()))?;
    replace_path(&temp, destination).map_err(|err| {
        let _ = fs::remove_file(&temp);
        err
    })
}

#[cfg(windows)]
fn replace_path(source: &Path, destination: &Path) -> Result<(), String> {
    let source_wide = wide_path(source);
    let destination_wide = wide_path(destination);
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(format!(
            "Cannot atomically replace {} with {}: {}",
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
    })
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
}
'''
root.joinpath('src/safe_fs.rs').write_text(safe_fs, encoding='utf-8')

# Register module at crate root.
app_path = root.joinpath('src/app_main.rs')
app = app_path.read_text(encoding='utf-8')
module_anchor = '#[path = "recovery.rs"]\nmod recovery;\n'
if '#[path = "safe_fs.rs"]' not in app:
    if module_anchor not in app:
        raise SystemExit('app_main recovery module anchor not found')
    app = app.replace(module_anchor, module_anchor + '#[path = "safe_fs.rs"]\nmod safe_fs;\n', 1)
app_path.write_text(app, encoding='utf-8')

# Route .shade saves through atomic write + .shade.bak.
model_path = root.joinpath('src/model_v6.rs')
model = model_path.read_text(encoding='utf-8')
if 'use crate::safe_fs;' not in model:
    model = model.replace('use serde::{Deserialize, Serialize};\n', 'use serde::{Deserialize, Serialize};\n\nuse crate::safe_fs;\n', 1)
old_save = '        fs::write(path, text).map_err(|err| format!("Cannot save .shade file: {err}"))\n'
new_save = '''        let backup = safe_fs::backup_path(path);\n        safe_fs::atomic_write(path, text.as_bytes(), Some(&backup)).map_err(|err| {\n            format!("Cannot safely save .shade file {}: {err}", path.display())\n        })\n'''
if old_save not in model:
    raise SystemExit('model save write anchor not found')
model = model.replace(old_save, new_save, 1)
model_path.write_text(model, encoding='utf-8')

# Recovery format v2: integrity hash + atomic three-state rotation.
recovery_path = root.joinpath('src/recovery.rs')
recovery = recovery_path.read_text(encoding='utf-8')
recovery = recovery.replace(
    'use serde::{Deserialize, Serialize};\n\nuse crate::model::ShadeProject;\n\nconst RECOVERY_FORMAT_VERSION: u32 = 1;\n',
    'use serde::{Deserialize, Serialize};\nuse sha2::{Digest, Sha256};\n\nuse crate::{model::ShadeProject, safe_fs};\n\nconst RECOVERY_FORMAT_VERSION: u32 = 2;\nconst LEGACY_RECOVERY_FORMAT_VERSION: u32 = 1;\n',
    1,
)
struct_anchor = '    pub face_paths: Vec<String>,\n    pub project: ShadeProject,\n'
if struct_anchor not in recovery:
    raise SystemExit('recovery struct anchor not found')
recovery = recovery.replace(
    struct_anchor,
    '    pub face_paths: Vec<String>,\n    pub project: ShadeProject,\n    #[serde(default)]\n    pub checksum_sha256: String,\n',
    1,
)
new_anchor = '            project,\n        }\n'
if new_anchor not in recovery:
    raise SystemExit('recovery constructor anchor not found')
recovery = recovery.replace(new_anchor, '            project,\n            checksum_sha256: String::new(),\n        }\n', 1)

checksum_helpers = r'''
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

'''
recovery_marker = 'pub fn recovery_path() -> PathBuf {'
if checksum_helpers.strip() not in recovery:
    if recovery_marker not in recovery:
        raise SystemExit('recovery helper insertion marker not found')
    recovery = recovery.replace(recovery_marker, checksum_helpers + recovery_marker, 1)

read_pattern = re.compile(r'fn read_recovery\(path: &Path\) -> Result<RecoveryFile, String> \{.*?\n\}\n\npub fn write\(', re.S)
read_replacement = r'''fn read_recovery(path: &Path) -> Result<RecoveryFile, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("Cannot read recovery file {}: {err}", path.display()))?;
    let mut recovery: RecoveryFile = serde_json::from_str(&text)
        .map_err(|err| format!("Invalid recovery file {}: {err}", path.display()))?;
    match recovery.format_version {
        LEGACY_RECOVERY_FORMAT_VERSION => {}
        RECOVERY_FORMAT_VERSION => verify_recovery_checksum(&recovery).map_err(|err| {
            format!("Invalid recovery integrity in {}: {err}", path.display())
        })?,
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

pub fn write('''
recovery, count = read_pattern.subn(read_replacement, recovery, count=1)
if count != 1:
    raise SystemExit(f'read_recovery replacement count={count}')

write_pattern = re.compile(r'fn write_to_paths\(recovery: &RecoveryFile, paths: &\[PathBuf\]\) -> Result<PathBuf, String> \{.*?\n\}\n\npub fn clear\(', re.S)
write_replacement = r'''fn write_to_paths(recovery: &RecoveryFile, paths: &[PathBuf]) -> Result<PathBuf, String> {
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

pub fn clear('''
recovery, count = write_pattern.subn(write_replacement, recovery, count=1)
if count != 1:
    raise SystemExit(f'write_to_paths replacement count={count}')

# Add integrity regression tests before the final test module brace.
test_insert = r'''

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
'''
last_brace = recovery.rfind('\n}')
if last_brace == -1:
    raise SystemExit('recovery tests final brace not found')
if 'recovery_v2_rejects_valid_json_with_tampered_payload' not in recovery:
    recovery = recovery[:last_brace] + test_insert + recovery[last_brace:]
recovery_path.write_text(recovery, encoding='utf-8')

print('Applied v0.15.1 safe project save and recovery hardening')
