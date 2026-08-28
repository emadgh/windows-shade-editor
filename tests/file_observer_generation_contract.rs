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
            "shade-file-observer-generation-{label}-{}-{id}",
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
fn acknowledged_missing_baseline_makes_recreation_a_new_generation() {
    let temp = TempDir::new("missing-ack");
    let path = temp.0.join("source.tif");
    write(&path, b"initial");

    let observer = FileObserver::new();
    observer.observe(&path, ExternalFileRole::Source);

    fs::remove_file(&path).unwrap();
    let missing = observer.rescan(&path).unwrap();
    assert_eq!(missing.state, ExternalFileState::Missing);
    assert_eq!(missing.generation, 1);

    let acknowledged = observer.acknowledge(&path).unwrap();
    assert_eq!(acknowledged.state, ExternalFileState::Missing);
    assert_eq!(acknowledged.generation, 1);

    write(&path, b"recreated-after-acknowledge");
    let recreated = observer.rescan(&path).unwrap();
    assert_eq!(recreated.state, ExternalFileState::Recreated);
    assert_eq!(recreated.generation, 2);

    observer.release(&path, ExternalFileRole::Source);
}
