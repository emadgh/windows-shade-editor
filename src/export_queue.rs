use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::export;
use crate::export_batch::{self, ConflictPolicy, DestinationDecision};
use crate::export_recipe::ExportRecipe;
use crate::path_safety;
use crate::safe_fs;
use crate::validation;

const QUEUE_FORMAT_VERSION: u32 = 1;
const FINGERPRINT_SAMPLE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExportQueueStatus {
    Waiting,
    Processing,
    Done,
    Failed,
    Cancelled,
}

impl ExportQueueStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::Processing => "Processing",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportQueueMark {
    pub snapshot_id: u64,
    pub face_key: String,
    pub folder: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub size_bytes: u64,
    pub modified_unix_ns: Option<u128>,
    pub sampled_sha256: String,
}

impl SourceFingerprint {
    pub fn capture(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::metadata(path)
            .map_err(|err| format!("Cannot fingerprint source TIFF {}: {err}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("Source TIFF is not a file: {}", path.display()));
        }
        let size_bytes = metadata.len();
        let modified_unix_ns = metadata.modified().ok().and_then(|time| {
            time.duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_nanos())
        });
        let sampled_sha256 = sampled_sha256(path, size_bytes)?;
        Ok(Self {
            size_bytes,
            modified_unix_ns,
            sampled_sha256,
        })
    }

    pub fn verify(&self, path: &Path) -> Result<(), String> {
        let current = Self::capture(path)?;
        if &current != self {
            return Err(format!(
                "Source TIFF changed after it was queued. Re-queue the export using the current source: {}",
                path.display()
            ));
        }
        Ok(())
    }
}

fn sampled_sha256(path: &Path, size: u64) -> Result<String, String> {
    let mut file = File::open(path).map_err(|err| {
        format!(
            "Cannot open source TIFF {} for fingerprint: {err}",
            path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());

    let head_len = size.min(FINGERPRINT_SAMPLE_BYTES as u64) as usize;
    let mut head = vec![0u8; head_len];
    file.read_exact(&mut head)
        .map_err(|err| format!("Cannot read source fingerprint head: {err}"))?;
    hasher.update(&head);

    if size > FINGERPRINT_SAMPLE_BYTES as u64 {
        let tail_len = size.min(FINGERPRINT_SAMPLE_BYTES as u64) as usize;
        file.seek(SeekFrom::Start(size.saturating_sub(tail_len as u64)))
            .map_err(|err| format!("Cannot seek source fingerprint tail: {err}"))?;
        let mut tail = vec![0u8; tail_len];
        file.read_exact(&mut tail)
            .map_err(|err| format!("Cannot read source fingerprint tail: {err}"))?;
        hasher.update(&tail);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportQueueSpec {
    pub label: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub recipe: ExportRecipe,
    pub default_dpi: f64,
    pub force_lzw: bool,
    pub validate_after_export: bool,
    pub conflict_policy: ConflictPolicy,
    pub mark: Option<ExportQueueMark>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct QueuedExportSpec {
    export: ExportQueueSpec,
    protected_sources: Vec<PathBuf>,
    source_fingerprint: Option<SourceFingerprint>,
    #[serde(skip)]
    project_session_id: u64,
}

#[derive(Clone, Debug)]
pub struct ExportQueueItem {
    pub id: u64,
    pub label: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub status: ExportQueueStatus,
    pub progress: f32,
    pub detail: String,
    pub error: Option<String>,
    /// True when this row came from a previous application session.
    pub restored: bool,
    /// Restored Waiting/Processing work never starts until the operator explicitly resumes it.
    pub requires_resume: bool,
    spec: QueuedExportSpec,
}

#[derive(Clone, Debug)]
pub struct ExportQueueCompletion {
    pub id: u64,
    pub project_session_id: u64,
    pub result: Result<String, String>,
    pub mark: Option<ExportQueueMark>,
}

enum ExportQueueEvent {
    Progress {
        id: u64,
        fraction: f32,
        detail: String,
    },
    Finished {
        id: u64,
        project_session_id: u64,
        result: Result<String, String>,
        mark: Option<ExportQueueMark>,
    },
}

#[derive(Serialize, Deserialize)]
struct PersistedQueue {
    format_version: u32,
    next_id: u64,
    items: Vec<PersistedQueueItem>,
}

#[derive(Serialize, Deserialize)]
struct PersistedQueueItem {
    id: u64,
    status: ExportQueueStatus,
    spec: QueuedExportSpec,
    error: Option<String>,
}

pub struct ExportQueue {
    items: Vec<ExportQueueItem>,
    next_id: u64,
    active_id: Option<u64>,
    stop_after_current: bool,
    tx: mpsc::Sender<ExportQueueEvent>,
    rx: mpsc::Receiver<ExportQueueEvent>,
    persistence_path: Option<PathBuf>,
    last_persistence_error: Option<String>,
}

impl Default for ExportQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportQueue {
    pub fn new() -> Self {
        Self::empty(None)
    }

    pub fn load_persistent() -> Result<Self, String> {
        Self::load_from_path(queue_persistence_path())
    }

    fn empty(persistence_path: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            items: Vec::new(),
            next_id: 1,
            active_id: None,
            stop_after_current: false,
            tx,
            rx,
            persistence_path,
            last_persistence_error: None,
        }
    }

    fn load_from_path(path: PathBuf) -> Result<Self, String> {
        let mut queue = Self::empty(Some(path.clone()));
        if !path.exists() {
            return Ok(queue);
        }
        let bytes = std::fs::read(&path)
            .map_err(|err| format!("Cannot read export queue {}: {err}", path.display()))?;
        let persisted: PersistedQueue = serde_json::from_slice(&bytes)
            .map_err(|err| format!("Invalid export queue {}: {err}", path.display()))?;
        if persisted.format_version != QUEUE_FORMAT_VERSION {
            return Err(format!(
                "Unsupported export queue format {} (expected {}).",
                persisted.format_version, QUEUE_FORMAT_VERSION
            ));
        }
        queue.next_id = persisted.next_id.max(1);
        for saved in persisted.items {
            if saved.status == ExportQueueStatus::Done {
                continue;
            }
            let recovered_processing = saved.status == ExportQueueStatus::Processing;
            let requires_resume = matches!(
                saved.status,
                ExportQueueStatus::Waiting | ExportQueueStatus::Processing
            );
            let status = if recovered_processing {
                ExportQueueStatus::Waiting
            } else {
                saved.status
            };
            let mut spec = saved.spec;
            spec.project_session_id = 0;
            spec.export.mark = None;
            queue.items.push(ExportQueueItem {
                id: saved.id,
                label: spec.export.label.clone(),
                source: spec.export.source.clone(),
                destination: spec.export.destination.clone(),
                status,
                progress: 0.0,
                detail: if requires_resume {
                    "Recovered from previous session · paused until you resume it".to_owned()
                } else {
                    String::new()
                },
                error: saved.error,
                restored: true,
                requires_resume,
                spec,
            });
        }
        Ok(queue)
    }

    pub fn take_persistence_error(&mut self) -> Option<String> {
        self.last_persistence_error.take()
    }

    fn persist(&mut self) {
        let Some(path) = self.persistence_path.clone() else {
            return;
        };
        let items = self
            .items
            .iter()
            .filter(|item| item.status != ExportQueueStatus::Done)
            .map(|item| PersistedQueueItem {
                id: item.id,
                status: item.status,
                spec: item.spec.clone(),
                error: item.error.clone(),
            })
            .collect();
        let persisted = PersistedQueue {
            format_version: QUEUE_FORMAT_VERSION,
            next_id: self.next_id,
            items,
        };
        let result = serde_json::to_vec_pretty(&persisted)
            .map_err(|err| format!("Cannot serialize export queue: {err}"))
            .and_then(|bytes| safe_fs::atomic_write(&path, &bytes, None));
        if let Err(err) = result {
            self.last_persistence_error = Some(err);
        }
    }

    pub fn enqueue(&mut self, spec: ExportQueueSpec) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.items.push(ExportQueueItem {
            id,
            label: spec.label.clone(),
            source: spec.source.clone(),
            destination: spec.destination.clone(),
            status: ExportQueueStatus::Waiting,
            progress: 0.0,
            detail: String::new(),
            error: None,
            restored: false,
            requires_resume: false,
            spec: QueuedExportSpec {
                export: spec,
                protected_sources: Vec::new(),
                source_fingerprint: None,
                project_session_id: 0,
            },
        });
        self.persist();
        id
    }

    pub fn enqueue_for_project(
        &mut self,
        spec: ExportQueueSpec,
        protected_sources: Vec<PathBuf>,
        project_session_id: u64,
    ) -> Result<u64, String> {
        validate_destination(&spec.destination, &protected_sources)?;
        let key = path_safety::path_key(&spec.destination);
        if self.reserved_destination_keys().contains(&key) {
            return Err(format!(
                "Export destination is already reserved by another queued job: {}",
                spec.destination.display()
            ));
        }
        let source_fingerprint = SourceFingerprint::capture(&spec.source)?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.items.push(ExportQueueItem {
            id,
            label: spec.label.clone(),
            source: spec.source.clone(),
            destination: spec.destination.clone(),
            status: ExportQueueStatus::Waiting,
            progress: 0.0,
            detail: String::new(),
            error: None,
            restored: false,
            requires_resume: false,
            spec: QueuedExportSpec {
                export: spec,
                protected_sources,
                source_fingerprint: Some(source_fingerprint),
                project_session_id,
            },
        });
        self.persist();
        Ok(id)
    }

    pub fn items(&self) -> &[ExportQueueItem] {
        &self.items
    }

    pub fn restored_count(&self) -> usize {
        self.items.iter().filter(|item| item.restored).count()
    }

    pub fn recovered_waiting_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == ExportQueueStatus::Waiting && item.requires_resume)
            .count()
    }

    pub fn resume(&mut self, id: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if item.status != ExportQueueStatus::Waiting || !item.requires_resume {
            return false;
        }
        item.requires_resume = false;
        item.detail.clear();
        self.persist();
        true
    }

    pub fn resume_recovered(&mut self) -> usize {
        let mut resumed = 0usize;
        for item in &mut self.items {
            if item.status == ExportQueueStatus::Waiting && item.requires_resume {
                item.requires_resume = false;
                item.detail.clear();
                resumed += 1;
            }
        }
        if resumed > 0 {
            self.persist();
        }
        resumed
    }

    pub fn pending_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| {
                item.status == ExportQueueStatus::Processing
                    || (item.status == ExportQueueStatus::Waiting && !item.requires_resume)
            })
            .count()
    }

    pub fn has_pending(&self) -> bool {
        self.pending_count() > 0
    }

    pub fn reserved_destination_keys(&self) -> BTreeSet<String> {
        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    ExportQueueStatus::Waiting | ExportQueueStatus::Processing
                )
            })
            .map(|item| path_safety::path_key(&item.destination))
            .collect()
    }

    pub fn active_summary(&self) -> Option<(f32, String)> {
        let id = self.active_id?;
        let item = self.items.iter().find(|item| item.id == id)?;
        let text = if item.detail.trim().is_empty() {
            item.label.clone()
        } else {
            format!("{} · {}", item.label, item.detail)
        };
        Some((item.progress, text))
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        let changed = match item.status {
            ExportQueueStatus::Waiting => {
                item.status = ExportQueueStatus::Cancelled;
                item.requires_resume = false;
                item.detail = "Cancelled before processing".to_owned();
                true
            }
            ExportQueueStatus::Processing => {
                self.stop_after_current = true;
                item.detail =
                    "Stop after current requested · current atomic export will finish safely"
                        .to_owned();
                true
            }
            _ => false,
        };
        if changed {
            self.persist();
        }
        changed
    }

    pub fn retry(&mut self, id: u64) -> bool {
        let reserved = self.reserved_destination_keys();
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if !matches!(
            item.status,
            ExportQueueStatus::Failed | ExportQueueStatus::Cancelled
        ) {
            return false;
        }
        if reserved.contains(&path_safety::path_key(&item.destination)) {
            item.error = Some("Destination is reserved by another queued export.".to_owned());
            return false;
        }
        item.status = ExportQueueStatus::Waiting;
        item.requires_resume = false;
        item.progress = 0.0;
        item.detail.clear();
        item.error = None;
        self.persist();
        true
    }

    pub fn cancel_all_waiting(&mut self) {
        let mut changed = false;
        for item in &mut self.items {
            if item.status == ExportQueueStatus::Waiting {
                item.status = ExportQueueStatus::Cancelled;
                item.requires_resume = false;
                item.detail = "Cancelled before processing".to_owned();
                changed = true;
            }
        }
        if changed {
            self.persist();
        }
    }

    pub fn clear_finished(&mut self) {
        self.items.retain(|item| !item.status.finished());
        self.persist();
    }

    pub fn poll(&mut self) -> Vec<ExportQueueCompletion> {
        let mut completions = Vec::new();
        let mut changed = false;
        while let Ok(event) = self.rx.try_recv() {
            match event {
                ExportQueueEvent::Progress {
                    id,
                    fraction,
                    detail,
                } => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        item.progress = fraction.clamp(0.0, 1.0);
                        item.detail = detail;
                    }
                }
                ExportQueueEvent::Finished {
                    id,
                    project_session_id,
                    result,
                    mark,
                } => {
                    self.active_id = None;
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        item.progress = 1.0;
                        match &result {
                            Ok(message) => {
                                item.status = ExportQueueStatus::Done;
                                item.detail = message.clone();
                                item.error = None;
                            }
                            Err(err) => {
                                item.status = ExportQueueStatus::Failed;
                                item.detail = "Export failed".to_owned();
                                item.error = Some(err.clone());
                            }
                        }
                    }
                    completions.push(ExportQueueCompletion {
                        id,
                        project_session_id,
                        result,
                        mark,
                    });
                    changed = true;
                    if self.stop_after_current {
                        self.stop_after_current = false;
                        for item in &mut self.items {
                            if item.status == ExportQueueStatus::Waiting {
                                item.status = ExportQueueStatus::Cancelled;
                                item.detail =
                                    "Cancelled after current export completed safely".to_owned();
                            }
                        }
                    }
                }
            }
        }

        if self.active_id.is_none() && !self.stop_after_current {
            changed |= self.start_next();
        }
        if changed {
            self.persist();
        }
        completions
    }

    fn start_next(&mut self) -> bool {
        let Some(index) = self
            .items
            .iter()
            .position(|item| item.status == ExportQueueStatus::Waiting && !item.requires_resume)
        else {
            return false;
        };
        let mut queued = self.items[index].spec.clone();
        let id = self.items[index].id;
        let session_id = queued.project_session_id;

        let preflight = validate_destination(&queued.export.destination, &queued.protected_sources)
            .and_then(|_| {
                if let Some(fingerprint) = &queued.source_fingerprint {
                    fingerprint.verify(&queued.export.source)?;
                }
                Ok(())
            });
        if let Err(err) = preflight {
            return self.finish_preflight_error(index, id, session_id, err);
        }

        if queued.export.destination.exists() {
            match queued.export.conflict_policy {
                ConflictPolicy::Overwrite => {}
                ConflictPolicy::Skip => {
                    self.items[index].status = ExportQueueStatus::Processing;
                    self.active_id = Some(id);
                    let tx = self.tx.clone();
                    thread::spawn(move || {
                        let _ = tx.send(ExportQueueEvent::Finished {
                            id,
                            project_session_id: session_id,
                            result: Ok("Skipped · destination already exists".to_owned()),
                            mark: None,
                        });
                    });
                    return true;
                }
                ConflictPolicy::AutoNumber => {
                    let folder = queued
                        .export
                        .destination
                        .parent()
                        .unwrap_or_else(|| Path::new("."));
                    let filename = queued
                        .export
                        .destination
                        .file_name()
                        .map(|value| value.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "shade-export.tif".to_owned());
                    let mut reserved = self
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(other_index, item)| {
                            *other_index != index
                                && matches!(
                                    item.status,
                                    ExportQueueStatus::Waiting | ExportQueueStatus::Processing
                                )
                        })
                        .map(|(_, item)| path_safety::path_key(&item.destination))
                        .collect::<BTreeSet<_>>();
                    if let DestinationDecision::Write(path) =
                        export_batch::resolve_destination_reserved(
                            folder,
                            &filename,
                            ConflictPolicy::AutoNumber,
                            &mut reserved,
                        )
                    {
                        queued.export.destination = path.clone();
                        self.items[index].destination = path;
                        self.items[index].spec = queued.clone();
                    }
                }
            }
        }

        if let Err(err) =
            validate_destination(&queued.export.destination, &queued.protected_sources)
        {
            return self.finish_preflight_error(index, id, session_id, err);
        }

        self.items[index].status = ExportQueueStatus::Processing;
        self.items[index].requires_resume = false;
        self.items[index].progress = 0.0;
        self.items[index].detail = "Starting".to_owned();
        self.items[index].error = None;
        self.active_id = Some(id);

        let spec = queued.export;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let validate_after_export = spec.validate_after_export;
            let progress_tx = tx.clone();
            let project = spec.recipe.materialize_project();
            let result = export::export_face_with_progress_options(
                &spec.source,
                &spec.destination,
                &project,
                spec.default_dpi,
                export::ExportOptions {
                    force_lzw: spec.force_lzw,
                },
                move |fraction, detail| {
                    let _ = progress_tx.send(ExportQueueEvent::Progress {
                        id,
                        fraction: if validate_after_export {
                            fraction * 0.90
                        } else {
                            fraction
                        },
                        detail: detail.to_owned(),
                    });
                },
            )
            .and_then(|_| {
                if spec.validate_after_export {
                    let _ = tx.send(ExportQueueEvent::Progress {
                        id,
                        fraction: 0.94,
                        detail: "Validating exported TIFF".to_owned(),
                    });
                    let verified = validation::validate_export_transport_with_options(
                        &spec.source,
                        &spec.destination,
                        spec.force_lzw,
                    )?;
                    Ok(format!("Done · {verified}"))
                } else {
                    Ok("Done".to_owned())
                }
            });
            let mark = result.as_ref().ok().and(spec.mark);
            let _ = tx.send(ExportQueueEvent::Finished {
                id,
                project_session_id: session_id,
                result,
                mark,
            });
        });
        true
    }

    fn finish_preflight_error(
        &mut self,
        index: usize,
        id: u64,
        session_id: u64,
        err: String,
    ) -> bool {
        self.items[index].status = ExportQueueStatus::Processing;
        self.active_id = Some(id);
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(ExportQueueEvent::Finished {
                id,
                project_session_id: session_id,
                result: Err(err),
                mark: None,
            });
        });
        true
    }
}

fn queue_persistence_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ShadeEditor")
        .join("export-queue.json")
}

fn validate_destination(destination: &Path, sources: &[PathBuf]) -> Result<(), String> {
    if let Some(source) = path_safety::conflicts_with_any_source(destination, sources) {
        return Err(format!(
            "Refusing export: destination resolves to a source TIFF. Source: {} · Destination: {}",
            source.display(),
            destination.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ShadeProject;

    fn spec(destination: &str) -> ExportQueueSpec {
        ExportQueueSpec {
            label: "test".to_owned(),
            source: PathBuf::from("missing.tif"),
            destination: PathBuf::from(destination),
            recipe: ExportRecipe::from_project(&ShadeProject::default()),
            default_dpi: 220.0,
            force_lzw: true,
            validate_after_export: false,
            conflict_policy: ConflictPolicy::AutoNumber,
            mark: None,
        }
    }

    fn temp_folder(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-queue-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn waiting_item_can_be_cancelled_and_retried_without_io() {
        let mut queue = ExportQueue::new();
        let id = queue.enqueue(spec("out.tif"));
        assert!(queue.cancel(id));
        assert_eq!(queue.items()[0].status, ExportQueueStatus::Cancelled);
        assert!(queue.retry(id));
        assert_eq!(queue.items()[0].status, ExportQueueStatus::Waiting);
    }

    #[test]
    fn queued_destination_cannot_target_a_protected_source() {
        let folder = temp_folder("protected");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.tif");
        std::fs::write(&source, b"TIFF source fixture").unwrap();
        let mut item = spec(source.to_str().unwrap());
        item.source = source.clone();
        let mut queue = ExportQueue::new();
        let err = queue
            .enqueue_for_project(item, vec![source], 7)
            .unwrap_err();
        assert!(err.contains("source TIFF"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn pending_destinations_are_globally_reserved() {
        let folder = temp_folder("reserved");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.tif");
        std::fs::write(&source, b"source").unwrap();
        let destination = folder.join("same.tif");
        let mut first = spec(destination.to_str().unwrap());
        first.source = source.clone();
        let second = first.clone();
        let mut queue = ExportQueue::new();
        queue
            .enqueue_for_project(first, vec![source.clone()], 1)
            .unwrap();
        let err = queue
            .enqueue_for_project(second, vec![source], 1)
            .unwrap_err();
        assert!(err.contains("already reserved"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn source_fingerprint_detects_mutation() {
        let folder = temp_folder("fingerprint");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.tif");
        std::fs::write(&source, b"first").unwrap();
        let fingerprint = SourceFingerprint::capture(&source).unwrap();
        std::fs::write(&source, b"second version").unwrap();
        assert!(fingerprint.verify(&source).is_err());
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn persistent_processing_item_recovers_as_waiting_without_snapshot_mark() {
        let folder = temp_folder("persist");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.tif");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = folder.join("out.tif");
        let path = folder.join("queue.json");
        let mut queue = ExportQueue::empty(Some(path.clone()));
        let mut queued = spec(destination.to_str().unwrap());
        queued.source = source.clone();
        queued.mark = Some(ExportQueueMark {
            snapshot_id: 9,
            face_key: "face".into(),
            folder: folder.clone(),
        });
        queue.enqueue_for_project(queued, vec![source], 55).unwrap();
        queue.items[0].status = ExportQueueStatus::Processing;
        queue.persist();
        drop(queue);

        let restored = ExportQueue::load_from_path(path).unwrap();
        assert_eq!(restored.items[0].status, ExportQueueStatus::Waiting);
        assert!(restored.items[0].restored);
        assert!(restored.items[0].requires_resume);
        assert_eq!(restored.pending_count(), 0);
        assert_eq!(restored.recovered_waiting_count(), 1);
        assert!(restored.items[0].spec.export.mark.is_none());
        assert_eq!(restored.items[0].spec.project_session_id, 0);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn restored_waiting_work_requires_explicit_resume() {
        let folder = temp_folder("paused-restore");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.tif");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = folder.join("out.tif");
        let path = folder.join("queue.json");
        let mut queue = ExportQueue::empty(Some(path.clone()));
        let mut queued = spec(destination.to_str().unwrap());
        queued.source = source.clone();
        let id = queue.enqueue_for_project(queued, vec![source], 55).unwrap();
        queue.persist();
        drop(queue);

        let mut restored = ExportQueue::load_from_path(path).unwrap();
        assert_eq!(restored.pending_count(), 0);
        assert!(restored.active_id.is_none());
        assert!(restored.poll().is_empty());
        assert!(restored.active_id.is_none());
        assert!(restored.resume(id));
        assert_eq!(restored.pending_count(), 1);
        let _ = std::fs::remove_dir_all(folder);
    }
}
