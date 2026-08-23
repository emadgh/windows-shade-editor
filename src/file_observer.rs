use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FindCloseChangeNotification, FindFirstChangeNotificationW,
    FindNextChangeNotification,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::WaitForSingleObject;

const EVENT_DEBOUNCE: Duration = Duration::from_millis(80);
const WAIT_SLICE_MS: u32 = 1_000;
const RESCAN_EVERY_TICKS: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExternalFileRole {
    Project,
    Reference,
    Face,
    IccProfile,
    Source,
    Converted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExternalFileState {
    #[default]
    Available,
    Missing,
    Modified,
    Replaced,
    Recreated,
    Unreadable,
}

impl ExternalFileState {
    pub fn is_available(self) -> bool {
        matches!(
            self,
            Self::Available | Self::Modified | Self::Replaced | Self::Recreated
        )
    }

    pub fn is_changed(self) -> bool {
        matches!(self, Self::Modified | Self::Replaced | Self::Recreated)
    }

    pub fn is_missing(self) -> bool {
        self == Self::Missing
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFingerprint {
    pub size: u64,
    pub modified_ns: Option<u128>,
    pub created_ns: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalFileSnapshot {
    pub state: ExternalFileState,
    pub fingerprint: Option<FileFingerprint>,
    pub generation: u64,
    pub last_error: Option<String>,
}

impl ExternalFileSnapshot {
    pub fn is_available(&self) -> bool {
        self.state.is_available()
    }

    pub fn is_changed(&self) -> bool {
        self.state.is_changed()
    }

    pub fn is_missing(&self) -> bool {
        self.state.is_missing()
    }
}

#[derive(Clone, Debug)]
struct TrackedEntry {
    path: PathBuf,
    parent_key: String,
    roles: BTreeSet<ExternalFileRole>,
    fingerprint: Option<FileFingerprint>,
    state: ExternalFileState,
    generation: u64,
    last_error: Option<String>,
}

impl TrackedEntry {
    fn snapshot(&self) -> ExternalFileSnapshot {
        ExternalFileSnapshot {
            state: self.state,
            fingerprint: self.fingerprint,
            generation: self.generation,
            last_error: self.last_error.clone(),
        }
    }
}

#[derive(Default)]
struct Registry {
    entries: HashMap<String, TrackedEntry>,
    watched_dirs: HashSet<String>,
}

#[derive(Clone)]
pub struct FileObserver {
    registry: Arc<Mutex<Registry>>,
    native_watches: bool,
}

impl Default for FileObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl FileObserver {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            native_watches: true,
        }
    }

    #[cfg(test)]
    fn without_native_watches() -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            native_watches: false,
        }
    }

    /// Registering the same canonical path + role repeatedly is idempotent.
    pub fn observe(&self, path: &Path, role: ExternalFileRole) -> ExternalFileSnapshot {
        let normalized = normalized_storage_path(path);
        let key = path_key(&normalized);
        let parent = normalized
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| normalized.clone());
        let parent_key = path_key(&parent);

        let mut start_watch = false;
        let snapshot = {
            let mut registry = lock_registry(&self.registry);
            if let Some(entry) = registry.entries.get_mut(&key) {
                entry.roles.insert(role);
                entry.snapshot()
            } else {
                let (state, fingerprint, last_error) = baseline(read_fingerprint(&normalized));
                let mut roles = BTreeSet::new();
                roles.insert(role);
                let entry = TrackedEntry {
                    path: normalized.clone(),
                    parent_key: parent_key.clone(),
                    roles,
                    fingerprint,
                    state,
                    generation: 0,
                    last_error,
                };
                let snapshot = entry.snapshot();
                registry.entries.insert(key, entry);
                if self.native_watches && registry.watched_dirs.insert(parent_key.clone()) {
                    start_watch = true;
                }
                snapshot
            }
        };

        if start_watch {
            spawn_directory_observer(self.registry.clone(), parent, parent_key);
        }
        snapshot
    }

    pub fn release(&self, path: &Path, role: ExternalFileRole) {
        let key = path_key(&normalized_storage_path(path));
        let mut registry = lock_registry(&self.registry);
        let remove = registry.entries.get_mut(&key).is_some_and(|entry| {
            entry.roles.remove(&role);
            entry.roles.is_empty()
        });
        if remove {
            registry.entries.remove(&key);
        }
    }

    pub fn snapshot(&self, path: &Path) -> Option<ExternalFileSnapshot> {
        let key = path_key(&normalized_storage_path(path));
        lock_registry(&self.registry)
            .entries
            .get(&key)
            .map(TrackedEntry::snapshot)
    }

    pub fn rescan(&self, path: &Path) -> Option<ExternalFileSnapshot> {
        let key = path_key(&normalized_storage_path(path));
        refresh_key(&self.registry, &key);
        self.snapshot(path)
    }

    /// Consumers call this only after their own semantic reload/validation accepts the bytes.
    pub fn acknowledge(&self, path: &Path) -> Option<ExternalFileSnapshot> {
        let key = path_key(&normalized_storage_path(path));
        let mut registry = lock_registry(&self.registry);
        let entry = registry.entries.get_mut(&key)?;
        let (state, fingerprint, last_error) = baseline(read_fingerprint(&entry.path));
        entry.state = state;
        entry.fingerprint = fingerprint;
        entry.last_error = last_error;
        Some(entry.snapshot())
    }

    pub fn subscriber_count(&self, path: &Path) -> usize {
        let key = path_key(&normalized_storage_path(path));
        lock_registry(&self.registry)
            .entries
            .get(&key)
            .map(|entry| entry.roles.len())
            .unwrap_or(0)
    }

    pub fn tracked_path_count(&self) -> usize {
        lock_registry(&self.registry).entries.len()
    }

    pub fn underlying_watch_count(&self) -> usize {
        lock_registry(&self.registry).watched_dirs.len()
    }
}

fn shared() -> &'static FileObserver {
    static OBSERVER: OnceLock<FileObserver> = OnceLock::new();
    OBSERVER.get_or_init(FileObserver::new)
}

pub fn observe(path: &Path, role: ExternalFileRole) -> ExternalFileSnapshot {
    shared().observe(path, role)
}

pub fn release(path: &Path, role: ExternalFileRole) {
    shared().release(path, role);
}

pub fn snapshot(path: &Path) -> Option<ExternalFileSnapshot> {
    shared().snapshot(path)
}

pub fn rescan(path: &Path) -> Option<ExternalFileSnapshot> {
    shared().rescan(path)
}

pub fn acknowledge(path: &Path) -> Option<ExternalFileSnapshot> {
    shared().acknowledge(path)
}

fn lock_registry(registry: &Arc<Mutex<Registry>>) -> std::sync::MutexGuard<'_, Registry> {
    registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

enum FingerprintReading {
    Present(FileFingerprint),
    Missing,
    Unreadable(String),
}

fn baseline(
    reading: FingerprintReading,
) -> (ExternalFileState, Option<FileFingerprint>, Option<String>) {
    match reading {
        FingerprintReading::Present(fingerprint) => {
            (ExternalFileState::Available, Some(fingerprint), None)
        }
        FingerprintReading::Missing => (ExternalFileState::Missing, None, None),
        FingerprintReading::Unreadable(error) => {
            (ExternalFileState::Unreadable, None, Some(error))
        }
    }
}

fn read_fingerprint(path: &Path) -> FingerprintReading {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => FingerprintReading::Present(FileFingerprint {
            size: metadata.len(),
            modified_ns: to_ns(metadata.modified().ok()),
            created_ns: to_ns(metadata.created().ok()),
        }),
        Ok(_) => FingerprintReading::Unreadable(format!(
            "Tracked external path is not a file: {}",
            path.display()
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => FingerprintReading::Missing,
        Err(err) => FingerprintReading::Unreadable(format!(
            "Cannot inspect external file {}: {err}",
            path.display()
        )),
    }
}

fn to_ns(time: Option<SystemTime>) -> Option<u128> {
    time.and_then(|time| {
        time.duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_nanos())
    })
}

fn refresh_key(registry: &Arc<Mutex<Registry>>, key: &str) {
    let path = {
        let registry = lock_registry(registry);
        let Some(entry) = registry.entries.get(key) else {
            return;
        };
        entry.path.clone()
    };
    let reading = read_fingerprint(&path);
    let mut registry = lock_registry(registry);
    let Some(entry) = registry.entries.get_mut(key) else {
        return;
    };
    apply_reading(entry, reading);
}

fn apply_reading(entry: &mut TrackedEntry, reading: FingerprintReading) {
    match reading {
        FingerprintReading::Present(current) => {
            let next = match entry.fingerprint {
                Some(previous) if previous == current => {
                    if entry.state.is_changed() {
                        entry.state
                    } else {
                        ExternalFileState::Available
                    }
                }
                Some(previous) => {
                    entry.generation = entry.generation.wrapping_add(1).max(1);
                    if previous.created_ns.is_some()
                        && current.created_ns.is_some()
                        && previous.created_ns != current.created_ns
                    {
                        ExternalFileState::Replaced
                    } else {
                        ExternalFileState::Modified
                    }
                }
                None if entry.state == ExternalFileState::Missing => {
                    entry.generation = entry.generation.wrapping_add(1).max(1);
                    ExternalFileState::Recreated
                }
                None => ExternalFileState::Available,
            };
            entry.fingerprint = Some(current);
            entry.state = next;
            entry.last_error = None;
        }
        FingerprintReading::Missing => {
            if entry.fingerprint.is_some() || entry.state != ExternalFileState::Missing {
                entry.generation = entry.generation.wrapping_add(1).max(1);
            }
            entry.fingerprint = None;
            entry.state = ExternalFileState::Missing;
            entry.last_error = None;
        }
        FingerprintReading::Unreadable(error) => {
            if entry.state != ExternalFileState::Unreadable
                || entry.last_error.as_deref() != Some(error.as_str())
            {
                entry.generation = entry.generation.wrapping_add(1).max(1);
            }
            entry.fingerprint = None;
            entry.state = ExternalFileState::Unreadable;
            entry.last_error = Some(error);
        }
    }
}

fn refresh_directory(registry: &Arc<Mutex<Registry>>, parent_key: &str) {
    let keys = {
        let registry = lock_registry(registry);
        registry
            .entries
            .iter()
            .filter(|(_, entry)| entry.parent_key == parent_key)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>()
    };
    for key in keys {
        refresh_key(registry, &key);
    }
}

fn directory_needed(registry: &Arc<Mutex<Registry>>, parent_key: &str) -> bool {
    lock_registry(registry)
        .entries
        .values()
        .any(|entry| entry.parent_key == parent_key && !entry.roles.is_empty())
}

fn finish_watch(registry: &Arc<Mutex<Registry>>, parent_key: &str) {
    let mut registry = lock_registry(registry);
    if !registry
        .entries
        .values()
        .any(|entry| entry.parent_key == parent_key && !entry.roles.is_empty())
    {
        registry.watched_dirs.remove(parent_key);
    }
}

fn spawn_directory_observer(
    registry: Arc<Mutex<Registry>>,
    parent: PathBuf,
    parent_key: String,
) {
    let _ = thread::Builder::new()
        .name("shade-file-observer".to_owned())
        .spawn(move || run_directory_observer(registry, parent, parent_key));
}

#[cfg(windows)]
fn run_directory_observer(
    registry: Arc<Mutex<Registry>>,
    parent: PathBuf,
    parent_key: String,
) {
    let mut wide = parent.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_CREATION;
    let handle = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 0, filter) };
    if handle == INVALID_HANDLE_VALUE {
        poll_fallback(&registry, &parent_key);
        finish_watch(&registry, &parent_key);
        return;
    }

    let mut ticks = 0u32;
    loop {
        if !directory_needed(&registry, &parent_key) {
            break;
        }
        let wait = unsafe { WaitForSingleObject(handle, WAIT_SLICE_MS) };
        if wait == WAIT_OBJECT_0 {
            thread::sleep(EVENT_DEBOUNCE);
            refresh_directory(&registry, &parent_key);
            ticks = 0;
            if unsafe { FindNextChangeNotification(handle) } == 0 {
                unsafe { FindCloseChangeNotification(handle) };
                poll_fallback(&registry, &parent_key);
                finish_watch(&registry, &parent_key);
                return;
            }
        } else if wait == WAIT_TIMEOUT {
            ticks = ticks.saturating_add(1);
            if ticks >= RESCAN_EVERY_TICKS {
                refresh_directory(&registry, &parent_key);
                ticks = 0;
            }
        } else {
            unsafe { FindCloseChangeNotification(handle) };
            poll_fallback(&registry, &parent_key);
            finish_watch(&registry, &parent_key);
            return;
        }
    }
    unsafe { FindCloseChangeNotification(handle) };
    finish_watch(&registry, &parent_key);
}

#[cfg(not(windows))]
fn run_directory_observer(
    registry: Arc<Mutex<Registry>>,
    _parent: PathBuf,
    parent_key: String,
) {
    poll_fallback(&registry, &parent_key);
    finish_watch(&registry, &parent_key);
}

fn poll_fallback(registry: &Arc<Mutex<Registry>>, parent_key: &str) {
    while directory_needed(registry, parent_key) {
        refresh_directory(registry, parent_key);
        thread::sleep(Duration::from_secs(1));
    }
}

fn normalized_storage_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    if let (Some(parent), Some(file_name)) = (absolute.parent(), absolute.file_name()) {
        if let Ok(parent) = fs::canonicalize(parent) {
            return parent.join(file_name);
        }
    }
    absolute
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "shade-file-observer-{label}-{}-{id}",
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
    fn duplicate_roles_share_one_path_and_release_is_reference_safe() {
        let temp = TempDir::new("dedup");
        let path = temp.0.join("a.tif");
        write(&path, b"one");
        let observer = FileObserver::without_native_watches();
        observer.observe(&path, ExternalFileRole::Face);
        observer.observe(&path, ExternalFileRole::Reference);
        observer.observe(&path, ExternalFileRole::Face);
        assert_eq!(observer.tracked_path_count(), 1);
        assert_eq!(observer.subscriber_count(&path), 2);
        observer.release(&path, ExternalFileRole::Face);
        assert_eq!(observer.subscriber_count(&path), 1);
        observer.release(&path, ExternalFileRole::Reference);
        assert_eq!(observer.tracked_path_count(), 0);
    }

    #[test]
    fn detects_modify_delete_and_recreate_with_sticky_change_state() {
        let temp = TempDir::new("lifecycle");
        let path = temp.0.join("a.tif");
        write(&path, b"one");
        let observer = FileObserver::without_native_watches();
        assert_eq!(
            observer.observe(&path, ExternalFileRole::Face).state,
            ExternalFileState::Available
        );
        write(&path, b"different-length");
        assert!(observer.rescan(&path).unwrap().is_changed());
        assert!(observer.rescan(&path).unwrap().is_changed());
        observer.acknowledge(&path).unwrap();
        assert_eq!(observer.snapshot(&path).unwrap().state, ExternalFileState::Available);
        fs::remove_file(&path).unwrap();
        assert!(observer.rescan(&path).unwrap().is_missing());
        write(&path, b"recreated");
        assert_eq!(observer.rescan(&path).unwrap().state, ExternalFileState::Recreated);
    }

    #[test]
    fn initial_missing_registration_detects_later_creation() {
        let temp = TempDir::new("missing");
        let path = temp.0.join("later.icc");
        let observer = FileObserver::without_native_watches();
        assert!(observer
            .observe(&path, ExternalFileRole::IccProfile)
            .is_missing());
        write(&path, b"profile");
        assert_eq!(observer.rescan(&path).unwrap().state, ExternalFileState::Recreated);
    }

    #[test]
    fn windows_case_variants_are_one_logical_path() {
        let temp = TempDir::new("case");
        let path = temp.0.join("CaseFile.tif");
        write(&path, b"one");
        let upper = PathBuf::from(path.to_string_lossy().to_uppercase());
        let observer = FileObserver::without_native_watches();
        observer.observe(&path, ExternalFileRole::Face);
        observer.observe(&upper, ExternalFileRole::Reference);
        assert_eq!(observer.tracked_path_count(), 1);
        assert_eq!(observer.subscriber_count(&path), 2);
    }

    #[test]
    fn conversion_source_and_converted_roles_are_supported() {
        let temp = TempDir::new("conversion");
        let source = temp.0.join("source.tif");
        let converted = temp.0.join("converted.tif");
        write(&source, b"source");
        write(&converted, b"converted");
        let observer = FileObserver::without_native_watches();
        assert!(observer.observe(&source, ExternalFileRole::Source).is_available());
        assert!(observer
            .observe(&converted, ExternalFileRole::Converted)
            .is_available());
        assert_eq!(observer.tracked_path_count(), 2);
    }
}
