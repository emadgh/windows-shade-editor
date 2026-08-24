use crate::*;
use eframe::egui;
use std::collections::BTreeMap;
use std::path::Path;
use windows_shade_editor::conversion_batch::{
    ConversionBatchCapture, ConversionBatchFaceCapture, ConversionBatchScope,
};
use windows_shade_editor::conversion_batch_queue::{
    ConversionBatchQueue, ConversionBatchQueueCompletion, ConversionBatchQueueCompletionResult,
    ConversionBatchQueueItem, ConversionBatchQueueStatus,
};
use windows_shade_editor::conversion_transaction::ConversionJobCapture;

use super::conversion_plan::{ConversionFaceInspection, UnifiedConversionPlan};

pub(crate) struct ConversionBatchController {
    queue: ConversionBatchQueue,
    startup_error: Option<String>,
    owns_queue_exclusion: bool,
    export_was_paused: bool,
    legacy_conversion_was_paused: bool,
}

impl ConversionBatchController {
    pub(crate) fn load() -> Self {
        let (queue, startup_error) = match ConversionBatchQueue::load_persistent() {
            Ok(queue) => (queue, None),
            Err(error) => (ConversionBatchQueue::new(), Some(error)),
        };
        Self {
            queue,
            startup_error,
            owns_queue_exclusion: false,
            export_was_paused: false,
            legacy_conversion_was_paused: false,
        }
    }
}

#[derive(Clone, Copy)]
enum BatchQueueUiAction {
    ResumeRecovered,
    TogglePaused,
    Cancel(u64),
    Retry(u64),
    Recover(u64),
    ClearFinished,
}

impl ShadeApp {
    /// Durable runtime for every new Production Color Conversion. Current Face is represented as
    /// a one-Face `ConversionBatchCapture`, so Current/Selected/All share checkpoint/recovery and
    /// one Production-project single-writer behavior.
    pub(crate) fn poll_conversion_batch_runtime(&mut self) {
        let owns_exclusion = self.conversion_batch.owns_queue_exclusion;
        let has_recovery = self
            .conversion_batch
            .queue
            .items()
            .iter()
            .any(|item| item.status == ConversionBatchQueueStatus::NeedsRecovery);
        let allow_start = self.job.is_none()
            && !self.export.queue.is_active()
            && !self.conversion_queue.is_active()
            && (owns_exclusion
                || (!self.export.queue.has_pending() && !self.conversion_queue.has_pending()));

        let source_paths = self
            .conversion_batch
            .queue
            .items()
            .iter()
            .map(|item| (item.id, item.source_project_path.clone()))
            .collect::<BTreeMap<_, _>>();
        let completions = self.conversion_batch.queue.poll_with_start(allow_start);
        let persistence_error = self.conversion_batch.queue.take_persistence_error();
        let should_block = batch_queue_blocks_other_work(&self.conversion_batch.queue);

        if let Some(error) = persistence_error {
            self.log
                .error(&format!("Conversion Batch Queue persistence: {error}"));
        }

        if should_block && !self.conversion_batch.owns_queue_exclusion {
            self.conversion_batch.export_was_paused = self.export.queue.is_paused();
            self.conversion_batch.legacy_conversion_was_paused = self.conversion_queue.is_paused();
            self.export.queue.set_paused(true);
            self.conversion_queue.set_paused(true);
            self.conversion_batch.owns_queue_exclusion = true;
        } else if !should_block
            && self.conversion_batch.owns_queue_exclusion
            && !has_recovery
        {
            self.export
                .queue
                .set_paused(self.conversion_batch.export_was_paused);
            self.conversion_queue
                .set_paused(self.conversion_batch.legacy_conversion_was_paused);
            self.conversion_batch.owns_queue_exclusion = false;
        }

        for completion in completions {
            let source_path = source_paths.get(&completion.id).cloned();
            self.handle_conversion_batch_completion(completion, source_path.as_deref());
        }
    }

    pub(crate) fn conversion_batch_blocks_project_transition(&self) -> bool {
        self.conversion_batch.queue.items().iter().any(|item| {
            matches!(
                item.status,
                ConversionBatchQueueStatus::Waiting
                    | ConversionBatchQueueStatus::Processing
                    | ConversionBatchQueueStatus::NeedsRecovery
            )
        })
    }

    pub(crate) fn conversion_batch_active_summary(&self) -> Option<(f32, String)> {
        self.conversion_batch.queue.active_summary()
    }

    pub(crate) fn conversion_batch_pending_count(&self) -> usize {
        self.conversion_batch
            .queue
            .items()
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    ConversionBatchQueueStatus::Waiting
                        | ConversionBatchQueueStatus::Processing
                        | ConversionBatchQueueStatus::NeedsRecovery
                )
            })
            .count()
    }

    pub(crate) fn queue_unified_conversion_plan(
        &mut self,
        scope: ConversionBatchScope,
        inspections: &[ConversionFaceInspection],
        plan: UnifiedConversionPlan,
    ) -> Result<u64, String> {
        if self.job.is_some() {
            return Err("Finish the current foreground operation before queueing conversion.".to_owned());
        }
        if self.export.queue.has_pending() || self.conversion_queue.has_pending() {
            return Err(
                "Finish or cancel legacy Export/Conversion Queue work before queueing Production Color Conversion."
                    .to_owned(),
            );
        }
        let source_project_path = self
            .project_path
            .clone()
            .ok_or_else(|| "Save the Source project before queueing conversion.".to_owned())?;
        if self.project_dirty {
            return Err("Save the Source project before queueing conversion.".to_owned());
        }
        if inspections.is_empty() {
            return Err("Select at least one Face for conversion.".to_owned());
        }
        if inspections.iter().any(|inspection| !inspection.ready()) {
            return Err("Resolve blocking per-Face preflight findings before conversion.".to_owned());
        }
        if plan.recipes.len() != inspections.len() || plan.output_paths.len() != inspections.len() {
            return Err("Unified conversion plan Face cardinality changed before capture.".to_owned());
        }
        if let Some(parent) = plan.production_project_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Cannot create Production destination folder {}: {error}",
                    parent.display()
                )
            })?;
        }

        let captured_project: windows_shade_editor::model::ShadeProject =
            serde_json::from_value(serde_json::to_value(&self.project).map_err(|error| {
                format!("Cannot serialize Source project for conversion capture: {error}")
            })?)
            .map_err(|error| format!("Cannot materialize Source project capture: {error}"))?;
        let source_project_sha_before =
            windows_shade_editor::icc_conversion_worker::sha256_file(&source_project_path)?;
        let production_project_name = format!(
            "{} - {}",
            self.project.name,
            plan.recipes
                .first()
                .map(|recipe| recipe.target.name.as_str())
                .unwrap_or("Production")
        );

        let mut face_captures = Vec::with_capacity(inspections.len());
        for ((inspection, recipe), output_path) in inspections
            .iter()
            .zip(plan.recipes.iter())
            .zip(plan.output_paths.iter())
        {
            let source_file_sha256 =
                windows_shade_editor::icc_conversion_worker::sha256_file(&inspection.source_path)
                    .map_err(|error| {
                        format!(
                            "Face {} ('{}') could not be hashed: {error}",
                            inspection.index + 1,
                            inspection.label
                        )
                    })?;
            let capture = ConversionJobCapture::capture(
                &captured_project,
                source_project_path.clone(),
                source_project_sha_before.clone(),
                inspection.source_path.clone(),
                self.project.active_snapshot_id,
                source_file_sha256,
                inspection.captured_profile.clone(),
                recipe.clone(),
                plan.output_policy,
                output_path.clone(),
                plan.production_project_path.clone(),
                production_project_name.clone(),
                inspection.label.clone(),
            )
            .map_err(|error| {
                format!(
                    "Face {} ('{}') capture failed: {error}",
                    inspection.index + 1,
                    inspection.label
                )
            })?;
            face_captures.push(ConversionBatchFaceCapture {
                source_face_index: inspection.index,
                capture,
            });
        }

        let source_project_sha_after =
            windows_shade_editor::icc_conversion_worker::sha256_file(&source_project_path)?;
        if !source_project_sha_before.eq_ignore_ascii_case(&source_project_sha_after) {
            return Err(
                "Source project changed while conversion was being captured. Save it and queue again."
                    .to_owned(),
            );
        }

        let batch = ConversionBatchCapture::capture(
            scope,
            self.project.faces.len(),
            plan.disposition,
            face_captures,
        )?;
        self.conversion_batch
            .queue
            .enqueue(batch, self.settings.default_dpi)
    }

    pub(crate) fn ui_unified_conversion_queue(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.conversion_batch.startup_error.as_deref() {
            ui.label(
                egui::RichText::new(format!("Queue recovery warning: {error}"))
                    .color(egui::Color32::LIGHT_RED),
            );
        }
        let rows = self.conversion_batch.queue.items().to_vec();
        let paused = self.conversion_batch.queue.is_paused();
        let recovered_waiting = self.conversion_batch.queue.recovered_waiting_count();
        let mut actions = Vec::new();
        render_batch_queue(ui, &rows, paused, recovered_waiting, &mut actions);
        for action in actions {
            self.dispatch_conversion_batch_queue_action(action);
        }

        if !self.conversion_queue.items().is_empty() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Legacy single-conversion queue")
                    .color(egui::Color32::YELLOW),
            );
            ui.small(
                "These items were persisted by an older Shade Editor build. New conversions use the unified durable batch runtime; legacy items remain polled for compatibility.",
            );
            for item in self.conversion_queue.items() {
                ui.small(format!("#{} · {} · {}", item.id, item.label, item.status.label()));
            }
        }
    }

    fn dispatch_conversion_batch_queue_action(&mut self, action: BatchQueueUiAction) {
        match action {
            BatchQueueUiAction::ResumeRecovered => {
                let count = self.conversion_batch.queue.resume_recovered();
                self.report_info(format!("Resumed {count} recovered conversion batch(es)"));
            }
            BatchQueueUiAction::TogglePaused => {
                let paused = self.conversion_batch.queue.is_paused();
                self.conversion_batch.queue.set_paused(!paused);
            }
            BatchQueueUiAction::Cancel(id) => {
                self.conversion_batch.queue.cancel(id);
            }
            BatchQueueUiAction::Retry(id) => {
                self.conversion_batch.queue.retry(id);
            }
            BatchQueueUiAction::Recover(id) => {
                let source_path = self
                    .conversion_batch
                    .queue
                    .items()
                    .iter()
                    .find(|item| item.id == id)
                    .map(|item| item.source_project_path.clone());
                match self.conversion_batch.queue.recover(id) {
                    Ok(completion) => {
                        self.handle_conversion_batch_completion(completion, source_path.as_deref());
                    }
                    Err(error) => self.report_error(format!(
                        "Production project-only recovery blocked: {error}"
                    )),
                }
            }
            BatchQueueUiAction::ClearFinished => {
                self.conversion_batch.queue.clear_finished();
            }
        }
    }

    fn handle_conversion_batch_completion(
        &mut self,
        completion: ConversionBatchQueueCompletion,
        source_project_path: Option<&Path>,
    ) {
        match completion.result {
            ConversionBatchQueueCompletionResult::CompletedFace {
                completed,
                ordinal,
                batch_complete,
            } => {
                let current_source = source_project_path.is_some_and(|source_path| {
                    self.project_path
                        .as_deref()
                        .is_some_and(|current| paths_match(current, source_path))
                });
                if current_source {
                    let source_project_path = self
                        .project_path
                        .as_deref()
                        .expect("current Source path was verified above");
                    match sync_open_source_project_to_production_route(
                        &mut self.project,
                        source_project_path,
                        &completed.production_project_path,
                        &completed.production_project,
                    ) {
                        Ok(()) => {
                            self.mark_project_dirty();
                            self.log.info(
                                "Production linkage/route changed the open Source project; explicit Save is required.",
                            );
                        }
                        Err(error) => self.log.error(&format!(
                            "Could not mirror Production conversion route in the open Source project: {error}"
                        )),
                    }
                }
                if batch_complete {
                    self.report_info(format!(
                        "Production conversion #{} complete: {}",
                        completion.id,
                        completed.production_project_path.display()
                    ));
                } else {
                    self.report_info(format!(
                        "Conversion #{} Face {} committed (checkpoint {}); continuing",
                        completion.id,
                        completion.source_face_index + 1,
                        ordinal + 1
                    ));
                }
            }
            ConversionBatchQueueCompletionResult::Cancelled { phase, message } => {
                self.report_info(format!(
                    "Production conversion #{} cancelled at {phase}: {message}",
                    completion.id
                ));
            }
            ConversionBatchQueueCompletionResult::Failed { phase, error } => {
                self.report_error(format!(
                    "Production conversion #{} Face {} failed at {phase}: {error}",
                    completion.id,
                    completion.source_face_index + 1
                ));
            }
            ConversionBatchQueueCompletionResult::NeedsRecovery(recovery) => {
                self.report_error(format!(
                    "Production conversion #{} Face {} committed its TIFF but needs project-only recovery before continuing: {}",
                    completion.id,
                    recovery.source_face_index + 1,
                    recovery.recovery.error
                ));
            }
        }
    }
}

fn sync_open_source_project_to_production_route(
    source: &mut model::ShadeProject,
    source_project_path: &Path,
    production_project_path: &Path,
    production_project: &windows_shade_editor::model::ShadeProject,
) -> Result<(), String> {
    let value = serde_json::to_value(&*source)
        .map_err(|error| format!("Cannot bridge open Source project for route persistence: {error}"))?;
    let mut shared_source = serde_json::from_value::<windows_shade_editor::model::ShadeProject>(value)
        .map_err(|error| format!("Cannot decode open Source project for route persistence: {error}"))?;
    let route = windows_shade_editor::conversion_route::build_conversion_route_record(
        &shared_source,
        source_project_path,
        production_project,
        production_project_path,
    )?;
    windows_shade_editor::production_project::link_source_project_to_production(
        &mut shared_source,
        production_project_path,
    )?;
    windows_shade_editor::conversion_route::upsert_conversion_route(&mut shared_source, route)?;
    let value = serde_json::to_value(shared_source)
        .map_err(|error| format!("Cannot serialize persisted Source conversion route: {error}"))?;
    *source = serde_json::from_value::<model::ShadeProject>(value)
        .map_err(|error| format!("Cannot restore open Source project after route persistence: {error}"))?;
    Ok(())
}

fn render_batch_queue(
    ui: &mut egui::Ui,
    rows: &[ConversionBatchQueueItem],
    paused: bool,
    recovered_waiting: usize,
    actions: &mut Vec<BatchQueueUiAction>,
) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(if paused { "Resume queue" } else { "Pause queue" })
            .clicked()
        {
            actions.push(BatchQueueUiAction::TogglePaused);
        }
        if recovered_waiting > 0
            && ui
                .button(format!("Resume {recovered_waiting} recovered"))
                .clicked()
        {
            actions.push(BatchQueueUiAction::ResumeRecovered);
        }
        if rows.iter().any(|row| {
            matches!(
                row.status,
                ConversionBatchQueueStatus::Done
                    | ConversionBatchQueueStatus::Failed
                    | ConversionBatchQueueStatus::Cancelled
            )
        }) && ui.button("Clear finished").clicked()
        {
            actions.push(BatchQueueUiAction::ClearFinished);
        }
    });
    if rows.is_empty() {
        ui.small("No Production Color Conversion jobs queued.");
        return;
    }
    for row in rows {
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("#{} {}", row.id, row.label));
                ui.label(row.status.label());
                ui.label(format!(
                    "{} / {} Faces",
                    row.completed_face_count, row.face_count
                ));
                if row.requires_resume {
                    ui.label(
                        egui::RichText::new("Recovered / paused")
                            .color(egui::Color32::YELLOW),
                    );
                }
            });
            if row.status == ConversionBatchQueueStatus::Processing {
                ui.add(
                    egui::ProgressBar::new(row.progress.clamp(0.0, 1.0))
                        .show_percentage()
                        .text(if row.phase.is_empty() {
                            row.detail.clone()
                        } else {
                            format!("{} - {}", row.phase, row.detail)
                        }),
                );
            } else if !row.detail.is_empty() {
                ui.small(&row.detail);
            }
            if let Some(source) = row.current_source.as_deref() {
                ui.small(format!("Current Source: {}", source.display()));
            }
            if let Some(destination) = row.current_destination.as_deref() {
                ui.small(format!("Current TIFF: {}", destination.display()));
            }
            ui.small(format!(
                "Production project: {}",
                row.production_project_path.display()
            ));
            if let Some(error) = row.error.as_deref() {
                ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
            }
            ui.horizontal_wrapped(|ui| match row.status {
                ConversionBatchQueueStatus::Waiting | ConversionBatchQueueStatus::Processing => {
                    if ui.button("Cancel").clicked() {
                        actions.push(BatchQueueUiAction::Cancel(row.id));
                    }
                }
                ConversionBatchQueueStatus::Failed | ConversionBatchQueueStatus::Cancelled => {
                    if ui.button("Retry from checkpoint").clicked() {
                        actions.push(BatchQueueUiAction::Retry(row.id));
                    }
                }
                ConversionBatchQueueStatus::NeedsRecovery => {
                    if ui
                        .button("Recover Production Project")
                        .on_hover_text(
                            "Replays only the exact captured Production-project save. The committed TIFF is verified and never rendered again.",
                        )
                        .clicked()
                    {
                        actions.push(BatchQueueUiAction::Recover(row.id));
                    }
                }
                ConversionBatchQueueStatus::Done => {}
            });
        });
    }
}

fn batch_queue_blocks_other_work(queue: &ConversionBatchQueue) -> bool {
    queue.is_active()
        || queue.items().iter().any(|item| {
            item.status == ConversionBatchQueueStatus::NeedsRecovery
                || (item.status == ConversionBatchQueueStatus::Waiting && !item.requires_resume)
        })
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn paths_match(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

#[cfg(test)]
mod tests {
    #[test]
    fn batch_runtime_contains_no_operator_target_config_or_window() {
        let source = include_str!("conversion_batch.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("struct ConversionBatchUiConfig"));
        assert!(!runtime.contains("egui::Window::new"));
        assert!(!runtime.contains("Batch Convert"));
        assert!(runtime.contains("ConversionBatchCapture::capture"));
        assert!(runtime.contains("ConversionBatchQueue::load_persistent"));
        assert!(runtime.contains("queue_unified_conversion_plan"));
    }
}
