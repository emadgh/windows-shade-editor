use std::any::Any;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use serde::{Deserialize, Serialize};

use crate::conversion_transaction::{
    CommittedConversionOutput, CompletedConversionTransaction, ConversionCancellation,
    ConversionJobCapture, ConversionPhase, ConversionTransactionOutcome,
    run_conversion_transaction,
};
use crate::icc_conversion_worker::FilesystemIccConversionBackend;
use crate::icc_conversion_worker::sha256_file;
use crate::model::ShadeProject;
use crate::production_project::link_source_project_to_production;
use crate::safe_fs;

const QUEUE_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionQueueStatus {
    Waiting,
    Processing,
    Done,
    Failed,
    Cancelled,
    NeedsRecovery,
}

impl ConversionQueueStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::Processing => "Processing",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::NeedsRecovery => "Needs recovery",
        }
    }

    fn reserves_destination(self) -> bool {
        matches!(self, Self::Waiting | Self::Processing | Self::NeedsRecovery)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionRecoveryRecord {
    pub committed_output: CommittedConversionOutput,
    pub production_project_path: PathBuf,
    pub production_project: Option<ShadeProject>,
    pub error: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct QueuedConversionSpec {
    capture: ConversionJobCapture,
    default_dpi: f64,
}

#[derive(Clone, Debug)]
pub struct ConversionQueueItem {
    pub id: u64,
    pub label: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub production_project_path: PathBuf,
    pub status: ConversionQueueStatus,
    pub phase: String,
    pub progress: f32,
    pub detail: String,
    pub error: Option<String>,
    pub recovery: Option<ConversionRecoveryRecord>,
    pub restored: bool,
    pub requires_resume: bool,
    spec: QueuedConversionSpec,
}

#[derive(Clone, Debug)]
pub struct ConversionQueueCompletion {
    pub id: u64,
    pub capture: ConversionJobCapture,
    pub result: ConversionQueueCompletionResult,
}

#[derive(Clone, Debug)]
pub enum ConversionQueueCompletionResult {
    Completed(CompletedConversionTransaction),
    Cancelled { phase: String, message: String },
    Failed { phase: String, error: String },
    NeedsRecovery(ConversionRecoveryRecord),
}

enum ConversionQueueEvent {
    Progress {
        id: u64,
        phase: String,
        fraction: f32,
        detail: String,
    },
    Finished {
        id: u64,
        capture: ConversionJobCapture,
        result: ConversionQueueCompletionResult,
    },
}

#[derive(Serialize, Deserialize)]
struct PersistedQueue {
    format_version: u32,
    next_id: u64,
    paused: bool,
    items: Vec<PersistedQueueItem>,
}

#[derive(Serialize, Deserialize)]
struct PersistedQueueItem {
    id: u64,
    status: ConversionQueueStatus,
    spec: QueuedConversionSpec,
    error: Option<String>,
    recovery: Option<ConversionRecoveryRecord>,
}

pub struct ConversionQueue {
    items: Vec<ConversionQueueItem>,
    next_id: u64,
    active_id: Option<u64>,
    active_cancellation: Option<ConversionCancellation>,
    paused: bool,
    tx: mpsc::Sender<ConversionQueueEvent>,
    rx: mpsc::Receiver<ConversionQueueEvent>,
    persistence_path: Option<PathBuf>,
    last_persistence_error: Option<String>,
}

impl Default for ConversionQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversionQueue {
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
            active_cancellation: None,
            paused: false,
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
        let bytes = fs::read(&path)
            .map_err(|err| format!("Cannot read conversion queue {}: {err}", path.display()))?;
        let persisted: PersistedQueue = serde_json::from_slice(&bytes)
            .map_err(|err| format!("Invalid conversion queue {}: {err}", path.display()))?;
        if persisted.format_version != QUEUE_FORMAT_VERSION {
            return Err(format!(
                "Unsupported conversion queue format {} (expected {}).",
                persisted.format_version, QUEUE_FORMAT_VERSION
            ));
        }
        queue.next_id = persisted.next_id.max(1);
        queue.paused = persisted.paused;
        for saved in persisted.items {
            if saved.status == ConversionQueueStatus::Done {
                continue;
            }
            let recovered_work = matches!(
                saved.status,
                ConversionQueueStatus::Waiting | ConversionQueueStatus::Processing
            );
            let status = if saved.status == ConversionQueueStatus::Processing {
                ConversionQueueStatus::Waiting
            } else {
                saved.status
            };
            queue.items.push(item_from_spec(
                saved.id,
                saved.spec,
                status,
                true,
                recovered_work,
                saved.error,
                saved.recovery,
            ));
        }
        Ok(queue)
    }

    pub fn enqueue(
        &mut self,
        capture: ConversionJobCapture,
        default_dpi: f64,
    ) -> Result<u64, String> {
        capture.validate()?;
        if !default_dpi.is_finite() || default_dpi <= 0.0 {
            return Err("Conversion fallback DPI must be finite and positive.".to_owned());
        }
        for item in self
            .items
            .iter()
            .filter(|item| item.status.reserves_destination())
        {
            if paths_match(&item.destination, &capture.output_tiff_path)
                || paths_match(
                    &item.production_project_path,
                    &capture.production_project_path,
                )
            {
                return Err("Conversion output or Production project is already reserved by another queued job.".to_owned());
            }
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.items.push(item_from_spec(
            id,
            QueuedConversionSpec {
                capture,
                default_dpi,
            },
            ConversionQueueStatus::Waiting,
            false,
            false,
            None,
            None,
        ));
        self.persist();
        Ok(id)
    }

    pub fn items(&self) -> &[ConversionQueueItem] {
        &self.items
    }

    pub fn has_pending(&self) -> bool {
        self.items.iter().any(|item| {
            item.status == ConversionQueueStatus::Processing
                || (item.status == ConversionQueueStatus::Waiting && !item.requires_resume)
        })
    }

    pub fn is_active(&self) -> bool {
        self.active_id.is_some()
    }

    pub fn recovered_waiting_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == ConversionQueueStatus::Waiting && item.requires_resume)
            .count()
    }

    pub fn active_summary(&self) -> Option<(f32, String)> {
        let id = self.active_id?;
        let item = self.items.iter().find(|item| item.id == id)?;
        let text = if item.detail.trim().is_empty() {
            format!("Conversion #{} - {}", item.id, item.phase)
        } else {
            format!("Conversion #{} - {} - {}", item.id, item.phase, item.detail)
        };
        Some((finite_progress(item.progress), text))
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.paused != paused {
            self.paused = paused;
            self.persist();
        }
    }

    pub fn resume_recovered(&mut self) -> usize {
        let mut count = 0;
        for item in &mut self.items {
            if item.status == ConversionQueueStatus::Waiting && item.requires_resume {
                item.requires_resume = false;
                item.detail.clear();
                count += 1;
            }
        }
        if count > 0 {
            self.persist();
        }
        count
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        match item.status {
            ConversionQueueStatus::Waiting => {
                item.status = ConversionQueueStatus::Cancelled;
                item.requires_resume = false;
                item.detail = "Cancelled before processing".to_owned();
                self.persist();
                true
            }
            ConversionQueueStatus::Processing if self.active_id == Some(id) => {
                if let Some(cancellation) = &self.active_cancellation {
                    cancellation.request();
                }
                item.detail = "Cancellation requested; safe commit boundary is enforced".to_owned();
                true
            }
            _ => false,
        }
    }

    pub fn retry(&mut self, id: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if !matches!(
            item.status,
            ConversionQueueStatus::Failed
                | ConversionQueueStatus::Cancelled
                | ConversionQueueStatus::NeedsRecovery
        ) {
            return false;
        }
        item.status = ConversionQueueStatus::Waiting;
        item.progress = 0.0;
        item.phase.clear();
        item.detail.clear();
        item.error = None;
        item.recovery = None;
        item.requires_resume = false;
        self.persist();
        true
    }

    pub fn clear_finished(&mut self) -> usize {
        let before = self.items.len();
        self.items.retain(|item| {
            !matches!(
                item.status,
                ConversionQueueStatus::Done
                    | ConversionQueueStatus::Failed
                    | ConversionQueueStatus::Cancelled
            )
        });
        let removed = before - self.items.len();
        if removed > 0 {
            self.persist();
        }
        removed
    }

    pub fn take_persistence_error(&mut self) -> Option<String> {
        self.last_persistence_error.take()
    }

    pub fn poll(&mut self) -> Vec<ConversionQueueCompletion> {
        self.poll_with_start(true)
    }

    pub fn poll_with_start(&mut self, allow_start: bool) -> Vec<ConversionQueueCompletion> {
        let mut completions = Vec::new();
        let mut changed = false;
        while let Ok(event) = self.rx.try_recv() {
            match event {
                ConversionQueueEvent::Progress {
                    id,
                    phase,
                    fraction,
                    detail,
                } => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        item.phase = phase;
                        item.progress = finite_progress(fraction);
                        item.detail = detail;
                    }
                }
                ConversionQueueEvent::Finished {
                    id,
                    capture,
                    result,
                } => {
                    self.active_id = None;
                    self.active_cancellation = None;
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        apply_completion(item, &result);
                    }
                    completions.push(ConversionQueueCompletion {
                        id,
                        capture,
                        result,
                    });
                    changed = true;
                }
            }
        }
        if allow_start && self.active_id.is_none() && !self.paused {
            changed |= self.start_next();
        }
        if changed {
            self.persist();
        }
        completions
    }

    fn start_next(&mut self) -> bool {
        let Some(index) = self.items.iter().position(|item| {
            item.status == ConversionQueueStatus::Waiting && !item.requires_resume
        }) else {
            return false;
        };
        let id = self.items[index].id;
        let spec = self.items[index].spec.clone();
        let cancellation = ConversionCancellation::default();
        self.items[index].status = ConversionQueueStatus::Processing;
        self.items[index].progress = 0.0;
        self.items[index].phase = ConversionPhase::CaptureValidation.label().to_owned();
        self.items[index].detail = "Starting production conversion".to_owned();
        self.items[index].error = None;
        self.active_id = Some(id);
        self.active_cancellation = Some(cancellation.clone());

        let tx = self.tx.clone();
        thread::spawn(move || {
            let capture = spec.capture;
            let worker_tx = tx.clone();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let mut backend = match FilesystemIccConversionBackend::new(spec.default_dpi) {
                    Ok(backend) => backend,
                    Err(error) => {
                        return ConversionTransactionOutcome::FailedBeforeCommit {
                            phase: ConversionPhase::CaptureValidation,
                            error,
                        };
                    }
                };
                run_conversion_transaction(&capture, &cancellation, &mut backend, |progress| {
                    let _ = worker_tx.send(ConversionQueueEvent::Progress {
                        id,
                        phase: progress.phase.label().to_owned(),
                        fraction: progress.fraction,
                        detail: progress.detail,
                    });
                })
            }))
            .unwrap_or_else(|payload| {
                ConversionTransactionOutcome::FailedBeforeCommit {
                    phase: ConversionPhase::CaptureValidation,
                    error: format!(
                        "Conversion worker panicked: {}",
                        panic_payload_text(payload.as_ref())
                    ),
                }
            });
            let result = completion_result(&capture, outcome);
            let _ = tx.send(ConversionQueueEvent::Finished {
                id,
                capture,
                result,
            });
        });
        true
    }

    fn persist(&mut self) {
        let Some(path) = self.persistence_path.clone() else {
            return;
        };
        let items = self
            .items
            .iter()
            .filter(|item| item.status != ConversionQueueStatus::Done)
            .map(|item| PersistedQueueItem {
                id: item.id,
                status: item.status,
                spec: item.spec.clone(),
                error: item.error.clone(),
                recovery: item.recovery.clone(),
            })
            .collect();
        let persisted = PersistedQueue {
            format_version: QUEUE_FORMAT_VERSION,
            next_id: self.next_id,
            paused: self.paused,
            items,
        };
        let result = serde_json::to_vec_pretty(&persisted)
            .map_err(|err| format!("Cannot serialize conversion queue: {err}"))
            .and_then(|bytes| safe_fs::atomic_write(&path, &bytes, None));
        if let Err(error) = result {
            self.last_persistence_error = Some(error);
        }
    }
}

fn item_from_spec(
    id: u64,
    spec: QueuedConversionSpec,
    status: ConversionQueueStatus,
    restored: bool,
    requires_resume: bool,
    error: Option<String>,
    recovery: Option<ConversionRecoveryRecord>,
) -> ConversionQueueItem {
    ConversionQueueItem {
        id,
        label: spec.capture.output_face_label.clone(),
        source: spec.capture.source_face_path.clone(),
        destination: spec.capture.output_tiff_path.clone(),
        production_project_path: spec.capture.production_project_path.clone(),
        status,
        phase: String::new(),
        progress: 0.0,
        detail: if requires_resume {
            "Recovered from previous session; paused until explicitly resumed".to_owned()
        } else {
            String::new()
        },
        error,
        recovery,
        restored,
        requires_resume,
        spec,
    }
}

fn completion_result(
    capture: &ConversionJobCapture,
    outcome: ConversionTransactionOutcome,
) -> ConversionQueueCompletionResult {
    match outcome {
        ConversionTransactionOutcome::Completed(value) => {
            match commit_source_project_link(capture, &value) {
                Ok(()) => ConversionQueueCompletionResult::Completed(value),
                Err(error) => {
                    ConversionQueueCompletionResult::NeedsRecovery(ConversionRecoveryRecord {
                        committed_output: value.committed_output,
                        production_project_path: value.production_project_path,
                        production_project: Some(value.production_project),
                        error,
                    })
                }
            }
        }
        ConversionTransactionOutcome::CancelledBeforeCommit { phase, message } => {
            ConversionQueueCompletionResult::Cancelled {
                phase: phase.label().to_owned(),
                message,
            }
        }
        ConversionTransactionOutcome::FailedBeforeCommit { phase, error } => {
            ConversionQueueCompletionResult::Failed {
                phase: phase.label().to_owned(),
                error,
            }
        }
        ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            committed_output,
            production_project_path,
            production_project,
            error,
        } => ConversionQueueCompletionResult::NeedsRecovery(ConversionRecoveryRecord {
            committed_output,
            production_project_path,
            production_project,
            error,
        }),
    }
}

fn commit_source_project_link(
    capture: &ConversionJobCapture,
    completed: &CompletedConversionTransaction,
) -> Result<(), String> {
    let current_hash = sha256_file(&capture.source_project_path)?;
    if !current_hash.eq_ignore_ascii_case(capture.source_project_file_sha256.trim()) {
        return Err(
            "Production output/project committed, but the Source project changed after capture; reciprocal link was not written."
                .to_owned(),
        );
    }
    let mut source = ShadeProject::load(&capture.source_project_path)?;
    let face_paths = source.resolve_face_paths(&capture.source_project_path);
    link_source_project_to_production(&mut source, &completed.production_project_path)?;
    source
        .save(&capture.source_project_path, &face_paths)
        .map_err(|error| {
            format!(
                "Production output/project committed, but the Source project link could not be saved: {error}"
            )
        })
}

fn apply_completion(item: &mut ConversionQueueItem, result: &ConversionQueueCompletionResult) {
    item.progress = 1.0;
    item.requires_resume = false;
    match result {
        ConversionQueueCompletionResult::Completed(_) => {
            item.status = ConversionQueueStatus::Done;
            item.phase = ConversionPhase::Complete.label().to_owned();
            item.detail = "Production TIFF and project committed".to_owned();
            item.error = None;
            item.recovery = None;
        }
        ConversionQueueCompletionResult::Cancelled { phase, message } => {
            item.status = ConversionQueueStatus::Cancelled;
            item.phase = phase.clone();
            item.detail = "Cancelled before output commit".to_owned();
            item.error = Some(message.clone());
            item.recovery = None;
        }
        ConversionQueueCompletionResult::Failed { phase, error } => {
            item.status = ConversionQueueStatus::Failed;
            item.phase = phase.clone();
            item.detail = "Conversion failed before output commit".to_owned();
            item.error = Some(error.clone());
            item.recovery = None;
        }
        ConversionQueueCompletionResult::NeedsRecovery(recovery) => {
            item.status = ConversionQueueStatus::NeedsRecovery;
            item.phase = ConversionPhase::ProductionProjectSave.label().to_owned();
            item.detail = "Production TIFF committed; project recovery required".to_owned();
            item.error = Some(recovery.error.clone());
            item.recovery = Some(recovery.clone());
        }
    }
}

fn finite_progress(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

fn queue_persistence_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ShadeEditor")
        .join("conversion-queue.json")
}

fn panic_payload_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
        TargetChannelDefinition,
    };
    use crate::conversion_transaction::{CapturedOutputPolicy, CapturedSourceProfile};
    use crate::model::IccProfileIdentity;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn capture(output: &str) -> ConversionJobCapture {
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "sRGB".to_owned(),
                sha256: HASH.to_owned(),
            },
            target: ConversionTargetDefinition {
                name: "Press".to_owned(),
                channels: ["Cyan", "Magenta", "Yellow", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: None,
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Press".to_owned(),
                    sha256: HASH.to_owned(),
                }),
                output_profile_path: Some(r"C:\Color\Press.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            strategy: SeparationStrategy::default(),
        };
        ConversionJobCapture::capture(
            &ShadeProject::default(),
            PathBuf::from(r"C:\Design\Source.shade"),
            HASH.to_owned(),
            PathBuf::from(r"C:\Design\Face.tif"),
            None,
            HASH.to_owned(),
            CapturedSourceProfile::Embedded,
            recipe,
            CapturedOutputPolicy::MustNotExist,
            PathBuf::from(output),
            PathBuf::from(format!("{output}.shade")),
            "Production".to_owned(),
            "Converted Face".to_owned(),
        )
        .unwrap()
    }

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-conversion-queue-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn waiting_conversion_can_be_cancelled_and_retried() {
        let mut queue = ConversionQueue::new();
        let id = queue
            .enqueue(capture(r"C:\Production\out.tif"), 220.0)
            .unwrap();
        assert!(queue.cancel(id));
        assert_eq!(queue.items()[0].status, ConversionQueueStatus::Cancelled);
        assert!(queue.retry(id));
        assert_eq!(queue.items()[0].status, ConversionQueueStatus::Waiting);
    }

    #[test]
    fn duplicate_output_reservation_is_rejected() {
        let mut queue = ConversionQueue::new();
        queue
            .enqueue(capture(r"C:\Production\out.tif"), 220.0)
            .unwrap();
        assert!(
            queue
                .enqueue(capture(r"C:\Production\OUT.TIF"), 220.0)
                .is_err()
        );
    }

    #[test]
    fn recovered_waiting_work_requires_explicit_resume() {
        let path = temp_path("resume");
        let mut queue = ConversionQueue::empty(Some(path.clone()));
        queue
            .enqueue(capture(r"C:\Production\out.tif"), 220.0)
            .unwrap();
        drop(queue);
        let mut restored = ConversionQueue::load_from_path(path.clone()).unwrap();
        assert_eq!(restored.recovered_waiting_count(), 1);
        assert!(!restored.has_pending());
        assert_eq!(restored.resume_recovered(), 1);
        assert!(restored.has_pending());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn completed_conversion_links_only_the_unchanged_source_project() {
        let source_path = temp_path("source").with_extension("shade");
        let production_path = temp_path("production").with_extension("shade");
        let source = ShadeProject::default();
        source.save(&source_path, &[]).unwrap();
        let source_hash = sha256_file(&source_path).unwrap();
        let mut captured = capture(r"C:\Production\out.tif");
        captured.source_project_path = source_path.clone();
        captured.source_project_file_sha256 = source_hash;
        let completed = CompletedConversionTransaction {
            committed_output: CommittedConversionOutput {
                path: PathBuf::from(r"C:\Production\out.tif"),
                sha256: HASH.to_owned(),
                converted_at_unix_ms: 1,
            },
            production_project_path: production_path.clone(),
            production_project: ShadeProject::default(),
        };

        commit_source_project_link(&captured, &completed).unwrap();
        let linked = ShadeProject::load(&source_path).unwrap();
        assert_eq!(linked.linked_projects.len(), 1);
        assert_eq!(
            linked.linked_projects[0].path,
            production_path.display().to_string()
        );
        assert!(commit_source_project_link(&captured, &completed).is_err());
        let _ = fs::remove_file(source_path);
    }
}
