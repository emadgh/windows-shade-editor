use crate::*;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use windows_shade_editor::conversion_recipe::recipe_sha256;
use windows_shade_editor::conversion_route_migration::prepare_route_migration_plan;
use windows_shade_editor::conversion_route_migration_discovery::discover_pending_route_migration;
use windows_shade_editor::conversion_route_migration_runtime::{
    execute_new_route_migration, resume_route_migration,
};
use windows_shade_editor::conversion_transaction::{
    CapturedOutputPolicy, CapturedSourceProfile, ConversionCancellation, ConversionJobCapture,
};
use windows_shade_editor::model::ConversionRouteRecord;
use windows_shade_editor::reconversion_policy::analyze_replacement_risk;

use super::conversion_plan::{
    UnifiedDestinationMode, build_conversion_recipe, inspect_conversion_face, production_routes,
};

const CONVERSION_WINDOW_ID: &str = "shade-editor-color-conversion-open";
const ROUTE_MIGRATION_DECISION_ID: &str = "shade-editor-route-migration-decision";
const ROUTE_MIGRATION_MAILBOX_ID: &str = "shade-editor-route-migration-mailbox";

#[derive(Clone, Default)]
struct RouteMigrationDecisionState {
    decision_key: String,
    confirm_destructive_migration: bool,
    acknowledge_production_work: bool,
}

#[derive(Clone)]
struct RouteMigrationDraftFace {
    source_face_index: usize,
    source_path: PathBuf,
    label: String,
    source_profile: CapturedSourceProfile,
    recipe: windows_shade_editor::color_conversion::ConversionRecipe,
    output_path: PathBuf,
}

#[derive(Clone)]
struct RouteMigrationDraft {
    route: ConversionRouteRecord,
    source_project: windows_shade_editor::model::ShadeProject,
    source_project_path: PathBuf,
    production_project_path: PathBuf,
    faces: Vec<RouteMigrationDraftFace>,
    recipe_drift: bool,
    destination_drift: bool,
    target_compatibility_changed: bool,
    production_work_warnings: Vec<String>,
    matching_other_route: Option<ConversionRouteRecord>,
    decision_key: String,
}

impl RouteMigrationDraft {
    fn requires_production_work_ack(&self) -> bool {
        !self.production_work_warnings.is_empty()
    }

    fn can_migrate(&self) -> bool {
        self.recipe_drift && !self.destination_drift
    }
}

#[derive(Clone)]
struct RouteMigrationCompletion {
    source_project_path: PathBuf,
    production_project_path: PathBuf,
    production_project: windows_shade_editor::model::ShadeProject,
}

#[derive(Clone)]
struct RouteMigrationStartRequest {
    source_project: windows_shade_editor::model::ShadeProject,
    source_project_path: PathBuf,
    production_project_path: PathBuf,
    faces: Vec<RouteMigrationDraftFace>,
    allow_production_work_discard: bool,
}

/// Migration itself runs through ShadeApp's global `launch_job`, so New/Open/Exit and every other
/// foreground operation see `self.job.is_some()`. This mailbox is intentionally not a second job
/// runtime: it carries only cancellation/progress ownership metadata and the completed Production
/// project needed to refresh the open Source-side route mirror after `poll_job` releases the lock.
#[derive(Clone, Default)]
struct RouteMigrationMailbox {
    shared: Arc<Mutex<RouteMigrationMailboxState>>,
}

#[derive(Default)]
struct RouteMigrationMailboxState {
    active: bool,
    production_project_path: Option<PathBuf>,
    cancellation: ConversionCancellation,
    outcome: Option<Result<RouteMigrationCompletion, String>>,
    restore_export_reminder: Option<bool>,
}

#[derive(Clone, Default)]
struct RouteMigrationMailboxSnapshot {
    active: bool,
    production_project_path: Option<PathBuf>,
}

impl RouteMigrationMailbox {
    fn begin(
        &self,
        production_project_path: PathBuf,
        cancellation: ConversionCancellation,
        restore_export_reminder: bool,
    ) -> Result<(), String> {
        let mut state = self
            .shared
            .lock()
            .map_err(|_| "Route migration mailbox lock is poisoned.".to_owned())?;
        if state.active {
            return Err("A route migration is already running.".to_owned());
        }
        *state = RouteMigrationMailboxState {
            active: true,
            production_project_path: Some(production_project_path),
            cancellation,
            outcome: None,
            restore_export_reminder: Some(restore_export_reminder),
        };
        Ok(())
    }

    fn finish(&self, outcome: Result<RouteMigrationCompletion, String>) {
        if let Ok(mut state) = self.shared.lock() {
            state.active = false;
            state.outcome = Some(outcome);
        }
    }

    fn cancel(&self) {
        if let Ok(state) = self.shared.lock() {
            state.cancellation.request();
        }
    }

    fn snapshot(&self) -> RouteMigrationMailboxSnapshot {
        match self.shared.lock() {
            Ok(state) => RouteMigrationMailboxSnapshot {
                active: state.active,
                production_project_path: state.production_project_path.clone(),
            },
            Err(_) => RouteMigrationMailboxSnapshot::default(),
        }
    }

    fn take_finished(
        &self,
    ) -> Option<(Result<RouteMigrationCompletion, String>, Option<bool>)> {
        let mut state = self.shared.lock().ok()?;
        if state.active {
            return None;
        }
        let outcome = state.outcome.take()?;
        let reminder = state.restore_export_reminder.take();
        state.production_project_path = None;
        Some((outcome, reminder))
    }
}

impl ShadeApp {
    /// Supplemental destructive-route decision/recovery surface for the single unified Production
    /// Color Conversion workflow. Normal same-route reconversion remains in the durable batch
    /// runtime; only recipe drift for an already-owned route enters this explicit migration path.
    pub(crate) fn ui_conversion_route_migration(&mut self, ctx: &egui::Context) {
        let mailbox = route_migration_mailbox(ctx);
        let snapshot = mailbox.snapshot();
        if snapshot.active {
            render_active_migration_window(ctx, &mailbox, &snapshot);
            ctx.request_repaint();
            return;
        }

        // `poll_job` clears the global job before this status-bar surface is rendered. Only after
        // that happens do we consume the migration result and restore the unrelated Export reminder
        // bit that was temporarily suppressed because JobResult::Export is used solely as the
        // existing no-op completion envelope for the global foreground worker.
        if self.job.is_none() {
            if let Some((outcome, restore_export_reminder)) = mailbox.take_finished() {
                if let Some(value) = restore_export_reminder {
                    self.export.remind_after_export = value;
                }
                self.handle_route_migration_outcome(outcome);
            }
        }

        if !conversion_window_open(ctx) {
            return;
        }
        if self.color_conversion.destination_mode != UnifiedDestinationMode::AppendExisting {
            return;
        }
        let Some(selected_path) = self.color_conversion.selected_existing.clone() else {
            return;
        };

        let exclusion_error = self.route_migration_exclusion_error();
        match discover_pending_route_migration(&selected_path) {
            Ok(Some(journal)) => {
                let stage = format!("{:?}", journal.checkpoint.stage);
                let staged = journal.checkpoint.staged_outputs.len();
                let committed = journal.checkpoint.committed_outputs.len();
                let total = journal.plan.faces.len();
                let source_project_path = journal.plan.source_project_path.clone();
                let production_project_path = journal.plan.production_project_path.clone();
                let mut resume_requested = false;
                egui::Window::new("Recover Conversion Route Migration")
                    .id(egui::Id::new("route-migration-recovery-window"))
                    .collapsible(false)
                    .resizable(true)
                    .default_width(620.0)
                    .show(ctx, |ui| {
                        ui.label(
                            egui::RichText::new(
                                "An unfinished destructive route migration owns this Production project. A new conversion cannot replace its journal.",
                            )
                            .color(egui::Color32::YELLOW)
                            .strong(),
                        );
                        ui.small(format!(
                            "Stage: {stage} · staged {staged}/{total} · committed {committed}/{total}"
                        ));
                        ui.small(production_project_path.display().to_string());
                        if let Some(error) = exclusion_error.as_deref() {
                            ui.label(egui::RichText::new(error).color(egui::Color32::YELLOW));
                        }
                        ui.add_space(8.0);
                        if ui
                            .add_enabled(
                                exclusion_error.is_none(),
                                egui::Button::new("Resume exact saved migration"),
                            )
                            .clicked()
                        {
                            resume_requested = true;
                        }
                    });
                if resume_requested {
                    match self.start_resume_route_migration_job(
                        &mailbox,
                        production_project_path,
                        source_project_path,
                    ) {
                        Ok(()) => self.report_info(
                            "Resuming the exact persisted conversion-route migration journal under the global operation lock.",
                        ),
                        Err(error) => self.report_error(error),
                    }
                }
                return;
            }
            Ok(None) => {}
            Err(error) => {
                egui::Window::new("Conversion Route Recovery Blocked")
                    .id(egui::Id::new("route-migration-recovery-error"))
                    .collapsible(false)
                    .show(ctx, |ui| {
                        ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
                    });
                return;
            }
        }

        let draft = match self.build_route_migration_draft(&selected_path) {
            Ok(Some(draft)) => draft,
            Ok(None) => return,
            Err(error) => {
                egui::Window::new("Conversion Route Change Blocked")
                    .id(egui::Id::new("route-migration-draft-error"))
                    .collapsible(false)
                    .resizable(true)
                    .default_width(650.0)
                    .show(ctx, |ui| {
                        ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
                        ui.small(
                            "No existing output will be overwritten. Restore the saved route settings or create a separate Production route.",
                        );
                        if ui.button("Create new conversion route / Production link").clicked() {
                            self.select_new_route_mode();
                        }
                    });
                return;
            }
        };

        let mut decision = route_migration_decision(ctx);
        if decision.decision_key != draft.decision_key {
            decision = RouteMigrationDecisionState {
                decision_key: draft.decision_key.clone(),
                ..RouteMigrationDecisionState::default()
            };
        }

        let mut choose_matching_route: Option<PathBuf> = None;
        let mut choose_new_route = false;
        let mut start_migration = false;
        egui::Window::new("Existing Conversion Route Changed")
            .id(egui::Id::new("route-migration-decision-window"))
            .collapsible(false)
            .resizable(true)
            .default_width(720.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "This saved route already owns {} Production Face(s), but the current conversion decision no longer matches it.",
                        draft.route.converted_face_count()
                    ))
                    .color(egui::Color32::YELLOW)
                    .strong(),
                );
                ui.small(format!(
                    "Production project: {}",
                    draft.production_project_path.display()
                ));
                ui.small(format!(
                    "Saved route policy: {}",
                    short_hash(&draft.route.batch_recipe_policy_sha256)
                ));

                if draft.destination_drift {
                    ui.label(
                        egui::RichText::new(format!(
                            "The selected route owns destination folder {}. Destructive migration cannot redirect its output mapping to the currently selected folder.",
                            draft.route.output_folder().display()
                        ))
                        .color(egui::Color32::LIGHT_RED),
                    );
                }
                if draft.target_compatibility_changed {
                    ui.label(
                        egui::RichText::new(
                            "Target compatibility changes. The Production project must be rebuilt in the new target channel space after all replacement TIFFs are staged.",
                        )
                        .color(egui::Color32::YELLOW),
                    );
                }
                for warning in &draft.production_work_warnings {
                    ui.label(egui::RichText::new(warning).color(egui::Color32::YELLOW));
                }
                if let Some(error) = exclusion_error.as_deref() {
                    ui.label(egui::RichText::new(error).color(egui::Color32::YELLOW));
                }

                ui.add_space(6.0);
                egui::CollapsingHeader::new(format!(
                    "Affected route outputs ({})",
                    draft.faces.len()
                ))
                .default_open(false)
                .show(ui, |ui| {
                    for face in &draft.faces {
                        ui.small(format!(
                            "Face {} · {} → {}",
                            face.source_face_index + 1,
                            face.label,
                            face.output_path.display()
                        ));
                    }
                });

                if let Some(other) = draft.matching_other_route.as_ref() {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "The current recipes already match another saved route. Reusing it avoids destructive migration and duplicate route history.",
                        )
                        .color(egui::Color32::LIGHT_GREEN),
                    );
                    if ui.button("Use matching existing conversion route").clicked() {
                        choose_matching_route = Some(other.production_project_path());
                    }
                }

                ui.separator();
                ui.checkbox(
                    &mut decision.confirm_destructive_migration,
                    format!(
                        "I confirm replacing/migrating all {} existing route Face output(s)",
                        draft.faces.len()
                    ),
                );
                if draft.requires_production_work_ack() {
                    ui.checkbox(
                        &mut decision.acknowledge_production_work,
                        "I understand existing Production adjustments/Snapshots may be invalidated by replacing the route base outputs",
                    );
                }
                if self.project_dirty {
                    ui.label(
                        egui::RichText::new(
                            "Save the Source project before destructive migration so the capture has one stable Source project SHA-256.",
                        )
                        .color(egui::Color32::LIGHT_RED),
                    );
                }

                ui.horizontal_wrapped(|ui| {
                    let confirmed = decision.confirm_destructive_migration
                        && (!draft.requires_production_work_ack()
                            || decision.acknowledge_production_work)
                        && !self.project_dirty
                        && self.project_path.is_some()
                        && draft.can_migrate()
                        && exclusion_error.is_none();
                    if ui
                        .add_enabled(
                            confirmed,
                            egui::Button::new("Replace / migrate this existing conversion route"),
                        )
                        .clicked()
                    {
                        start_migration = true;
                    }
                    if ui.button("Create new conversion route / Production link").clicked() {
                        choose_new_route = true;
                    }
                });
                if !draft.can_migrate() {
                    ui.small(
                        "Migration is disabled because this change redirects the route destination rather than changing only the owned conversion recipe. Create a new route instead.",
                    );
                }
            });

        set_route_migration_decision(ctx, decision.clone());

        if let Some(path) = choose_matching_route {
            if let Some(route) = production_routes(self)
                .into_iter()
                .find(|route| paths_match(&route.production_project_path(), &path))
            {
                self.color_conversion.destination_mode = UnifiedDestinationMode::AppendExisting;
                self.color_conversion.selected_existing = Some(route.production_project_path());
                self.color_conversion.output_folder = Some(route.output_folder());
                self.color_conversion.allow_production_work_discard = false;
                set_route_migration_decision(ctx, RouteMigrationDecisionState::default());
                self.report_info(
                    "Selected the matching saved conversion route instead of migrating the previous route.",
                );
            }
        } else if choose_new_route {
            self.select_new_route_mode();
            set_route_migration_decision(ctx, RouteMigrationDecisionState::default());
        } else if start_migration {
            let request = RouteMigrationStartRequest {
                source_project: draft.source_project,
                source_project_path: draft.source_project_path,
                production_project_path: draft.production_project_path,
                faces: draft.faces,
                allow_production_work_discard: decision.acknowledge_production_work,
            };
            match self.start_new_route_migration_job(&mailbox, request) {
                Ok(()) => self.report_info(
                    "Started project-wide conversion-route migration under the global operation lock. All replacement TIFFs will be staged before the destructive commit boundary.",
                ),
                Err(error) => self.report_error(error),
            }
        }
    }

    fn route_migration_exclusion_error(&self) -> Option<String> {
        if self.job.is_some() {
            return Some(
                "Finish the current foreground operation before starting or recovering route migration."
                    .to_owned(),
            );
        }
        if self.export.queue.has_pending() {
            return Some(
                "Finish or cancel the Export Queue before destructive route migration."
                    .to_owned(),
            );
        }
        if self.conversion_queue.has_pending() {
            return Some(
                "Finish or cancel the legacy Conversion Queue before destructive route migration."
                    .to_owned(),
            );
        }
        if self.conversion_batch_blocks_project_transition() {
            return Some(
                "Finish or recover the Production Color Conversion batch queue before destructive route migration."
                    .to_owned(),
            );
        }
        None
    }

    fn start_new_route_migration_job(
        &mut self,
        mailbox: &RouteMigrationMailbox,
        request: RouteMigrationStartRequest,
    ) -> Result<(), String> {
        if let Some(error) = self.route_migration_exclusion_error() {
            return Err(error);
        }
        let cancellation = ConversionCancellation::default();
        let previous_export_reminder = self.export.remind_after_export;
        self.export.remind_after_export = false;
        mailbox.begin(
            request.production_project_path.clone(),
            cancellation.clone(),
            previous_export_reminder,
        )?;

        let worker_mailbox = mailbox.clone();
        let completion_source = request.source_project_path.clone();
        let completion_production = request.production_project_path.clone();
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Migrating conversion route", move |progress| {
            let outcome = worker_guard::catch_result("Route migration worker", || {
                execute_migration_request(
                    request,
                    default_dpi,
                    &cancellation,
                    |ordinal, total, item| {
                        Self::set_progress(
                            &progress,
                            Some(item.fraction),
                            "Migrating conversion route",
                            &format!(
                                "Face {} of {} · {} — {}",
                                ordinal + 1,
                                total,
                                item.phase.label(),
                                item.detail
                            ),
                        );
                    },
                )
            })
            .map(|production_project| RouteMigrationCompletion {
                source_project_path: completion_source,
                production_project_path: completion_production,
                production_project,
            });
            worker_mailbox.finish(outcome);
            JobResult::Export(SnapshotExportBatchResult {
                result: Ok("Conversion route migration worker finished".to_owned()),
                marks: Vec::new(),
            })
        });
        Ok(())
    }

    fn start_resume_route_migration_job(
        &mut self,
        mailbox: &RouteMigrationMailbox,
        production_project_path: PathBuf,
        source_project_path: PathBuf,
    ) -> Result<(), String> {
        if let Some(error) = self.route_migration_exclusion_error() {
            return Err(error);
        }
        let cancellation = ConversionCancellation::default();
        let previous_export_reminder = self.export.remind_after_export;
        self.export.remind_after_export = false;
        mailbox.begin(
            production_project_path.clone(),
            cancellation.clone(),
            previous_export_reminder,
        )?;

        let worker_mailbox = mailbox.clone();
        let completion_source = source_project_path.clone();
        let completion_production = production_project_path.clone();
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Recovering conversion route", move |progress| {
            let outcome = worker_guard::catch_result("Route migration recovery worker", || {
                resume_route_migration(
                    &production_project_path,
                    default_dpi,
                    &cancellation,
                    |ordinal, total, item| {
                        Self::set_progress(
                            &progress,
                            Some(item.fraction),
                            "Recovering conversion route",
                            &format!(
                                "Face {} of {} · {} — {}",
                                ordinal + 1,
                                total,
                                item.phase.label(),
                                item.detail
                            ),
                        );
                    },
                )
            })
            .map(|production_project| RouteMigrationCompletion {
                source_project_path: completion_source,
                production_project_path: completion_production,
                production_project,
            });
            worker_mailbox.finish(outcome);
            JobResult::Export(SnapshotExportBatchResult {
                result: Ok("Conversion route recovery worker finished".to_owned()),
                marks: Vec::new(),
            })
        });
        Ok(())
    }

    fn build_route_migration_draft(
        &self,
        selected_path: &Path,
    ) -> Result<Option<RouteMigrationDraft>, String> {
        let routes = production_routes(self);
        let route = routes
            .iter()
            .find(|route| paths_match(&route.production_project_path(), selected_path))
            .cloned()
            .ok_or_else(|| {
                "The selected Production project no longer has a valid persisted Source-side conversion route mirror."
                    .to_owned()
            })?;
        route.validate()?;
        let source_project_path = self
            .project_path
            .clone()
            .ok_or_else(|| "Save the Source project before changing a persisted route.".to_owned())?;
        let source_project: windows_shade_editor::model::ShadeProject = serde_json::from_value(
            serde_json::to_value(&self.project)
                .map_err(|error| format!("Cannot capture Source project for route migration: {error}"))?,
        )
        .map_err(|error| format!("Cannot materialize Source project for route migration: {error}"))?;

        let mut faces = Vec::with_capacity(route.faces.len());
        let mut recipe_drift = false;
        let mut recipe_hashes = Vec::with_capacity(route.faces.len());
        for route_face in &route.faces {
            let source_path = PathBuf::from(&route_face.provenance.source.source_face_path);
            let source_face_index = self
                .faces
                .iter()
                .position(|face| paths_match(&face.path, &source_path))
                .ok_or_else(|| {
                    format!(
                        "Saved route Source Face is not currently available: {}. Relink it before migrating the route.",
                        source_path.display()
                    )
                })?;
            let saved_transparency = route_face.provenance.recipe.source_transparency_policy;
            let selected_transparency = self
                .color_conversion
                .transparency_policies
                .get(&source_face_index)
                .copied();
            if saved_transparency.is_some() && selected_transparency.is_none() {
                return Err(format!(
                    "Face {} has a saved route transparency policy that is not currently restored. Use 'Restore saved route settings' before changing this route.",
                    source_face_index + 1
                ));
            }
            let inspection = inspect_conversion_face(
                self,
                source_face_index,
                selected_transparency.as_ref(),
            );
            if !inspection.ready() {
                return Err(format!(
                    "Face {} ('{}') is not ready for project-wide route migration. Resolve its blocking production preflight findings first.",
                    source_face_index + 1,
                    inspection.label
                ));
            }
            let recipe = build_conversion_recipe(
                &self.color_conversion.target,
                &inspection,
                selected_transparency,
            )?;
            recipe_drift |= recipe != route_face.provenance.recipe;
            recipe_hashes.push(recipe_sha256(&recipe)?);
            faces.push(RouteMigrationDraftFace {
                source_face_index,
                source_path: inspection.source_path,
                label: inspection.label,
                source_profile: inspection.captured_profile,
                recipe,
                output_path: PathBuf::from(&route_face.provenance.output_path),
            });
        }

        let destination_drift = self
            .color_conversion
            .output_folder
            .as_deref()
            .is_some_and(|folder| !paths_match(folder, &route.output_folder()));
        if !recipe_drift && !destination_drift {
            return Ok(None);
        }

        let production_project_path = route.production_project_path();
        let production_project = windows_shade_editor::model::ShadeProject::load(
            &production_project_path,
        )
        .map_err(|error| {
            format!(
                "Cannot inspect selected Production project before route migration: {error}"
            )
        })?;
        let mut production_work_warnings = Vec::new();
        for provenance in &production_project.production_provenance {
            let risk = analyze_replacement_risk(
                &production_project,
                &production_project_path,
                provenance,
            )?;
            if let Some(warning) = risk.warning {
                if !production_work_warnings.contains(&warning) {
                    production_work_warnings.push(warning);
                }
            }
        }

        let target_compatibility_changed = route
            .baseline_recipe()
            .zip(faces.first().map(|face| &face.recipe))
            .is_some_and(|(old, new)| !same_target_compatibility(old, new));
        let matching_other_route = routes
            .into_iter()
            .filter(|other| !paths_match(&other.production_project_path(), &production_project_path))
            .find(|other| route_exactly_matches_current_recipes(other, &faces));
        let decision_key = format!(
            "{}|{}|{}",
            production_project_path.to_string_lossy().to_ascii_lowercase(),
            self.color_conversion
                .output_folder
                .as_deref()
                .map(|path| path.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
            recipe_hashes.join(":")
        );

        Ok(Some(RouteMigrationDraft {
            route,
            source_project,
            source_project_path,
            production_project_path,
            faces,
            recipe_drift,
            destination_drift,
            target_compatibility_changed,
            production_work_warnings,
            matching_other_route,
            decision_key,
        }))
    }

    fn select_new_route_mode(&mut self) {
        self.color_conversion.destination_mode = UnifiedDestinationMode::CreateNew;
        self.color_conversion.selected_existing = None;
        self.color_conversion.allow_production_work_discard = false;
        self.report_info(
            "Preserving the existing route. Choose a route-safe destination for the new Production link; different-route TIFF collisions will remain blocked.",
        );
    }

    fn handle_route_migration_outcome(
        &mut self,
        outcome: Result<RouteMigrationCompletion, String>,
    ) {
        match outcome {
            Ok(completion) => {
                let current_source = self
                    .project_path
                    .as_deref()
                    .is_some_and(|path| paths_match(path, &completion.source_project_path));
                if current_source {
                    match sync_open_source_route_after_migration(
                        &mut self.project,
                        &completion.source_project_path,
                        &completion.production_project_path,
                        &completion.production_project,
                    ) {
                        Ok(()) => {
                            self.mark_project_dirty();
                            self.report_info(format!(
                                "Conversion route migration completed: {}. The Source route mirror was refreshed; save the Source project to persist it.",
                                completion.production_project_path.display()
                            ));
                        }
                        Err(error) => self.report_error(format!(
                            "Production route migration completed, but the open Source route mirror could not be refreshed: {error}"
                        )),
                    }
                } else {
                    self.report_info(format!(
                        "Conversion route migration completed: {}. Reopen the owning Source project to refresh its route mirror.",
                        completion.production_project_path.display()
                    ));
                }
            }
            Err(error) => self.report_error(format!("Conversion route migration failed: {error}")),
        }
    }
}

fn execute_migration_request<F>(
    request: RouteMigrationStartRequest,
    default_dpi: f64,
    cancellation: &ConversionCancellation,
    report: F,
) -> Result<windows_shade_editor::model::ShadeProject, String>
where
    F: FnMut(usize, usize, windows_shade_editor::conversion_transaction::ConversionProgress),
{
    if request.faces.is_empty() {
        return Err("Route migration requires at least one affected Face.".to_owned());
    }
    let source_project_sha_before =
        windows_shade_editor::icc_conversion_worker::sha256_file(&request.source_project_path)?;
    let expected_project_sha =
        windows_shade_editor::icc_conversion_worker::sha256_file(&request.production_project_path)?;
    let production_project = windows_shade_editor::model::ShadeProject::load(
        &request.production_project_path,
    )?;
    let production_project_name = if production_project.name.trim().is_empty() {
        request
            .faces
            .first()
            .map(|face| face.recipe.target.name.clone())
            .unwrap_or_else(|| "Production".to_owned())
    } else {
        production_project.name.clone()
    };

    let mut captures = Vec::with_capacity(request.faces.len());
    for face in &request.faces {
        cancellation.check_before_commit()?;
        let source_file_sha =
            windows_shade_editor::icc_conversion_worker::sha256_file(&face.source_path)?;
        captures.push(ConversionJobCapture::capture(
            &request.source_project,
            request.source_project_path.clone(),
            source_project_sha_before.clone(),
            face.source_path.clone(),
            request.source_project.active_snapshot_id,
            source_file_sha,
            face.source_profile.clone(),
            face.recipe.clone(),
            CapturedOutputPolicy::TransactionalReplace,
            face.output_path.clone(),
            request.production_project_path.clone(),
            production_project_name.clone(),
            face.label.clone(),
        )?);
    }
    let source_project_sha_after =
        windows_shade_editor::icc_conversion_worker::sha256_file(&request.source_project_path)?;
    if !source_project_sha_before.eq_ignore_ascii_case(&source_project_sha_after) {
        return Err(
            "Source project changed while destructive route migration was being captured. Save it and start the migration again."
                .to_owned(),
        );
    }

    let plan = prepare_route_migration_plan(
        &production_project,
        &request.production_project_path,
        expected_project_sha,
        captures,
        true,
        request.allow_production_work_discard,
    )?;
    execute_new_route_migration(plan, default_dpi, cancellation, report)
}

fn sync_open_source_route_after_migration(
    source: &mut model::ShadeProject,
    source_project_path: &Path,
    production_project_path: &Path,
    production_project: &windows_shade_editor::model::ShadeProject,
) -> Result<(), String> {
    let value = serde_json::to_value(&*source)
        .map_err(|error| format!("Cannot bridge Source project for route persistence: {error}"))?;
    let mut shared_source = serde_json::from_value::<windows_shade_editor::model::ShadeProject>(value)
        .map_err(|error| format!("Cannot decode Source project for route persistence: {error}"))?;
    let route = windows_shade_editor::conversion_route::build_conversion_route_record(
        &shared_source,
        source_project_path,
        production_project,
        production_project_path,
    )?;
    windows_shade_editor::conversion_route::upsert_conversion_route(&mut shared_source, route)?;
    let value = serde_json::to_value(shared_source)
        .map_err(|error| format!("Cannot encode refreshed Source route mirror: {error}"))?;
    *source = serde_json::from_value(value)
        .map_err(|error| format!("Cannot apply refreshed Source route mirror: {error}"))?;
    Ok(())
}

fn route_exactly_matches_current_recipes(
    route: &ConversionRouteRecord,
    faces: &[RouteMigrationDraftFace],
) -> bool {
    route.validate().is_ok()
        && route.faces.len() == faces.len()
        && faces.iter().all(|face| {
            route
                .face_for_source(&face.source_path)
                .is_some_and(|owned| owned.provenance.recipe == face.recipe)
        })
}

fn same_target_compatibility(
    left: &windows_shade_editor::color_conversion::ConversionRecipe,
    right: &windows_shade_editor::color_conversion::ConversionRecipe,
) -> bool {
    left.engine_mode == right.engine_mode
        && left.target.output_profile_identity == right.target.output_profile_identity
        && left.target.device_link_identity == right.target.device_link_identity
        && left.target.characterization_id == right.target.characterization_id
        && left.target.bit_depth == right.target.bit_depth
        && left
            .target
            .channels
            .iter()
            .map(|channel| channel.name.trim().to_owned())
            .eq(right
                .target
                .channels
                .iter()
                .map(|channel| channel.name.trim().to_owned()))
}

fn render_active_migration_window(
    ctx: &egui::Context,
    mailbox: &RouteMigrationMailbox,
    snapshot: &RouteMigrationMailboxSnapshot,
) {
    egui::Window::new("Conversion Route Migration")
        .id(egui::Id::new("route-migration-active-window"))
        .collapsible(false)
        .resizable(true)
        .default_width(620.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "Project-wide destructive migration is running under the application's global operation lock and a durable recovery journal.",
                )
                .strong(),
            );
            if let Some(path) = snapshot.production_project_path.as_deref() {
                ui.small(path.display().to_string());
            }
            ui.spinner();
            ui.small(
                "Detailed Face/phase progress is also reported through the normal foreground-operation progress surface.",
            );
            ui.add_space(6.0);
            if ui.button("Cancel before destructive commit").clicked() {
                mailbox.cancel();
            }
            ui.small(
                "Cancellation is honored during staging. After all replacement TIFFs are durably staged, Shade Editor finishes the short commit boundary so the Production route cannot be intentionally left mixed.",
            );
        });
}

fn route_migration_mailbox(ctx: &egui::Context) -> RouteMigrationMailbox {
    ctx.data_mut(|data| {
        let id = egui::Id::new(ROUTE_MIGRATION_MAILBOX_ID);
        if let Some(mailbox) = data.get_temp::<RouteMigrationMailbox>(id) {
            mailbox
        } else {
            let mailbox = RouteMigrationMailbox::default();
            data.insert_temp(id, mailbox.clone());
            mailbox
        }
    })
}

fn route_migration_decision(ctx: &egui::Context) -> RouteMigrationDecisionState {
    ctx.data(|data| {
        data.get_temp::<RouteMigrationDecisionState>(egui::Id::new(ROUTE_MIGRATION_DECISION_ID))
            .unwrap_or_default()
    })
}

fn set_route_migration_decision(ctx: &egui::Context, state: RouteMigrationDecisionState) {
    ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new(ROUTE_MIGRATION_DECISION_ID), state)
    });
}

fn conversion_window_open(ctx: &egui::Context) -> bool {
    ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new(CONVERSION_WINDOW_ID))
            .unwrap_or(false)
    })
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_shade_editor::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
        TargetChannelDefinition,
    };
    use windows_shade_editor::model::IccProfileIdentity;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn recipe(target_hash: char) -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash('s'),
            },
            source_transparency_policy: None,
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
                    sha256: hash(target_hash),
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

    #[test]
    fn target_compatibility_detects_profile_change_but_ignores_source_only_change() {
        let old = recipe('a');
        let mut source_only = old.clone();
        source_only.source_profile_identity.sha256 = hash('z');
        assert!(same_target_compatibility(&old, &source_only));
        assert!(!same_target_compatibility(&old, &recipe('b')));
    }

    #[test]
    fn destructive_choice_tokens_are_explicit_and_separate() {
        let source = include_str!("conversion_route_migration.rs");
        assert!(source.contains("Replace / migrate this existing conversion route"));
        assert!(source.contains("Create new conversion route / Production link"));
        assert!(source.contains("Resume exact saved migration"));
        assert!(source.contains("global operation lock"));
        assert!(!source.contains("std::thread::spawn"));
    }
}
