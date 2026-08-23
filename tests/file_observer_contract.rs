#![cfg(windows)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use windows_shade_editor::file_observer::{
    ExternalFileRole, ExternalFileState, FileObserver,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "shade-file-observer-contract-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, bytes: &[u8]) {
    let mut file = fs::File::create(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn multiple_consumers_of_same_path_keep_one_underlying_directory_watch() {
    let temp = TempDir::new("watch-dedup");
    let path = temp.0.join("shared.tif");
    write(&path, b"initial");

    let observer = FileObserver::new();
    observer.observe(&path, ExternalFileRole::Face);
    assert_eq!(observer.underlying_watch_count(), 1);

    observer.observe(&path, ExternalFileRole::Reference);
    observer.observe(&path, ExternalFileRole::Source);
    assert_eq!(observer.tracked_path_count(), 1);
    assert_eq!(observer.subscriber_count(&path), 3);
    assert_eq!(observer.underlying_watch_count(), 1);

    observer.release(&path, ExternalFileRole::Face);
    observer.release(&path, ExternalFileRole::Reference);
    observer.release(&path, ExternalFileRole::Source);
}

#[test]
fn rename_away_and_back_is_missing_then_recreated() {
    let temp = TempDir::new("rename");
    let path = temp.0.join("project.shade");
    let moved = temp.0.join("project-moved.shade");
    write(&path, b"project-v1");

    let observer = FileObserver::new();
    observer.observe(&path, ExternalFileRole::Project);

    fs::rename(&path, &moved).unwrap();
    let missing = observer.rescan(&path).unwrap();
    assert_eq!(missing.state, ExternalFileState::Missing);

    fs::rename(&moved, &path).unwrap();
    let recreated = observer.rescan(&path).unwrap();
    assert_eq!(recreated.state, ExternalFileState::Recreated);
    assert!(recreated.is_changed());

    observer.release(&path, ExternalFileRole::Project);
}

#[test]
fn atomic_style_replacement_is_reported_as_external_change() {
    let temp = TempDir::new("replace");
    let path = temp.0.join("profile.icc");
    let replacement = temp.0.join("profile.icc.tmp");
    write(&path, b"old-profile-bytes");

    let observer = FileObserver::new();
    observer.observe(&path, ExternalFileRole::IccProfile);

    write(&replacement, b"new-profile-bytes-with-different-size");
    fs::remove_file(&path).unwrap();
    fs::rename(&replacement, &path).unwrap();

    let changed = observer.rescan(&path).unwrap();
    assert!(matches!(
        changed.state,
        ExternalFileState::Modified | ExternalFileState::Replaced
    ));
    assert!(changed.is_changed());

    observer.release(&path, ExternalFileRole::IccProfile);
}

#[test]
fn burst_changes_coalesce_to_one_sticky_generation_until_acknowledged() {
    let temp = TempDir::new("burst");
    let path = temp.0.join("source.tif");
    write(&path, b"v1");

    let observer = FileObserver::new();
    observer.observe(&path, ExternalFileRole::Source);

    write(&path, b"version-two");
    write(&path, b"version-three-is-longer");
    write(&path, b"version-four-is-the-final-burst-value");

    let first = observer.rescan(&path).unwrap();
    assert!(first.is_changed());
    assert_eq!(first.generation, 1);

    let repeated = observer.rescan(&path).unwrap();
    assert!(repeated.is_changed());
    assert_eq!(repeated.generation, first.generation);

    let acknowledged = observer.acknowledge(&path).unwrap();
    assert_eq!(acknowledged.state, ExternalFileState::Available);
    assert_eq!(acknowledged.generation, first.generation);

    observer.release(&path, ExternalFileRole::Source);
}
