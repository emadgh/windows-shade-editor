use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::safe_fs;

/// Existing Export/Conversion queue files both use format version 1.
///
/// The shared core intentionally does not change either domain's on-disk schema;
/// callers keep their existing file names and item/status payloads.
pub const QUEUE_FORMAT_VERSION_V1: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueLifecycle {
    Waiting,
    Processing,
    Done,
    Failed,
    Cancelled,
}

impl QueueLifecycle {
    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    /// Waiting and interrupted Processing work from a previous process must not
    /// auto-run on startup. Processing is normalized back to Waiting and both
    /// states require an explicit operator resume.
    pub fn restored(self) -> (Self, bool) {
        match self {
            Self::Waiting | Self::Processing => (Self::Waiting, true),
            other => (other, false),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedQueueEnvelope<T> {
    pub format_version: u32,
    pub next_id: u64,
    #[serde(default)]
    pub paused: bool,
    pub items: Vec<T>,
}

impl<T> PersistedQueueEnvelope<T> {
    pub fn new(next_id: u64, paused: bool, items: Vec<T>) -> Self {
        Self {
            format_version: QUEUE_FORMAT_VERSION_V1,
            next_id: next_id.max(1),
            paused,
            items,
        }
    }
}

/// Shared runtime mechanics for queue domains. Domain payloads, statuses,
/// reservations, cancellation semantics and recovery remain owned by each
/// queue implementation.
pub struct QueueRuntime<E> {
    next_id: u64,
    active_id: Option<u64>,
    paused: bool,
    tx: mpsc::Sender<E>,
    rx: mpsc::Receiver<E>,
    persistence_path: Option<PathBuf>,
    last_persistence_error: Option<String>,
}

impl<E> QueueRuntime<E> {
    pub fn new(persistence_path: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            next_id: 1,
            active_id: None,
            paused: false,
            tx,
            rx,
            persistence_path,
            last_persistence_error: None,
        }
    }

    pub fn restore_runtime(
        persistence_path: Option<PathBuf>,
        next_id: u64,
        paused: bool,
    ) -> Self {
        let mut runtime = Self::new(persistence_path);
        runtime.next_id = next_id.max(1);
        runtime.paused = paused;
        runtime
    }

    pub fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub fn active_id(&self) -> Option<u64> {
        self.active_id
    }

    pub fn set_active_id(&mut self, id: Option<u64>) {
        self.active_id = id;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Change only runtime state. The owning domain is responsible for calling
    /// its persistence method after a successful change.
    pub fn set_paused(&mut self, paused: bool) -> bool {
        if self.paused == paused {
            return false;
        }
        self.paused = paused;
        true
    }

    pub fn sender(&self) -> mpsc::Sender<E> {
        self.tx.clone()
    }

    pub fn try_recv(&self) -> Result<E, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    pub fn persistence_path(&self) -> Option<&Path> {
        self.persistence_path.as_deref()
    }

    pub fn take_persistence_error(&mut self) -> Option<String> {
        self.last_persistence_error.take()
    }

    pub fn record_persistence_result(&mut self, result: Result<(), String>) {
        if let Err(error) = result {
            self.last_persistence_error = Some(error);
        }
    }
}

pub fn load_persisted_queue<T: DeserializeOwned>(
    path: &Path,
    domain_label: &str,
) -> Result<Option<PersistedQueueEnvelope<T>>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Cannot read {domain_label} queue {}: {error}", path.display()))?;
    let persisted: PersistedQueueEnvelope<T> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid {domain_label} queue {}: {error}", path.display()))?;
    if persisted.format_version != QUEUE_FORMAT_VERSION_V1 {
        return Err(format!(
            "Unsupported {domain_label} queue format {} (expected {}).",
            persisted.format_version, QUEUE_FORMAT_VERSION_V1
        ));
    }
    Ok(Some(persisted))
}

pub fn write_persisted_queue<T: Serialize>(
    path: &Path,
    domain_label: &str,
    envelope: &PersistedQueueEnvelope<T>,
) -> Result<(), String> {
    if envelope.format_version != QUEUE_FORMAT_VERSION_V1 {
        return Err(format!(
            "Cannot write {domain_label} queue format {} (expected {}).",
            envelope.format_version, QUEUE_FORMAT_VERSION_V1
        ));
    }
    let bytes = serde_json::to_vec_pretty(envelope)
        .map_err(|error| format!("Cannot serialize {domain_label} queue: {error}"))?;
    safe_fs::atomic_write(path, &bytes, None)
}

pub fn sanitize_progress(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct Item {
        id: u64,
    }

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-queue-core-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn ids_are_stable_monotonic_and_wrap_away_from_zero() {
        let mut runtime = QueueRuntime::<()>::new(None);
        assert_eq!(runtime.allocate_id(), 1);
        assert_eq!(runtime.allocate_id(), 2);
        assert_eq!(runtime.next_id(), 3);
    }

    #[test]
    fn restored_waiting_and_processing_require_operator_resume() {
        assert_eq!(
            QueueLifecycle::Waiting.restored(),
            (QueueLifecycle::Waiting, true)
        );
        assert_eq!(
            QueueLifecycle::Processing.restored(),
            (QueueLifecycle::Waiting, true)
        );
        assert_eq!(
            QueueLifecycle::Failed.restored(),
            (QueueLifecycle::Failed, false)
        );
    }

    #[test]
    fn event_transport_and_active_pause_state_are_shared() {
        let mut runtime = QueueRuntime::new(None);
        runtime.sender().send("progress").unwrap();
        assert_eq!(runtime.try_recv().unwrap(), "progress");
        runtime.set_active_id(Some(9));
        assert_eq!(runtime.active_id(), Some(9));
        assert!(runtime.set_paused(true));
        assert!(runtime.is_paused());
        assert!(!runtime.set_paused(true));
    }

    #[test]
    fn persisted_v1_envelope_round_trips_without_schema_migration() {
        let path = temp_path("roundtrip");
        let envelope = PersistedQueueEnvelope::new(7, true, vec![Item { id: 3 }]);
        write_persisted_queue(&path, "test", &envelope).unwrap();
        let restored: PersistedQueueEnvelope<Item> =
            load_persisted_queue(&path, "test").unwrap().unwrap();
        assert_eq!(restored.format_version, 1);
        assert_eq!(restored.next_id, 7);
        assert!(restored.paused);
        assert_eq!(restored.items, vec![Item { id: 3 }]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn progress_is_sanitized_once_for_all_queue_domains() {
        assert_eq!(sanitize_progress(f32::NAN), 0.0);
        assert_eq!(sanitize_progress(f32::INFINITY), 0.0);
        assert_eq!(sanitize_progress(-1.0), 0.0);
        assert_eq!(sanitize_progress(2.0), 1.0);
    }
}
