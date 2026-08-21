use std::any::Any;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::thread;

use serde::{Deserialize, Serialize};

pub use crate::conversion_recovery::{ConversionRecoveryRecord, ConversionRecoveryStage};
use crate::conversion_recovery::recover_production_project;
use crate::conversion_transaction::{
    CommittedConversionOutput, CompletedConversionTransaction, ConversionCancellation,
    ConversionJobCapture, ConversionPhase, ConversionTransactionOutcome,
};
use crate::conversion_transaction_disposition::run_conversion_transaction_with_disposition;
use crate::icc_conversion_worker::FilesystemIccConversionBackend;
use crate::model::ShadeProject;
use crate::production_project_disposition::ProductionProjectDisposition;
use crate::queue_core::{
    PersistedQueueEnvelope, QueueLifecycle, QueueRuntime, load_persisted_queue,
    sanitize_progress, write_persisted_queue,
};

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

    fn from_common_lifecycle(status: QueueLifecycle) -> Self {
        match status {
            QueueLifecycle::Waiting => Self::Waiting,
            QueueLifecycle::Processing => Self::Processing,
            QueueLifecycle::Done => Self::Done,
            QueueLifecycle::Failed => Self::Failed,
            QueueLifecycle::Cancelled => Self::Cancelled,
        }
    }

    fn restored(self) -> (Self, bool) {
        let Some(common) = self.common_lifecycle() else {
            return (Self::NeedsRecovery, false);
        };
        let (restored, requires_resume) = common.restored();
        (Self::from_common_lifecycle(restored), requires_resume)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct QueuedConversionSpec {
    capture: ConversionJobCapture,
    #[serde(default)]
    production_project_disposition: ProductionProjectDisposition,
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
struct PersistedQueueItem {
    id: u64,
    status: ConversionQueueStatus,
    spec: QueuedConversionSpec,
    error: Option<String>,
    recovery: Option<ConversionRecoveryRecord>,
}

pub struct ConversionQueue {
    items: Vec<ConversionQueueItem>,
    runtime: QueueRuntime<ConversionQueueEvent>,
    active_cancellation: Option<ConversionCancellation>,
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
        Self {
            items: Vec::new(),
            runtime: QueueRuntime::new(persistence_path),
            active_cancellation: None,
        }
    }

    fn load_from_path(path: PathBuf) -> Result<Self, String> {
        let Some(persisted) = load_persisted_queue::<PersistedQueueItem>(&path, "conversion")?
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
            if saved.status == ConversionQueueStatus::Done {
                continue;
            }
            let (status, requires_resume) = saved.status.restored();
            queue.items.push(item_from_spec(
                saved.id,
                saved.spec,
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
        capture: ConversionJobCapture,
        default_dpi: f64,
    ) -> Result<u64, String> {
        self.enqueue_with_production_project_disposition(
            capture,
            ProductionProjectDisposition::CreateNew,
            default_dpi,
        )
    }

    pub fn enqueue_with_production_project_disposition(
        &mut self,
        capture: ConversionJobCapture,
        production_project_disposition: ProductionProjectDisposition,
        default_dpi: f64,
    ) -> Result<u64, String> {
        capture.validate()?;
        production_project_disposition.validate()?;
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
        let id = self.runtime.allocate_id();
        self.items.push(item_from_spec(
            id,
            QueuedConversionSpec {
                capture,
                production_project_disposition,
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
        self.runtime.active_id().is_some()
    }

    pub fn recovered_waiting_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == ConversionQueueStatus::Waiting && item.requires_resume)
            .count()
    }

    pub fn active_summary(&self) -> Option<(f32, String)> {
        let id = self.runtime.active_id()?;
        let item = self.items.iter().find(|item| item.id == id)?;
        let text = if item.detail.trim().is_empty() {
            format!("Conversion #{} - {}", item.id, item.phase)
        } else {
            format!("Conversion #{} - {} - {}", item.id, item.phase, item.detail)
        };
        Some((sanitize_progress(item.progress), text))
    }

    pub fn is_paused(&self) -> bool {
        self.runtime.is_paused()
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.runtime.set_paused(paused) {
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
        let active_id = self.runtime.active_id();
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
            ConversionQueueStatus::Processing if active_id == Some(id) => {
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
            ConversionQueueStatus::Failed | ConversionQueueStatus::Cancelled
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

    pub fn recover_project(
        &mut self,
        id: u64,
    ) -> Result<Option<ConversionQueueCompletion>, String> {
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return Ok(None);
        };
        if self.items[index].status != ConversionQueueStatus::NeedsRecovery {
            return Ok(None);
        }
        let recovery = self.items[index]
            .recovery
            .clone()
            .ok_or_else(|| "Conversion queue item is missing its persisted recovery record.".to_owned())?;
        let capture = self.items[index].spec.capture.clone();
        let disposition = self.items[index].spec.production_project_disposition.clone();

        match recover_production_project(&capture, &disposition, &recovery) {
            Ok(completed) => {
                let result = ConversionQueueCompletionResult::Completed(completed);
                apply_completion(&mut self.items[index], &result);
                self.items[index].detail = "Production project recovery completed".to_owned();
                let completion = ConversionQueueCompletion {
                    id,
                    capture,
                    result,
                };
                self.persist();
                Ok(Some(completion))
            }
            Err(error) => {
                self.items[index].phase = ConversionPhase::ProductionProjectSave.label().to_owned();
                self.items[index].detail = "Production project recovery remains blocked".to_owned();
                self.items[index].error = Some(error.clone());
                if let Some(record) = self.items[index].recovery.as_mut() {
                    record.error = error.clone();
                }
                self.persist();
                Err(error)
            }
        }
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
        self.runtime.take_persistence_error()
    }

    pub fn poll(&mut self) -> Vec<ConversionQueueCompletion> {
        self.poll_with_start(true)
    }

    pub fn poll_with_start(&mut self, allow_start: bool) -> Vec<ConversionQueueCompletion> {
        let mut completions = Vec::new();
        let mut changed = false;
        while let Ok(event) = self.runtime.try_recv() {
            match event {
                ConversionQueueEvent::Progress {
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
                ConversionQueueEvent::Finished {
                    id,
                    capture,
                    result,
                } => {
                    self.runtime.set_active_id(None);
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
        if allow_start && self.runtime.active_id().is_none() && !self.runtime.is_paused() {
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
        self.runtime.set_active_id(Some(id));
        self.active_cancellation = Some(cancellation.clone());

        let tx = self.runtime.sender();
        thread::spawn(move || {
            let capture = spec.capture;
            let production_project_disposition = spec.production_project_disposition;
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
                run_conversion_transaction_with_disposition(
                    &capture,
                    &production_project_disposition,
                    &cancellation,
                    &mut backend,
                    |progress| {
                        let _ = worker_tx.send(ConversionQueueEvent::Progress {
                            id,
                            phase: progress.phase.label().to_owned(),
                            fraction: progress.fraction,
                            detail: progress.detail,
                        });
                    },
                )
            }))
            .unwrap_or_else(|payload| ConversionTransactionOutcome::FailedBeforeCommit {
                phase: ConversionPhase::CaptureValidation,
                error: format!(
                    "Conversion worker panicked: {}",
                    panic_payload_text(payload.as_ref())
                ),
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
        let Some(path) = self.runtime.persistence_path().map(Path::to_path_buf) else {
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
        let persisted = PersistedQueueEnvelope::new(
            self.runtime.next_id(),
            self.runtime.is_paused(),
            items,
        );
        let result = write_persisted_queue(&path, "conversion", &persisted);
        self.runtime.record_persistence_result(result);
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
    _capture: &ConversionJobCapture,
    outcome: ConversionTransactionOutcome,
) -> ConversionQueueCompletionResult {
    match outcome {
        ConversionTransactionOutcome::Completed(value) => {
            ConversionQueueCompletionResult::Completed(value)
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
            stage: ConversionRecoveryStage::ProductionProjectSavePending,
            committed_output,
            production_project_path,
            production_project,
            error,
        }),
    }
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
            source_transparency_policy: None,
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
            custom_optimizer_solver: None,
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
    fn needs_recovery_cannot_be_retried_as_full_conversion() {
        let mut queue = ConversionQueue::new();
        let id = queue
            .enqueue(capture(r"C:\Production\recover.tif"), 220.0)
            .unwrap();
        {
            let item = queue.items.iter_mut().find(|item| item.id == id).unwrap();
            item.status = ConversionQueueStatus::NeedsRecovery;
            item.recovery = Some(ConversionRecoveryRecord {
                stage: ConversionRecoveryStage::ProductionProjectSavePending,
                committed_output: CommittedConversionOutput {
                    path: PathBuf::from(r"C:\Production\recover.tif"),
                    sha256: HASH.to_owned(),
                    converted_at_unix_ms: 1,
                },
                production_project_path: PathBuf::from(r"C:\Production\recover.shade"),
                production_project: Some(ShadeProject::default()),
                error: "simulated project-save failure".to_owned(),
            });
        }

        assert!(!queue.retry(id));
        let item = queue.items.iter().find(|item| item.id == id).unwrap();
        assert_eq!(item.status, ConversionQueueStatus::NeedsRecovery);
        assert!(item.recovery.is_some());
        assert_eq!(
            item.recovery.as_ref().unwrap().error,
            "simulated project-save failure"
        );
    }

    #[test]
    fn legacy_recovery_record_defaults_to_project_save_stage() {
        let record = ConversionRecoveryRecord {
            stage: ConversionRecoveryStage::ProductionProjectSavePending,
            committed_output: CommittedConversionOutput {
                path: PathBuf::from(r"C:\Production\recover.tif"),
                sha256: HASH.to_owned(),
                converted_at_unix_ms: 1,
            },
            production_project_path: PathBuf::from(r"C:\Production\recover.shade"),
            production_project: Some(ShadeProject::default()),
            error: "legacy".to_owned(),
        };
        let mut value = serde_json::to_value(record).unwrap();
        value.as_object_mut().unwrap().remove("stage");
        let restored: ConversionRecoveryRecord = serde_json::from_value(value).unwrap();
        assert_eq!(
            restored.stage,
            ConversionRecoveryStage::ProductionProjectSavePending
        );
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
    fn completed_conversion_does_not_write_source_project() {
        let source_path = temp_path("source-explicit-save").with_extension("shade");
        let production_path = temp_path("production-explicit-save").with_extension("shade");
        let source = ShadeProject::default();
        source.save(&source_path, &[]).unwrap();
        let before = fs::read(&source_path).unwrap();
        let mut captured = capture(r"C:\Production\out.tif");
        captured.source_project_path = source_path.clone();
        let completed = CompletedConversionTransaction {
            committed_output: CommittedConversionOutput {
                path: PathBuf::from(r"C:\Production\out.tif"),
                sha256: HASH.to_owned(),
                converted_at_unix_ms: 1,
            },
            production_project_path: production_path,
            production_project: ShadeProject::default(),
        };

        let result = completion_result(
            &captured,
            ConversionTransactionOutcome::Completed(completed),
        );
        assert!(matches!(result, ConversionQueueCompletionResult::Completed(_)));
        assert_eq!(fs::read(&source_path).unwrap(), before);
        let _ = fs::remove_file(source_path);
    }

    #[test]
    fn legacy_icc_capture_round_trip_preserves_stored_recipe_hash() {
        let mut original = capture(r"C:\Production\Legacy.tif");
        original.conversion_recipe.schema_version =
            crate::color_conversion::LEGACY_CONVERSION_RECIPE_SCHEMA_VERSION;
        original.conversion_recipe.custom_optimizer_solver = None;
        original.conversion_recipe_sha256 =
            crate::conversion_recipe::recipe_sha256(&original.conversion_recipe).unwrap();
        original.validate().expect("legacy ICC capture is valid");
        let expected = original.conversion_recipe_sha256.clone();
        let json = serde_json::to_vec(&original).unwrap();
        let restored: ConversionJobCapture = serde_json::from_slice(&json).unwrap();
        restored.validate().expect("restored legacy ICC capture is valid");
        assert_eq!(restored.conversion_recipe_sha256, expected);
        assert_eq!(
            crate::conversion_recipe::recipe_sha256(&restored.conversion_recipe).unwrap(),
            expected
        );
    }

    #[test]
    fn queued_spec_without_project_disposition_defaults_to_create_new() {
        let spec = QueuedConversionSpec {
            capture: capture(r"C:\Production\legacy.tif"),
            production_project_disposition: ProductionProjectDisposition::CreateNew,
            default_dpi: 220.0,
        };
        let mut value = serde_json::to_value(&spec).unwrap();
        value
            .as_object_mut()
            .expect("queued spec object")
            .remove("production_project_disposition");
        let restored: QueuedConversionSpec = serde_json::from_value(value).unwrap();
        assert_eq!(
            restored.production_project_disposition,
            ProductionProjectDisposition::CreateNew
        );
    }

    #[test]
    fn enqueue_with_append_disposition_persists_exact_destination_intent() {
        let mut queue = ConversionQueue::new();
        let key = crate::production_project_compat::ProductionCompatibilityKey {
            engine_mode: ConversionEngineMode::Icc,
            output_profile_sha256: Some(HASH.to_owned()),
            device_link_sha256: None,
            characterization_id: None,
            channel_names: vec![
                "Cyan".to_owned(),
                "Magenta".to_owned(),
                "Yellow".to_owned(),
                "Black".to_owned(),
            ],
            bit_depth: 16,
        };
        let disposition = ProductionProjectDisposition::append_existing(HASH.to_owned(), &key)
            .unwrap();
        queue
            .enqueue_with_production_project_disposition(
                capture(r"C:\Production\append.tif"),
                disposition.clone(),
                220.0,
            )
            .unwrap();
        assert_eq!(
            queue.items[0].spec.production_project_disposition,
            disposition
        );
    }

    #[test]
    fn needs_recovery_restore_is_not_flattened_into_common_waiting_state() {
        let path = temp_path("needs-recovery");
        let mut queue = ConversionQueue::empty(Some(path.clone()));
        let id = queue
            .enqueue(capture(r"C:\Production\recover-persisted.tif"), 220.0)
            .unwrap();
        let item = queue.items.iter_mut().find(|item| item.id == id).unwrap();
        item.status = ConversionQueueStatus::NeedsRecovery;
        item.recovery = Some(ConversionRecoveryRecord {
            stage: ConversionRecoveryStage::ProductionProjectSavePending,
            committed_output: CommittedConversionOutput {
                path: PathBuf::from(r"C:\Production\recover-persisted.tif"),
                sha256: HASH.to_owned(),
                converted_at_unix_ms: 1,
            },
            production_project_path: PathBuf::from(r"C:\Production\recover-persisted.shade"),
            production_project: Some(ShadeProject::default()),
            error: "persisted recovery".to_owned(),
        });
        queue.persist();
        drop(queue);

        let restored = ConversionQueue::load_from_path(path.clone()).unwrap();
        assert_eq!(restored.items[0].status, ConversionQueueStatus::NeedsRecovery);
        assert!(!restored.items[0].requires_resume);
        assert!(restored.items[0].recovery.is_some());
        let _ = fs::remove_file(path);
    }
}
