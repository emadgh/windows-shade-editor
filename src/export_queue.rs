use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crate::export;
use crate::export_batch::{self, ConflictPolicy, DestinationDecision};
use crate::model::ShadeProject;
use crate::path_safety;
use crate::validation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug)]
pub struct ExportQueueMark {
    pub snapshot_id: u64,
    pub face_key: String,
    pub folder: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ExportQueueSpec {
    pub label: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub project: ShadeProject,
    pub default_dpi: f64,
    pub force_lzw: bool,
    pub validate_after_export: bool,
    pub conflict_policy: ConflictPolicy,
    pub mark: Option<ExportQueueMark>,
}

#[derive(Clone, Debug)]
struct QueuedExportSpec {
    export: ExportQueueSpec,
    protected_sources: Vec<PathBuf>,
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

pub struct ExportQueue {
    items: Vec<ExportQueueItem>,
    next_id: u64,
    active_id: Option<u64>,
    stop_after_current: bool,
    tx: mpsc::Sender<ExportQueueEvent>,
    rx: mpsc::Receiver<ExportQueueEvent>,
}

impl Default for ExportQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportQueue {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            items: Vec::new(),
            next_id: 1,
            active_id: None,
            stop_after_current: false,
            tx,
            rx,
        }
    }

    /// Test/helper enqueue without project isolation. Production UI should use
    /// `enqueue_for_project` so source TIFFs and project ownership are protected.
    pub fn enqueue(&mut self, spec: ExportQueueSpec) -> u64 {
        self.enqueue_for_project(spec, Vec::new(), 0)
            .expect("unprotected queue enqueue must not fail")
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
            spec: QueuedExportSpec {
                export: spec,
                protected_sources,
                project_session_id,
            },
        });
        Ok(id)
    }

    pub fn items(&self) -> &[ExportQueueItem] {
        &self.items
    }

    pub fn pending_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    ExportQueueStatus::Waiting | ExportQueueStatus::Processing
                )
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
        match item.status {
            ExportQueueStatus::Waiting => {
                item.status = ExportQueueStatus::Cancelled;
                item.detail = "Cancelled before processing".to_owned();
                true
            }
            ExportQueueStatus::Processing => {
                self.stop_after_current = true;
                item.detail =
                    "Stop requested · current atomic export will finish safely".to_owned();
                true
            }
            _ => false,
        }
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
        item.progress = 0.0;
        item.detail.clear();
        item.error = None;
        true
    }

    pub fn cancel_all_waiting(&mut self) {
        for item in &mut self.items {
            if item.status == ExportQueueStatus::Waiting {
                item.status = ExportQueueStatus::Cancelled;
                item.detail = "Cancelled before processing".to_owned();
            }
        }
    }

    pub fn clear_finished(&mut self) {
        self.items.retain(|item| !item.status.finished());
    }

    pub fn poll(&mut self) -> Vec<ExportQueueCompletion> {
        let mut completions = Vec::new();
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
                    if self.stop_after_current {
                        self.stop_after_current = false;
                        self.cancel_all_waiting();
                    }
                }
            }
        }

        if self.active_id.is_none() && !self.stop_after_current {
            self.start_next();
        }
        completions
    }

    fn start_next(&mut self) {
        let Some(index) = self
            .items
            .iter()
            .position(|item| item.status == ExportQueueStatus::Waiting)
        else {
            return;
        };

        let mut queued = self.items[index].spec.clone();
        let id = self.items[index].id;
        let session_id = queued.project_session_id;

        if let Err(err) =
            validate_destination(&queued.export.destination, &queued.protected_sources)
        {
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
            return;
        }

        // A file may appear after enqueue. Honor the original conflict policy
        // immediately before processing while respecting every other queued reservation.
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
                    return;
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
            return;
        }

        self.items[index].status = ExportQueueStatus::Processing;
        self.items[index].progress = 0.0;
        self.items[index].detail = "Starting".to_owned();
        self.items[index].error = None;
        self.active_id = Some(id);

        let spec = queued.export;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let validate_after_export = spec.validate_after_export;
            let progress_tx = tx.clone();
            let result = export::export_face_with_progress_options(
                &spec.source,
                &spec.destination,
                &spec.project,
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
    }
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

    fn spec(destination: &str) -> ExportQueueSpec {
        ExportQueueSpec {
            label: "test".to_owned(),
            source: PathBuf::from("missing.tif"),
            destination: PathBuf::from(destination),
            project: ShadeProject::default(),
            default_dpi: 220.0,
            force_lzw: true,
            validate_after_export: false,
            conflict_policy: ConflictPolicy::AutoNumber,
            mark: None,
        }
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
        let mut queue = ExportQueue::new();
        let err = queue
            .enqueue_for_project(spec("source.tif"), vec![PathBuf::from("source.tif")], 7)
            .unwrap_err();
        assert!(err.contains("source TIFF"));
    }

    #[test]
    fn pending_destinations_are_globally_reserved() {
        let mut queue = ExportQueue::new();
        queue
            .enqueue_for_project(spec("same.tif"), vec![], 1)
            .unwrap();
        let err = queue
            .enqueue_for_project(spec("same.tif"), vec![], 1)
            .unwrap_err();
        assert!(err.contains("already reserved"));
    }
}
