use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::conversion_batch::{ConversionBatchCapture, ConversionBatchCheckpoint};
use crate::conversion_batch_execution::{
    ConversionBatchStepOutcome, run_next_conversion_batch_face,
};
use crate::conversion_recovery::{
    ConversionRecoveryRecord, ConversionRecoveryStage, recover_production_project,
};
use crate::conversion_transaction::{
    CompletedConversionTransaction, ConversionCancellation, ConversionJobCapture, ConversionPhase,
    ConversionTransactionOutcome,
};
use crate::icc_conversion_worker::FilesystemIccConversionBackend;
use crate::production_project_disposition::ProductionProjectDisposition;
use crate::queue_core::{
    PersistedQueueEnvelope, QueueLifecycle, QueueRuntime, load_persisted_queue,
    sanitize_progress, write_persisted_queue,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionBatchQueueStatus {
    Waiting,
    Processing,
    Done,
    Failed,
    Cancelled,
    NeedsRecovery,
}

impl ConversionBatchQueueStatus {
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

    fn common_lifecycle(self) -> Option<QueueLifecycle> {
        match self {
            Self::Waiting => Some(QueueLifecycle::Waiting),
            Self::Processing => Some(QueueLifecycle::Processing),
            Self::Done => Some(QueueLifecycle::Done),
            Self::Failed => Some(QueueLifecycle::Failed),
            Self::Cancelled => Some(QueueLifecycle::Cancelled),
            Self::NeedsRecovery => None,
        }
    }

    fn restored(self) -> (Self, bool) {
        let Some(common) = self.common_lifecycle() else {
            return (Self::NeedsRecovery, false);
        };
        let (status, requires_resume) = common.restored();
        let status = match status {
            QueueLifecycle::Waiting => Self::Waiting,
            QueueLifecycle::Processing => Self::Processing,
            QueueLifecycle::Done => Self::Done,
            QueueLifecycle::Failed => Self::Failed,
            QueueLifecycle::Cancelled => Self::Cancelled,
        };
        (status, requires_resume)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct QueuedConversionBatchSpec {
    batch: ConversionBatchCapture,
    default_dpi: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionBatchRecoveryRecord {
    pub source_face_index: usize,
    pub ordinal: usize,
    #[serde(default)]
    pub disposition: Option<ProductionProjectDisposition>,
    pub recovery: ConversionRecoveryRecord,
}

#[derive(Clone, Debug)]
pub struct ConversionBatchQueueItem {
    pub id: u64,
    pub label: String,
    pub source_project_path: PathBuf,
    pub production_project_path: PathBuf,
    pub current_source: Option<PathBuf>,
    pub current_destination: Option<PathBuf>,
    pub face_count: usize,
    pub completed_face_count: usize,
    pub status: ConversionBatchQueueStatus,
    pub phase: String,
    pub progress: f32,
    pub detail: String,
    pub error: Option<String>,
    pub recovery: Option<ConversionBatchRecoveryRecord>,
    pub restored: bool,
    pub requires_resume: bool,
    checkpoint: ConversionBatchCheckpoint,
    spec: QueuedConversionBatchSpec,
}

#[derive(Clone, Debug)]
pub struct ConversionBatchQueueCompletion {
    pub id: u64,
    pub source_face_index: usize,
    pub result: ConversionBatchQueueCompletionResult,
}

#[derive(Clone, Debug)]
pub enum ConversionBatchQueueCompletionResult {
    CompletedFace {
        completed: CompletedConversionTransaction,
        ordinal: usize,
        batch_complete: bool,
    },
    Cancelled {
        phase: String,
        message: String,
    },
    Failed {
        phase: String,
        error: String,
    },
    NeedsRecovery(ConversionBatchRecoveryRecord),
}

enum ConversionBatchQueueEvent {
    Progress {
        id: u64,
        phase: String,
        fraction: f32,
        detail: String,
    },
    Finished {
        id: u64,
        outcome: ConversionBatchStepOutcome,
    },
}

#[derive(Serialize, Deserialize)]
struct PersistedBatchQueueItem {
    id: u64,
    status: ConversionBatchQueueStatus,
    spec: QueuedConversionBatchSpec,
    #[serde(default)]
    checkpoint: ConversionBatchCheckpoint,
    error: Option<String>,
    recovery: Option<ConversionBatchRecoveryRecord>,
}

pub struct ConversionBatchQueue {
    items: Vec<ConversionBatchQueueItem>,
    runtime: QueueRuntime<ConversionBatchQueueEvent>,
    active_cancellation: Option<ConversionCancellation>,
}

impl Default for ConversionBatchQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversionBatchQueue {
    pub fn new() -> Self {
        Self::empty(None)
    }

    pub fn load_persistent() -> Result<Self, String> {
        Self::load_from_path(batch_queue_persistence_path())
    }

    fn empty(persistence_path: Option<PathBuf>) -> Self {
        Self {
            items: Vec::new(),
            runtime: QueueRuntime::new(persistence_path),
            active_cancellation: None,
        }
    }

    fn load_from_path(path: PathBuf) -> Result<Self, String> {
        let Some(persisted) =
            load_persisted_queue::<PersistedBatchQueueItem>(&path, "conversion batch")?
        else {
            return Ok(Self::empty(Some(path)));
        };
        let mut queue = Self {
            items: Vec::new(),
            runtime: QueueRuntime::restore_runtime(
                Some(path),
                persisted.next_id,
                persisted.paused,
            ),
            active_cancellation: None,
        };
        for saved in persisted.items {
            if saved.status == ConversionBatchQueueStatus::Done {
                continue;
            }
            saved.spec.batch.validate()?;
            saved.checkpoint.validate_for(&saved.spec.batch)?;
            let (status, requires_resume) = saved.status.restored();
            queue.items.push(item_from_spec(
                saved.id,
                saved.spec,
                saved.checkpoint,
                status,
                true,
                requires_resume,
                saved.error,
                saved.recovery,
            ));
        }
        Ok(queue)
    }

    pub fn enqueue(
        &mut self,
        batch: ConversionBatchCapture,
        default_dpi: f64,
    ) -> Result<u64, String> {
        batch.validate()?;
        if !default_dpi.is_finite() || default_dpi <= 0.0 {
            return Err("Conversion batch fallback DPI must be finite and positive.".to_owned());
        }

        for item in self
            .items
            .iter()
            .filter(|item| item.status.reserves_destination())
        {
            if paths_match(&item.production_project_path, batch.production_project_path()) {
                return Err(
                    "Production project is already reserved by another conversion batch."
                        .to_owned(),
                );
            }
            for incoming in &batch.faces {
                if item.spec.batch.faces.iter().any(|existing| {
                    paths_match(
                        &existing.capture.output_tiff_path,
                        &incoming.capture.output_tiff_path,
                    )
                }) {
                    return Err(
                        "A conversion batch output TIFF is already reserved by another batch."
                            .to_owned(),
                    );
                }
            }
        }

        let id = self.runtime.allocate_id();
        self.items.push(item_from_spec(
            id,
            QueuedConversionBatchSpec { batch, default_dpi },
            ConversionBatchCheckpoint::default(),
            ConversionBatchQueueStatus::Waiting,
            false,
            false,
            None,
            None,
        ));
        self.persist();
        Ok(id)
    }

    pub fn items(&self) -> &[ConversionBatchQueueItem] {
        &self.items
    }

    pub fn has_pending(&self) -> bool {
        self.items.iter().any(|item| {
            item.status == ConversionBatchQueueStatus::Processing
                || (item.status == ConversionBatchQueueStatus::Waiting && !item.requires_resume)
        })
    }

    pub fn is_active(&self) -> bool {
        self.runtime.active_id().is_some()
    }

    pub fn is_paused(&self) -> bool {
        self.runtime.is_paused()
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.runtime.set_paused(paused) {
            self.persist();
        }
    }

    pub fn recovered_waiting_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| {
                item.status == ConversionBatchQueueStatus::Waiting && item.requires_resume
            })
            .count()
    }

    pub fn resume_recovered(&mut self) -> usize {
        let mut count = 0;
        for item in &mut self.items {
            if item.status == ConversionBatchQueueStatus::Waiting && item.requires_resume {
                item.requires_resume = false;
                item.detail = format!(
                    "Resumed at Face {} of {}",
                    item.checkpoint.completed_count() + 1,
                    item.spec.batch.face_count()
                );
                count += 1;
            }
        }
        if count > 0 {
            self.persist();
        }
        count
    }

    pub fn active_summary(&self) -> Option<(f32, String)> {
        let id = self.runtime.active_id()?;
        let item = self.items.iter().find(|item| item.id == id)?;
        Some((
            sanitize_progress(item.progress),
            format!("Batch #{} - {} - {}", item.id, item.phase, item.detail),
        ))
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        let active_id = self.runtime.active_id();
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        match item.status {
            ConversionBatchQueueStatus::Waiting => {
                item.status = ConversionBatchQueueStatus::Cancelled;
                item.requires_resume = false;
                item.detail = format!(
                    "Cancelled after {} of {} committed Faces",
                    item.checkpoint.completed_count(),
                    item.spec.batch.face_count()
                );
                self.persist();
                true
            }
            ConversionBatchQueueStatus::Processing if active_id == Some(id) => {
                if let Some(cancellation) = &self.active_cancellation {
                    cancellation.request();
                }
                item.detail =
                    "Cancellation requested; current Face safe commit boundary is enforced"
                        .to_owned();
                true
            }
            _ => false,
        }
    }

    /// Retry preserves the durable checkpoint so a failed/cancelled batch starts
    /// at the first uncommitted Face instead of rebuilding successful outputs.
    pub fn retry(&mut self, id: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if !matches!(
            item.status,
            ConversionBatchQueueStatus::Failed | ConversionBatchQueueStatus::Cancelled
        ) {
            return false;
        }
        item.status = ConversionBatchQueueStatus::Waiting;
        item.progress = item.checkpoint.completed_count() as f32 / item.spec.batch.face_count() as f32;
        item.phase.clear();
        item.detail = format!(
            "Retrying at Face {} of {}",
            item.checkpoint.completed_count() + 1,
            item.spec.batch.face_count()
        );
        item.error = None;
        item.recovery = None;
        item.requires_resume = false;
        refresh_item_cursor(item);
        self.persist();
        true
    }

    /// Recover a Face whose TIFF is already committed by replaying only the
    /// Production-project save with the exact disposition captured before the
    /// failed save. The TIFF conversion is never re-rendered here.
    pub fn recover(&mut self, id: u64) -> Result<ConversionBatchQueueCompletion, String> {
        self.recover_with(id, recover_production_project)
    }

    fn recover_with<R>(
        &mut self,
        id: u64,
        mut recover: R,
    ) -> Result<ConversionBatchQueueCompletion, String>
    where
        R: FnMut(
            &ConversionJobCapture,
            &ProductionProjectDisposition,
            &ConversionRecoveryRecord,
        ) -> Result<CompletedConversionTransaction, String>,
    {
        if self.runtime.active_id() == Some(id) {
            return Err("Active conversion batch cannot enter project-only recovery.".to_owned());
        }
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return Err("Conversion batch recovery item no longer exists.".to_owned());
        };
        if self.items[index].status != ConversionBatchQueueStatus::NeedsRecovery {
            return Err("Conversion batch does not require project recovery.".to_owned());
        }
        let recovery_record = self.items[index]
            .recovery
            .clone()
            .ok_or_else(|| "Conversion batch recovery state is missing.".to_owned())?;
        let Some(disposition) = recovery_record.disposition.clone() else {
            let error = "Batch recovery record predates exact Production disposition capture; automatic recovery is blocked. Re-run must remain manual and non-destructive.".to_owned();
            self.items[index].detail = "Exact Production recovery disposition is unavailable".to_owned();
            self.items[index].error = Some(error.clone());
            self.persist();
            return Err(error);
        };

        let ordinal = recovery_record.ordinal;
        if ordinal != self.items[index].checkpoint.completed_count() {
            return Err(
                "Batch recovery ordinal does not match the first uncommitted Face.".to_owned(),
            );
        }
        let face = self.items[index]
            .spec
            .batch
            .faces
            .get(ordinal)
            .cloned()
            .ok_or_else(|| "Batch recovery Face is outside the immutable capture.".to_owned())?;
        if face.source_face_index != recovery_record.source_face_index {
            return Err("Batch recovery Source Face identity changed.".to_owned());
        }

        let completed = match recover(&face.capture, &disposition, &recovery_record.recovery) {
            Ok(completed) => completed,
            Err(error) => {
                self.items[index].detail = format!(
                    "Face {} project-only recovery is still blocked",
                    face.source_face_index + 1
                );
                self.items[index].error = Some(error.clone());
                self.persist();
                return Err(error);
            }
        };

        let batch_complete;
        {
            let item = &mut self.items[index];
            if let Err(error) = item
                .checkpoint
                .record_committed(&item.spec.batch, &completed.committed_output)
            {
                let error = format!(
                    "Production project recovered, but batch checkpoint could not advance: {error}"
                );
                item.detail = "Recovered project is safe, but batch checkpoint remains blocked".to_owned();
                item.error = Some(error.clone());
                self.persist();
                return Err(error);
            }
            batch_complete = item.checkpoint.completed_count() == item.spec.batch.face_count();
            item.status = if batch_complete {
                ConversionBatchQueueStatus::Done
            } else {
                ConversionBatchQueueStatus::Waiting
            };
            item.progress =
                item.checkpoint.completed_count() as f32 / item.spec.batch.face_count() as f32;
            item.phase = if batch_complete {
                ConversionPhase::Complete.label().to_owned()
            } else {
                "Face recovery checkpoint committed".to_owned()
            };
            item.detail = if batch_complete {
                format!("All {} batch Faces committed", item.spec.batch.face_count())
            } else {
                format!(
                    "Face {} Production project recovered; checkpoint durable before next Face",
                    face.source_face_index + 1
                )
            };
            item.error = None;
            item.recovery = None;
            item.requires_resume = false;
            refresh_item_cursor(item);
        }
        self.persist();

        Ok(ConversionBatchQueueCompletion {
            id,
            source_face_index: face.source_face_index,
            result: ConversionBatchQueueCompletionResult::CompletedFace {
                completed,
                ordinal,
                batch_complete,
            },
        })
    }

    pub fn clear_finished(&mut self) -> usize {
        let before = self.items.len();
        self.items.retain(|item| {
            !matches!(
                item.status,
                ConversionBatchQueueStatus::Done
                    | ConversionBatchQueueStatus::Failed
                    | ConversionBatchQueueStatus::Cancelled
            )
        });
        let removed = before - self.items.len();
        if removed > 0 {
            self.persist();
        }
        removed
    }

    pub fn take_persistence_error(&mut self) -> Option<String> {
        self.runtime.take_persistence_error()
    }

    pub fn poll(&mut self) -> Vec<ConversionBatchQueueCompletion> {
        self.poll_with_start(true)
    }

    pub fn poll_with_start(&mut self, allow_start: bool) -> Vec<ConversionBatchQueueCompletion> {
        let mut completions = Vec::new();
        let mut changed = false;

        while let Ok(event) = self.runtime.try_recv() {
            match event {
                ConversionBatchQueueEvent::Progress {
                    id,
                    phase,
                    fraction,
                    detail,
                } => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        item.phase = phase;
                        item.progress = sanitize_progress(fraction);
                        item.detail = detail;
                    }
                }
                ConversionBatchQueueEvent::Finished { id, outcome } => {
                    self.runtime.set_active_id(None);
                    self.active_cancellation = None;
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        if let Some(completion) = apply_step_outcome(item, outcome) {
                            completions.push(ConversionBatchQueueCompletion { id, ..completion });
                        }
                    }
                    changed = true;
                }
            }
        }

        // The checkpoint from a completed Face must hit durable storage before
        // another Face can start. This is the crash/restart boundary for #342.
        if changed {
            self.persist();
        }

        if allow_start && self.runtime.active_id().is_none() && !self.runtime.is_paused() {
            if self.start_next() {
                self.persist();
            }
        }
        completions
    }

    fn start_next(&mut self) -> bool {
        let Some(index) = self.items.iter().position(|item| {
            item.status == ConversionBatchQueueStatus::Waiting && !item.requires_resume
        }) else {
            return false;
        };

        let id = self.items[index].id;
        let spec = self.items[index].spec.clone();
        let checkpoint = self.items[index].checkpoint.clone();
        let cancellation = ConversionCancellation::default();
        let completed = checkpoint.completed_count();
        let face_count = spec.batch.face_count();
        let pending_index = spec
            .batch
            .faces
            .get(completed)
            .map(|face| face.source_face_index)
            .unwrap_or(0);

        self.items[index].status = ConversionBatchQueueStatus::Processing;
        self.items[index].progress = completed as f32 / face_count as f32;
        self.items[index].phase = ConversionPhase::CaptureValidation.label().to_owned();
        self.items[index].detail = format!(
            "Starting Source Face {} ({} of {})",
            pending_index + 1,
            completed + 1,
            face_count
        );
        self.items[index].error = None;
        refresh_item_cursor(&mut self.items[index]);
        self.runtime.set_active_id(Some(id));
        self.active_cancellation = Some(cancellation.clone());

        let tx = self.runtime.sender();
        thread::spawn(move || {
            let worker_tx = tx.clone();
            let fallback_checkpoint = checkpoint.clone();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let mut backend = match FilesystemIccConversionBackend::new(spec.default_dpi) {
                    Ok(backend) => backend,
                    Err(error) => {
                        return ConversionBatchStepOutcome::Halted {
                            checkpoint,
                            source_face_index: pending_index,
                            disposition: None,
                            outcome: ConversionTransactionOutcome::FailedBeforeCommit {
                                phase: ConversionPhase::CaptureValidation,
                                error,
                            },
                        };
                    }
                };
                run_next_conversion_batch_face(
                    &spec.batch,
                    checkpoint,
                    &cancellation,
                    &mut backend,
                    |progress| {
                        let _ = worker_tx.send(ConversionBatchQueueEvent::Progress {
                            id,
                            phase: progress.phase.label().to_owned(),
                            fraction: progress.overall_fraction,
                            detail: format!(
                                "Face {} of {} - {}",
                                progress.ordinal + 1,
                                progress.face_count,
                                progress.detail
                            ),
                        });
                    },
                )
            }))
            .unwrap_or_else(|payload| ConversionBatchStepOutcome::Halted {
                checkpoint: fallback_checkpoint,
                source_face_index: pending_index,
                disposition: None,
                outcome: ConversionTransactionOutcome::FailedBeforeCommit {
                    phase: ConversionPhase::CaptureValidation,
                    error: format!(
                        "Conversion batch worker panicked: {}",
                        panic_payload_text(payload.as_ref())
                    ),
                },
            });
            let _ = tx.send(ConversionBatchQueueEvent::Finished { id, outcome });
        });
        true
    }

    fn persist(&mut self) {
        let Some(path) = self.runtime.persistence_path().map(Path::to_path_buf) else {
            return;
        };
        let items = self
            .items
            .iter()
            .filter(|item| item.status != ConversionBatchQueueStatus::Done)
            .map(|item| PersistedBatchQueueItem {
                id: item.id,
                status: item.status,
                spec: item.spec.clone(),
                checkpoint: item.checkpoint.clone(),
                error: item.error.clone(),
                recovery: item.recovery.clone(),
            })
            .collect();
        let envelope = PersistedQueueEnvelope::new(
            self.runtime.next_id(),
            self.runtime.is_paused(),
            items,
        );
        let result = write_persisted_queue(&path, "conversion batch", &envelope);
        self.runtime.record_persistence_result(result);
    }
}

fn item_from_spec(
    id: u64,
    spec: QueuedConversionBatchSpec,
    checkpoint: ConversionBatchCheckpoint,
    status: ConversionBatchQueueStatus,
    restored: bool,
    requires_resume: bool,
    error: Option<String>,
    recovery: Option<ConversionBatchRecoveryRecord>,
) -> ConversionBatchQueueItem {
    let first = &spec.batch.faces[0].capture;
    let mut item = ConversionBatchQueueItem {
        id,
        label: format!(
            "{} Faces → {}",
            spec.batch.face_count(),
            first.production_project_name
        ),
        source_project_path: first.source_project_path.clone(),
        production_project_path: first.production_project_path.clone(),
        current_source: None,
        current_destination: None,
        face_count: spec.batch.face_count(),
        completed_face_count: checkpoint.completed_count(),
        status,
        phase: String::new(),
        progress: checkpoint.completed_count() as f32 / spec.batch.face_count() as f32,
        detail: if requires_resume {
            format!(
                "Recovered with {} of {} committed Faces; explicit resume required",
                checkpoint.completed_count(),
                spec.batch.face_count()
            )
        } else {
            String::new()
        },
        error,
        recovery,
        restored,
        requires_resume,
        checkpoint,
        spec,
    };
    refresh_item_cursor(&mut item);
    item
}

fn refresh_item_cursor(item: &mut ConversionBatchQueueItem) {
    item.completed_face_count = item.checkpoint.completed_count();
    let pending = item.spec.batch.faces.get(item.completed_face_count);
    item.current_source = pending.map(|face| face.capture.source_face_path.clone());
    item.current_destination = pending.map(|face| face.capture.output_tiff_path.clone());
}

fn apply_step_outcome(
    item: &mut ConversionBatchQueueItem,
    outcome: ConversionBatchStepOutcome,
) -> Option<ConversionBatchQueueCompletion> {
    match outcome {
        ConversionBatchStepOutcome::AlreadyComplete { checkpoint } => {
            item.checkpoint = checkpoint;
            item.status = ConversionBatchQueueStatus::Done;
            item.progress = 1.0;
            item.phase = ConversionPhase::Complete.label().to_owned();
            item.detail = "All batch Faces are committed".to_owned();
            item.error = None;
            item.recovery = None;
            item.requires_resume = false;
            refresh_item_cursor(item);
            None
        }
        ConversionBatchStepOutcome::CompletedFace {
            checkpoint,
            completed,
            source_face_index,
            ordinal,
            batch_complete,
        } => {
            item.checkpoint = checkpoint;
            item.status = if batch_complete {
                ConversionBatchQueueStatus::Done
            } else {
                ConversionBatchQueueStatus::Waiting
            };
            item.progress = item.checkpoint.completed_count() as f32 / item.spec.batch.face_count() as f32;
            item.phase = if batch_complete {
                ConversionPhase::Complete.label().to_owned()
            } else {
                "Face checkpoint committed".to_owned()
            };
            item.detail = if batch_complete {
                format!("All {} batch Faces committed", item.spec.batch.face_count())
            } else {
                format!(
                    "Face {} committed; checkpoint durable before next Face",
                    source_face_index + 1
                )
            };
            item.error = None;
            item.recovery = None;
            item.requires_resume = false;
            refresh_item_cursor(item);
            Some(ConversionBatchQueueCompletion {
                id: item.id,
                source_face_index,
                result: ConversionBatchQueueCompletionResult::CompletedFace {
                    completed,
                    ordinal,
                    batch_complete,
                },
            })
        }
        ConversionBatchStepOutcome::Halted {
            checkpoint,
            source_face_index,
            disposition,
            outcome,
        } => {
            item.checkpoint = checkpoint;
            item.requires_resume = false;
            refresh_item_cursor(item);
            let ordinal = item.checkpoint.completed_count();
            let result = match outcome {
                ConversionTransactionOutcome::CancelledBeforeCommit { phase, message } => {
                    item.status = ConversionBatchQueueStatus::Cancelled;
                    item.phase = phase.label().to_owned();
                    item.detail = format!(
                        "Batch cancelled with {} of {} Faces committed",
                        item.checkpoint.completed_count(),
                        item.spec.batch.face_count()
                    );
                    item.error = Some(message.clone());
                    item.recovery = None;
                    ConversionBatchQueueCompletionResult::Cancelled {
                        phase: phase.label().to_owned(),
                        message,
                    }
                }
                ConversionTransactionOutcome::FailedBeforeCommit { phase, error } => {
                    item.status = ConversionBatchQueueStatus::Failed;
                    item.phase = phase.label().to_owned();
                    item.detail = format!(
                        "Face {} failed before commit; previous batch Faces remain committed",
                        source_face_index + 1
                    );
                    item.error = Some(error.clone());
                    item.recovery = None;
                    ConversionBatchQueueCompletionResult::Failed {
                        phase: phase.label().to_owned(),
                        error,
                    }
                }
                ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
                    committed_output,
                    production_project_path,
                    production_project,
                    error,
                } => {
                    let recovery = ConversionBatchRecoveryRecord {
                        source_face_index,
                        ordinal,
                        disposition,
                        recovery: ConversionRecoveryRecord {
                            stage: ConversionRecoveryStage::ProductionProjectSavePending,
                            committed_output,
                            production_project_path,
                            production_project,
                            error: error.clone(),
                        },
                    };
                    item.status = ConversionBatchQueueStatus::NeedsRecovery;
                    item.phase = ConversionPhase::ProductionProjectSave.label().to_owned();
                    item.detail = if recovery.disposition.is_some() {
                        format!(
                            "Face {} TIFF committed; Production project recovery required before batch can continue",
                            source_face_index + 1
                        )
                    } else {
                        format!(
                            "Face {} TIFF committed; exact Production disposition is unavailable, so automatic recovery is blocked",
                            source_face_index + 1
                        )
                    };
                    item.error = Some(error);
                    item.recovery = Some(recovery.clone());
                    ConversionBatchQueueCompletionResult::NeedsRecovery(recovery)
                }
                ConversionTransactionOutcome::Completed(completed) => {
                    // `run_next_conversion_batch_face` converts Completed into
                    // CompletedFace, so reaching this arm indicates an executor
                    // contract violation. Fail closed instead of advancing.
                    let error = format!(
                        "Batch executor returned an uncheckpointed completed Face at {}.",
                        completed.committed_output.path.display()
                    );
                    item.status = ConversionBatchQueueStatus::Failed;
                    item.phase = ConversionPhase::OutputCommit.label().to_owned();
                    item.detail = "Batch checkpoint contract violation".to_owned();
                    item.error = Some(error.clone());
                    item.recovery = None;
                    ConversionBatchQueueCompletionResult::Failed {
                        phase: ConversionPhase::OutputCommit.label().to_owned(),
                        error,
                    }
                }
            };
            Some(ConversionBatchQueueCompletion {
                id: item.id,
                source_face_index,
                result,
            })
        }
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

fn batch_queue_persistence_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ShadeEditor")
        .join("conversion-batch-queue.json")
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
    use crate::conversion_batch::{ConversionBatchFaceCapture, ConversionBatchScope};
    use crate::conversion_transaction::{
        CapturedOutputPolicy, CapturedSourceProfile, CommittedConversionOutput,
        ConversionJobCapture,
    };
    use crate::model::{IccProfileIdentity, ShadeProject};
    use crate::production_project_disposition::ProductionProjectDisposition;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn recipe(source_hash: &str) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source RGB".to_owned(),
                sha256: source_hash.to_owned(),
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
                    sha256: hash('d'),
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
            custom_optimizer_solver: None,
        }
    }

    fn batch(project_suffix: &str) -> ConversionBatchCapture {
        let project = ShadeProject::default();
        let mut faces = Vec::new();
        for index in 0..2 {
            let source_profile_hash = if index == 0 { hash('1') } else { hash('2') };
            let capture = ConversionJobCapture::capture(
                &project,
                PathBuf::from(r"C:\Design\Source.shade"),
                hash('a'),
                PathBuf::from(format!(r"C:\Design\Face-{index}.tif")),
                Some(7),
                hash(if index == 0 { 'b' } else { 'c' }),
                CapturedSourceProfile::Embedded,
                recipe(&source_profile_hash),
                CapturedOutputPolicy::MustNotExist,
                PathBuf::from(format!(r"C:\Production\{project_suffix}-Face-{index}.tif")),
                PathBuf::from(format!(r"C:\Production\{project_suffix}.shade")),
                format!("Production {project_suffix}"),
                format!("Face {index}"),
            )
            .unwrap();
            faces.push(ConversionBatchFaceCapture {
                source_face_index: index,
                capture,
            });
        }
        ConversionBatchCapture::capture(
            ConversionBatchScope::AllFaces,
            2,
            ProductionProjectDisposition::CreateNew,
            faces,
        )
        .unwrap()
    }

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-conversion-batch-queue-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn inject_recovery(
        queue: &mut ConversionBatchQueue,
        id: u64,
        disposition: Option<ProductionProjectDisposition>,
    ) -> CommittedConversionOutput {
        let item = queue.items.iter_mut().find(|item| item.id == id).unwrap();
        let output = CommittedConversionOutput {
            path: item.spec.batch.faces[0].capture.output_tiff_path.clone(),
            sha256: hash('e'),
            converted_at_unix_ms: 1234,
        };
        item.status = ConversionBatchQueueStatus::NeedsRecovery;
        item.recovery = Some(ConversionBatchRecoveryRecord {
            source_face_index: item.spec.batch.faces[0].source_face_index,
            ordinal: 0,
            disposition,
            recovery: ConversionRecoveryRecord {
                stage: ConversionRecoveryStage::ProductionProjectSavePending,
                committed_output: output.clone(),
                production_project_path: item.spec.batch.faces[0]
                    .capture
                    .production_project_path
                    .clone(),
                production_project: Some(ShadeProject::default()),
                error: "mock project save failure".to_owned(),
            },
        });
        refresh_item_cursor(item);
        output
    }

    #[test]
    fn restored_batch_requires_resume_and_preserves_intent() {
        let path = temp_path("restore");
        let mut queue = ConversionBatchQueue::empty(Some(path.clone()));
        queue.enqueue(batch("Job"), 220.0).unwrap();
        drop(queue);

        let restored = ConversionBatchQueue::load_from_path(path.clone()).unwrap();
        assert_eq!(restored.items.len(), 1);
        assert_eq!(restored.items[0].status, ConversionBatchQueueStatus::Waiting);
        assert!(restored.items[0].requires_resume);
        assert_eq!(restored.items[0].face_count, 2);
        assert_eq!(restored.items[0].completed_face_count, 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn durable_checkpoint_survives_restart_between_faces() {
        let path = temp_path("checkpoint");
        let mut queue = ConversionBatchQueue::empty(Some(path.clone()));
        let id = queue.enqueue(batch("Checkpoint"), 220.0).unwrap();
        let item = queue.items.iter_mut().find(|item| item.id == id).unwrap();
        let output = CommittedConversionOutput {
            path: item.spec.batch.faces[0].capture.output_tiff_path.clone(),
            sha256: hash('e'),
            converted_at_unix_ms: 1234,
        };
        item.checkpoint.record_committed(&item.spec.batch, &output).unwrap();
        item.status = ConversionBatchQueueStatus::Waiting;
        refresh_item_cursor(item);
        queue.persist();
        drop(queue);

        let restored = ConversionBatchQueue::load_from_path(path.clone()).unwrap();
        assert_eq!(restored.items[0].completed_face_count, 1);
        assert!(restored.items[0]
            .current_source
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .contains("Face-1"));
        assert!(restored.items[0].requires_resume);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn retry_keeps_committed_face_checkpoint() {
        let mut queue = ConversionBatchQueue::new();
        let id = queue.enqueue(batch("Retry"), 220.0).unwrap();
        let item = queue.items.iter_mut().find(|item| item.id == id).unwrap();
        let output = CommittedConversionOutput {
            path: item.spec.batch.faces[0].capture.output_tiff_path.clone(),
            sha256: hash('e'),
            converted_at_unix_ms: 1234,
        };
        item.checkpoint.record_committed(&item.spec.batch, &output).unwrap();
        item.status = ConversionBatchQueueStatus::Failed;
        refresh_item_cursor(item);

        assert!(queue.retry(id));
        let item = queue.items.iter().find(|item| item.id == id).unwrap();
        assert_eq!(item.completed_face_count, 1);
        assert_eq!(item.status, ConversionBatchQueueStatus::Waiting);
        assert!(item.current_source.as_ref().unwrap().to_string_lossy().contains("Face-1"));
    }

    #[test]
    fn project_only_recovery_advances_checkpoint_without_rerendering_face() {
        let mut queue = ConversionBatchQueue::new();
        let id = queue.enqueue(batch("Recover"), 220.0).unwrap();
        let output = inject_recovery(
            &mut queue,
            id,
            Some(ProductionProjectDisposition::CreateNew),
        );
        let mut recovery_calls = 0;

        let completion = queue
            .recover_with(id, |capture, disposition, recovery| {
                recovery_calls += 1;
                assert!(matches!(disposition, ProductionProjectDisposition::CreateNew));
                assert_eq!(capture.output_tiff_path, output.path);
                Ok(CompletedConversionTransaction {
                    committed_output: recovery.committed_output.clone(),
                    production_project_path: recovery.production_project_path.clone(),
                    production_project: recovery.production_project.clone().unwrap(),
                })
            })
            .unwrap();

        assert_eq!(recovery_calls, 1);
        let ConversionBatchQueueCompletionResult::CompletedFace {
            ordinal,
            batch_complete,
            ..
        } = completion.result
        else {
            panic!("recovery should complete exactly one Face");
        };
        assert_eq!(ordinal, 0);
        assert!(!batch_complete);
        let item = queue.items.iter().find(|item| item.id == id).unwrap();
        assert_eq!(item.completed_face_count, 1);
        assert_eq!(item.status, ConversionBatchQueueStatus::Waiting);
        assert!(item.recovery.is_none());
        assert!(item.current_source.as_ref().unwrap().to_string_lossy().contains("Face-1"));
    }

    #[test]
    fn legacy_recovery_without_exact_disposition_is_blocked() {
        let mut queue = ConversionBatchQueue::new();
        let id = queue.enqueue(batch("LegacyRecovery"), 220.0).unwrap();
        inject_recovery(&mut queue, id, None);

        let error = queue
            .recover_with(id, |_, _, _| panic!("legacy recovery must fail before replay"))
            .expect_err("missing exact disposition must block recovery");
        assert!(error.contains("predates exact Production disposition"));
        let item = queue.items.iter().find(|item| item.id == id).unwrap();
        assert_eq!(item.status, ConversionBatchQueueStatus::NeedsRecovery);
        assert_eq!(item.completed_face_count, 0);
    }

    #[test]
    fn recovery_disposition_survives_restart() {
        let path = temp_path("recovery-disposition");
        let mut queue = ConversionBatchQueue::empty(Some(path.clone()));
        let id = queue.enqueue(batch("RecoveryPersistence"), 220.0).unwrap();
        inject_recovery(
            &mut queue,
            id,
            Some(ProductionProjectDisposition::CreateNew),
        );
        queue.persist();
        drop(queue);

        let restored = ConversionBatchQueue::load_from_path(path.clone()).unwrap();
        let recovery = restored.items[0].recovery.as_ref().unwrap();
        assert!(matches!(
            recovery.disposition,
            Some(ProductionProjectDisposition::CreateNew)
        ));
        assert_eq!(restored.items[0].status, ConversionBatchQueueStatus::NeedsRecovery);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn same_production_project_cannot_be_reserved_by_two_batches() {
        let mut queue = ConversionBatchQueue::new();
        queue.enqueue(batch("Reserved"), 220.0).unwrap();
        let error = queue
            .enqueue(batch("Reserved"), 220.0)
            .expect_err("same Production project must remain single-writer");
        assert!(error.contains("already reserved"));
    }
}
