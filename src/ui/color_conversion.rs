use crate::*;
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use windows_shade_editor::color_conversion::{ConversionEngineMode, ConversionRenderingIntent};
use windows_shade_editor::conversion_batch::ConversionBatchScope;
use windows_shade_editor::conversion_preflight::{PreflightCode, PreflightSeverity};
use windows_shade_editor::icc_profile_registry::IccProfileRegistry;
use windows_shade_editor::production_destination::ProductionDestinationAvailability;
use windows_shade_editor::production_profile_catalog::{
    ProductionProfileCandidate, installed_production_profiles,
};
use windows_shade_editor::production_target::inspect_production_target_profile;
use windows_shade_editor::source_transparency::SourceTransparencyPolicy;

use super::conversion_plan::{
    ConversionFaceInspection, ConversionTargetState, UnifiedDestinationMode,
    build_conversion_recipe, build_unified_plan, conversion_color_model, default_output_folder,
    inspect_conversion_face, production_candidates, production_routes, restore_target_from_route,
    scope_indices,
};

const CONVERSION_WINDOW_ID: &str = "shade-editor-color-conversion-open";

#[derive(Clone)]
pub(crate) struct ColorConversionUiState {
    project_key: String,
    pub(crate) target: ConversionTargetState,
    installed_target_profiles: Vec<ProductionProfileCandidate>,
    installed_catalog_key: String,
    installed_profiles_error: Option<String>,
    target_profile_query: String,
    show_incompatible_profiles: bool,
    pub(crate) scope: ConversionBatchScope,
    pub(crate) selected_faces: BTreeSet<usize>,
    pub(crate) transparency_policies: BTreeMap<usize, SourceTransparencyPolicy>,
    pub(crate) output_folder: Option<PathBuf>,
    pub(crate) destination_mode: UnifiedDestinationMode,
    pub(crate) selected_existing: Option<PathBuf>,
    pub(crate) allow_production_work_discard: bool,
}

impl Default for ColorConversionUiState {
    fn default() -> Self {
        Self {
            project_key: String::new(),
            target: ConversionTargetState::default(),
            installed_target_profiles: Vec::new(),
            installed_catalog_key: String::new(),
            installed_profiles_error: None,
            target_profile_query: String::new(),
            show_incompatible_profiles: false,
            scope: ConversionBatchScope::CurrentFace,
            selected_faces: BTreeSet::new(),
            transparency_policies: BTreeMap::new(),
            output_folder: None,
            destination_mode: UnifiedDestinationMode::CreateNew,
            selected_existing: None,
            allow_production_work_discard: false,
        }
    }
}

impl ColorConversionUiState {
    fn bind_project(
        &mut self,
        project_key: String,
        default_folder: Option<PathBuf>,
        current_face: usize,
        face_count: usize,
    ) {
        if self.project_key != project_key {
            *self = Self {
                project_key,
                output_folder: default_folder,
                ..Self::default()
            };
        } else if self.output_folder.is_none() {
            self.output_folder = default_folder;
        }
        self.selected_faces.retain(|index| *index < face_count);
        if self.selected_faces.is_empty() && current_face < face_count {
            self.selected_faces.insert(current_face);
        }
    }

    fn clear_profile_catalog(&mut self) {
        self.installed_target_profiles.clear();
        self.installed_catalog_key.clear();
        self.installed_profiles_error = None;
    }
}

impl ShadeApp {
    pub(crate) fn open_color_conversion(&mut self, ctx: &egui::Context) {
        set_conversion_window_open(ctx, true);
    }

    pub(crate) fn ui_color_conversion_window(&mut self, ctx: &egui::Context) {
        if !conversion_window_open(ctx) {
            return;
        }
        if self.faces.is_empty() {
            set_conversion_window_open(ctx, false);
            self.clear_conversion_candidate();
            return;
        }

        let project_key = self
            .project_path
            .as_deref()
            .map(|path| path.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| format!("<unsaved>:{}", self.project.name));
        let mut state = self.color_conversion.clone();
        state.bind_project(
            project_key,
            default_output_folder(self),
            self.current_face,
            self.project.faces.len(),
        );

        let current_policy = state.transparency_policies.get(&self.current_face).copied();
        let current_inspection =
            inspect_conversion_face(self, self.current_face, current_policy.as_ref());
        ensure_installed_profile_catalog(&mut state, &current_inspection);

        let indices = scope_indices(
            state.scope,
            self.current_face,
            self.project.faces.len(),
            &state.selected_faces,
        );
        let inspections = indices
            .iter()
            .map(|index| {
                inspect_conversion_face(self, *index, state.transparency_policies.get(index))
            })
            .collect::<Vec<_>>();
        let candidates = production_candidates(self);
        let routes = production_routes(self);
        if state.destination_mode == UnifiedDestinationMode::AppendExisting
            && state.selected_existing.is_none()
        {
            if let (Some(folder), Ok(recipe)) = (
                state.output_folder.as_deref(),
                build_conversion_recipe(&state.target, &current_inspection, current_policy),
            ) {
                let matching = routes
                    .iter()
                    .filter(|route| {
                        route.matches_recipe_policy(&recipe).unwrap_or(false)
                            && route
                                .output_folder()
                                .to_string_lossy()
                                .eq_ignore_ascii_case(&folder.to_string_lossy())
                    })
                    .take(2)
                    .collect::<Vec<_>>();
                if let [route] = matching.as_slice() {
                    state.selected_existing = Some(route.production_project_path());
                }
            }
        }
        let plan_preview = state.output_folder.as_deref().map(|folder| {
            build_unified_plan(
                self,
                state.scope,
                &inspections,
                &state.transparency_policies,
                &state.target,
                folder,
                state.destination_mode,
                state.selected_existing.as_deref(),
                &candidates,
                &routes,
                state.allow_production_work_discard,
            )
        });

        let mut open = true;
        let mut choose_target_file = false;
        let mut choose_output_folder = false;
        let mut refresh_profile_catalog = false;
        let mut selected_installed_profile: Option<PathBuf> = None;
        let mut assign_source_profile: Option<usize> = None;
        let mut clear_source_profile: Option<usize> = None;
        let mut force_candidate_refresh = false;
        let mut requested_candidate_visibility: Option<bool> = None;
        let mut restore_route_requested: Option<PathBuf> = None;
        let mut queue_requested = false;

        egui::Window::new("Production Color Conversion")
            .id(egui::Id::new("production-color-conversion-window"))
            .open(&mut open)
            .resizable(true)
            .default_size([920.0, 760.0])
            .min_width(680.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("production-color-conversion-scroll")
                    .auto_shrink([false, false])
                    .max_height(700.0)
                    .show(ui, |ui| {
                        ui.heading("Production Color Conversion");
                        ui.label(
                            "One target recipe drives Candidate Preview and Current / Selected / All Face conversion.",
                        );
                        ui.small(
                            "Preview is non-destructive. TIFF/Production output is created only after the final Queue action.",
                        );

                        ui.add_space(8.0);
                        egui::CollapsingHeader::new("1. Source & preflight")
                            .id_salt("unified-conversion-source")
                            .default_open(true)
                            .show(ui, |ui| {
                                render_face_summary(ui, &current_inspection);
                                render_face_findings(ui, &current_inspection);
                                ui.horizontal_wrapped(|ui| {
                                    if ui.button("Assign Source ICC...").clicked() {
                                        assign_source_profile = Some(self.current_face);
                                    }
                                    if self
                                        .project
                                        .faces
                                        .get(self.current_face)
                                        .and_then(|face| face.production_source_profile.as_ref())
                                        .is_some()
                                        && ui.button("Use embedded / RGB fallback").clicked()
                                    {
                                        clear_source_profile = Some(self.current_face);
                                    }
                                    if self.project_dirty {
                                        ui.label(
                                            egui::RichText::new(
                                                "Save Source changes before final conversion. Candidate Preview can still inspect the current adjustment state.",
                                            )
                                            .color(egui::Color32::YELLOW),
                                        );
                                    }
                                });
                            });

                        ui.add_space(6.0);
                        egui::CollapsingHeader::new("2. Production target")
                            .id_salt("unified-conversion-target")
                            .default_open(true)
                            .show(ui, |ui| {
                                let previous_engine = state.target.engine_mode;
                                egui::ComboBox::from_label("Conversion engine")
                                    .selected_text(engine_label(state.target.engine_mode))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut state.target.engine_mode,
                                            ConversionEngineMode::Icc,
                                            "Standard Output ICC",
                                        );
                                        ui.selectable_value(
                                            &mut state.target.engine_mode,
                                            ConversionEngineMode::DeviceLink,
                                            "DeviceLink",
                                        );
                                    });
                                if previous_engine != state.target.engine_mode {
                                    state.target.clear_profile();
                                    state.clear_profile_catalog();
                                    state.selected_existing = None;
                                }

                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .button(match state.target.engine_mode {
                                            ConversionEngineMode::Icc => "Choose Output ICC file...",
                                            ConversionEngineMode::DeviceLink => "Choose DeviceLink file...",
                                            ConversionEngineMode::CustomOptimizer => "Choose target...",
                                        })
                                        .clicked()
                                    {
                                        choose_target_file = true;
                                    }
                                    if let Some(profile) = state.target.target_profile.as_ref() {
                                        ui.label(format!(
                                            "{} · {} · {} channels",
                                            profile.identity.description,
                                            profile.output_space_label,
                                            profile.output_channel_count
                                        ));
                                    } else {
                                        ui.label("No production target selected");
                                    }
                                });

                                egui::CollapsingHeader::new("Installed production profiles")
                                    .id_salt("unified-installed-production-profiles")
                                    .default_open(false)
                                    .show(ui, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label("Filter");
                                            ui.text_edit_singleline(&mut state.target_profile_query);
                                            ui.checkbox(
                                                &mut state.show_incompatible_profiles,
                                                "Show incompatible",
                                            );
                                            if ui.small_button("Refresh").clicked() {
                                                refresh_profile_catalog = true;
                                            }
                                        });
                                        if let Some(error) = state.installed_profiles_error.as_deref() {
                                            ui.label(
                                                egui::RichText::new(error)
                                                    .color(egui::Color32::LIGHT_RED),
                                            );
                                        }
                                        let query = state.target_profile_query.trim().to_lowercase();
                                        egui::ScrollArea::vertical()
                                            .id_salt("unified-production-profile-list")
                                            .max_height(190.0)
                                            .show(ui, |ui| {
                                                for candidate in &state.installed_target_profiles {
                                                    if !query.is_empty()
                                                        && !candidate.profile.matches_query(&query)
                                                    {
                                                        continue;
                                                    }
                                                    if !state.show_incompatible_profiles
                                                        && !candidate.selectable()
                                                    {
                                                        continue;
                                                    }
                                                    let selected = state
                                                        .target
                                                        .target_profile
                                                        .as_ref()
                                                        .is_some_and(|profile| {
                                                            profile.identity == candidate.profile.identity
                                                        });
                                                    let label = format!(
                                                        "{} · {}",
                                                        candidate.profile.description,
                                                        candidate.profile.filename()
                                                    );
                                                    let response = ui.add_enabled(
                                                        candidate.selectable(),
                                                        egui::Button::selectable(selected, label),
                                                    );
                                                    let response = if let Some(rejection) = candidate.rejection {
                                                        response.on_hover_text(rejection.label())
                                                    } else {
                                                        response.on_hover_text(
                                                            candidate.profile.path.display().to_string(),
                                                        )
                                                    };
                                                    if response.clicked() && candidate.selectable() {
                                                        selected_installed_profile =
                                                            Some(candidate.profile.path.clone());
                                                    }
                                                }
                                            });
                                    });

                                if let Some(profile) = state.target.target_profile.as_ref() {
                                    ui.add_space(5.0);
                                    egui::Grid::new("unified-target-summary")
                                        .num_columns(4)
                                        .striped(true)
                                        .spacing([12.0, 5.0])
                                        .show(ui, |ui| {
                                            ui.strong("Target");
                                            ui.label(&state.target.target_name);
                                            ui.strong("Bit depth");
                                            ui.horizontal(|ui| {
                                                ui.selectable_value(
                                                    &mut state.target.output_bit_depth,
                                                    8,
                                                    "8-bit",
                                                );
                                                ui.selectable_value(
                                                    &mut state.target.output_bit_depth,
                                                    16,
                                                    "16-bit",
                                                );
                                            });
                                            ui.end_row();
                                            ui.strong("Profile SHA");
                                            ui.label(short_hash(&profile.identity.sha256));
                                            ui.strong("Topology");
                                            ui.label(format!(
                                                "{} / {} inks",
                                                profile.output_space_label,
                                                profile.output_channel_count
                                            ));
                                            ui.end_row();
                                        });

                                    if state.target.engine_mode == ConversionEngineMode::Icc {
                                        egui::ComboBox::from_label("Rendering intent")
                                            .selected_text(intent_label(state.target.rendering_intent))
                                            .show_ui(ui, |ui| {
                                                for intent in [
                                                    ConversionRenderingIntent::Perceptual,
                                                    ConversionRenderingIntent::RelativeColorimetric,
                                                    ConversionRenderingIntent::Saturation,
                                                    ConversionRenderingIntent::AbsoluteColorimetric,
                                                ] {
                                                    ui.selectable_value(
                                                        &mut state.target.rendering_intent,
                                                        intent,
                                                        intent_label(intent),
                                                    );
                                                }
                                            });
                                        ui.checkbox(
                                            &mut state.target.black_point_compensation,
                                            "Black Point Compensation",
                                        );
                                    }

                                    ui.strong("Output channel order");
                                    if profile.channel_names_authoritative {
                                        ui.small(state.target.channel_names.join(" / "));
                                        state.target.channel_names_confirmed = true;
                                    } else {
                                        ui.label(
                                            egui::RichText::new(
                                                "Profile does not expose authoritative colorant names. Confirm the real RIP/press order.",
                                            )
                                            .color(egui::Color32::YELLOW),
                                        );
                                        for (index, name) in
                                            state.target.channel_names.iter_mut().enumerate()
                                        {
                                            ui.horizontal(|ui| {
                                                ui.label(format!("Ink {}", index + 1));
                                                ui.text_edit_singleline(name);
                                            });
                                        }
                                        ui.checkbox(
                                            &mut state.target.channel_names_confirmed,
                                            "I confirm this production channel order",
                                        );
                                    }
                                }
                            });

                        ui.add_space(6.0);
                        egui::CollapsingHeader::new("3. Candidate Preview")
                            .id_salt("unified-conversion-candidate")
                            .default_open(true)
                            .show(ui, |ui| {
                                let status = self.conversion_candidate_status();
                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .selectable_label(!status.show_converted, "Source")
                                        .clicked()
                                    {
                                        requested_candidate_visibility = Some(false);
                                    }
                                    if ui
                                        .selectable_label(status.show_converted, "Converted Candidate")
                                        .clicked()
                                    {
                                        requested_candidate_visibility = Some(true);
                                    }
                                    if ui.button("Refresh candidate now").clicked() {
                                        force_candidate_refresh = true;
                                    }
                                    if status.pending {
                                        ui.spinner();
                                        ui.label("Rendering converted candidate...");
                                    } else if status.active {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Ready · {} target inks",
                                                status.channel_count
                                            ))
                                            .color(egui::Color32::LIGHT_GREEN),
                                        );
                                    }
                                });
                                if let Some(hash) = status.recipe_sha256.as_deref() {
                                    ui.small(format!("Candidate recipe SHA-256: {hash}"));
                                }
                                if let Some(error) = status.error.as_deref() {
                                    ui.label(
                                        egui::RichText::new(error)
                                            .color(egui::Color32::LIGHT_RED),
                                    );
                                }
                                if state.target.target_profile.is_some() {
                                    ui.small(
                                        "A valid target automatically renders on the main viewport. The Channels/Histogram panel switches to converted target samples while Converted Candidate is selected.",
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new(
                                            "Choose a production target to generate Candidate Preview.",
                                        )
                                        .color(egui::Color32::YELLOW),
                                    );
                                }
                            });

                        ui.add_space(6.0);
                        egui::CollapsingHeader::new("4. Production destination")
                            .id_salt("unified-conversion-destination")
                            .default_open(true)
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label("Destination folder");
                                    ui.strong(
                                        state
                                            .output_folder
                                            .as_deref()
                                            .map(|path| path.display().to_string())
                                            .unwrap_or_else(|| "Not selected".to_owned()),
                                    );
                                    if ui.button("Choose folder...").clicked() {
                                        choose_output_folder = true;
                                    }
                                });
                                ui.small(
                                    "Per-Face TIFF names are canonical and not editable. The same Source Face maps to the same filename whether converted alone, Selected, or All Faces.",
                                );
                                ui.horizontal_wrapped(|ui| {
                                    ui.selectable_value(
                                        &mut state.destination_mode,
                                        UnifiedDestinationMode::CreateNew,
                                        "Create New Production project",
                                    );
                                    ui.selectable_value(
                                        &mut state.destination_mode,
                                        UnifiedDestinationMode::AppendExisting,
                                        "Append Existing linked project",
                                    );
                                });
                                if state.destination_mode == UnifiedDestinationMode::AppendExisting {
                                    if let Some(selected_path) = state.selected_existing.as_deref() {
                                        if let Some(route) = routes.iter().find(|route| {
                                            route.production_project_path()
                                                .to_string_lossy()
                                                .eq_ignore_ascii_case(&selected_path.to_string_lossy())
                                        }) {
                                            let missing_outputs = route
                                                .faces
                                                .iter()
                                                .filter(|face| !PathBuf::from(&face.provenance.output_path).exists())
                                                .count();
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Saved route · {} committed Face(s) · {} missing output(s) · policy {}",
                                                    route.converted_face_count(),
                                                    missing_outputs,
                                                    short_hash(&route.batch_recipe_policy_sha256)
                                                ))
                                                .color(if missing_outputs == 0 {
                                                    egui::Color32::LIGHT_GREEN
                                                } else {
                                                    egui::Color32::YELLOW
                                                }),
                                            );
                                            if ui.button("Restore saved route settings").clicked() {
                                                restore_route_requested = Some(route.production_project_path());
                                            }
                                            ui.checkbox(
                                                &mut state.allow_production_work_discard,
                                                "Allow same-route replacement when Production-side adjustments/Snapshots require explicit discard confirmation",
                                            );
                                        }
                                    }
                                    for candidate in &candidates {
                                        let selected = state.selected_existing.as_deref()
                                            == Some(candidate.path.as_path());
                                        let status = destination_status_label(candidate.availability);
                                        let response = ui.add_enabled(
                                            candidate.can_append(),
                                            egui::Button::selectable(
                                                selected,
                                                format!(
                                                    "{} · {}",
                                                    candidate
                                                        .project_name
                                                        .as_deref()
                                                        .unwrap_or("Production"),
                                                    status
                                                ),
                                            ),
                                        );
                                        if response.clicked() && candidate.can_append() {
                                            if state.selected_existing.as_deref()
                                                != Some(candidate.path.as_path())
                                            {
                                                state.allow_production_work_discard = false;
                                            }
                                            state.selected_existing = Some(candidate.path.clone());
                                        }
                                        ui.small(candidate.path.display().to_string());
                                        if let Some(diagnostic) = candidate.diagnostic.as_deref() {
                                            ui.label(
                                                egui::RichText::new(diagnostic)
                                                    .color(egui::Color32::YELLOW),
                                            );
                                        }
                                    }
                                }
                            });

                        ui.add_space(6.0);
                        egui::CollapsingHeader::new("5. Convert Faces")
                            .id_salt("unified-conversion-scope")
                            .default_open(true)
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.selectable_value(
                                        &mut state.scope,
                                        ConversionBatchScope::CurrentFace,
                                        "Current Face",
                                    );
                                    ui.selectable_value(
                                        &mut state.scope,
                                        ConversionBatchScope::SelectedFaces,
                                        "Selected Faces",
                                    );
                                    ui.selectable_value(
                                        &mut state.scope,
                                        ConversionBatchScope::AllFaces,
                                        "All Faces",
                                    );
                                });
                                if state.scope == ConversionBatchScope::SelectedFaces {
                                    ui.group(|ui| {
                                        ui.strong("Selected Source Faces");
                                        for index in 0..self.project.faces.len() {
                                            let label = self
                                                .project
                                                .faces
                                                .get(index)
                                                .map(|face| face.label.clone())
                                                .filter(|label| !label.trim().is_empty())
                                                .unwrap_or_else(|| format!("Face {}", index + 1));
                                            let mut selected = state.selected_faces.contains(&index);
                                            if ui
                                                .checkbox(
                                                    &mut selected,
                                                    format!("{} — {}", index + 1, label),
                                                )
                                                .changed()
                                            {
                                                if selected {
                                                    state.selected_faces.insert(index);
                                                } else {
                                                    state.selected_faces.remove(&index);
                                                }
                                            }
                                        }
                                    });
                                }

                                for inspection in &inspections {
                                    render_scope_face(
                                        ui,
                                        inspection,
                                        &mut state,
                                        &mut assign_source_profile,
                                    );
                                }
                                render_profile_consistency_warning(ui, &inspections);

                                match plan_preview.as_ref() {
                                    Some(Ok(plan)) => {
                                        ui.add_space(5.0);
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Ready: {} Face(s) → {}",
                                                inspections.len(),
                                                plan.production_project_path.display()
                                            ))
                                            .color(egui::Color32::LIGHT_GREEN)
                                            .strong(),
                                        );
                                        for (inspection, output) in
                                            inspections.iter().zip(&plan.output_paths)
                                        {
                                            ui.small(format!(
                                                "Face {} · {} → {}",
                                                inspection.index + 1,
                                                inspection.label,
                                                output.display()
                                            ));
                                        }
                                        if ui
                                            .add_enabled(
                                                self.job.is_none(),
                                                egui::Button::new(format!(
                                                    "Queue Production Conversion — {} Face(s)",
                                                    inspections.len()
                                                )),
                                            )
                                            .clicked()
                                        {
                                            queue_requested = true;
                                        }
                                    }
                                    Some(Err(errors)) => {
                                        for error in errors {
                                            ui.label(
                                                egui::RichText::new(format!("• {error}"))
                                                    .color(egui::Color32::LIGHT_RED),
                                            );
                                        }
                                        ui.add_enabled(
                                            false,
                                            egui::Button::new("Queue Production Conversion"),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            egui::RichText::new(
                                                "Choose a Production destination folder.",
                                            )
                                            .color(egui::Color32::YELLOW),
                                        );
                                    }
                                }
                            });

                        ui.add_space(6.0);
                        egui::CollapsingHeader::new(format!(
                            "6. Conversion Queue ({})",
                            self.conversion_batch_pending_count()
                        ))
                        .id_salt("unified-conversion-queue")
                        .default_open(false)
                        .show(ui, |ui| self.ui_unified_conversion_queue(ui));
                    });
            });

        if let Some(route_path) = restore_route_requested {
            if let Some(route) = routes.iter().find(|route| {
                route
                    .production_project_path()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&route_path.to_string_lossy())
            }) {
                match restore_target_from_route(route, current_inspection.source_model) {
                    Ok(target) => match self.restore_source_bindings_from_route(route, &mut state) {
                        Ok(source_changed) => {
                            state.target = target;
                            state.output_folder = Some(route.output_folder());
                            state.destination_mode = UnifiedDestinationMode::AppendExisting;
                            state.selected_existing = Some(route.production_project_path());
                            state.allow_production_work_discard = false;
                            state.clear_profile_catalog();
                            force_candidate_refresh = true;
                            if source_changed {
                                self.report_info(
                                    "Restored saved conversion route. Source ICC bindings changed; Save the Source project before final conversion."
                                );
                            } else {
                                self.report_info("Restored and reverified saved conversion route settings.");
                            }
                        }
                        Err(error) => self.report_error(error),
                    },
                    Err(error) => self.report_error(format!(
                        "Saved conversion route requires repair before restore: {error}"
                    )),
                }
            }
        }

        if refresh_profile_catalog {
            state.clear_profile_catalog();
            ensure_installed_profile_catalog(&mut state, &current_inspection);
        }

        if choose_target_file || selected_installed_profile.is_some() {
            let path = if let Some(path) = selected_installed_profile {
                Some(path)
            } else {
                rfd::FileDialog::new()
                    .add_filter("ICC / DeviceLink profiles", &["icc", "icm"])
                    .set_title(match state.target.engine_mode {
                        ConversionEngineMode::Icc => "Select Production Output ICC",
                        ConversionEngineMode::DeviceLink => "Select Production DeviceLink",
                        ConversionEngineMode::CustomOptimizer => "Select production target",
                    })
                    .pick_file()
            };
            if let Some(path) = path {
                match inspect_production_target_profile(
                    &path,
                    state.target.engine_mode,
                    conversion_color_model(current_inspection.source_model),
                ) {
                    Ok(profile) => {
                        let description = profile.identity.description.clone();
                        let channels = profile.output_channel_count;
                        state.target.accept_profile(profile);
                        state.selected_existing = None;
                        self.report_info(format!(
                            "Selected production target '{description}' ({channels} channels); Candidate Preview scheduled."
                        ));
                    }
                    Err(error) => self.report_error(error),
                }
            }
        }

        if choose_output_folder {
            let mut dialog = rfd::FileDialog::new().set_title("Select Production Conversion Folder");
            if let Some(folder) = state.output_folder.as_deref() {
                dialog = dialog.set_directory(folder);
            }
            if let Some(folder) = dialog.pick_folder() {
                state.output_folder = Some(folder);
            }
        }

        if let Some(index) = assign_source_profile {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .set_title("Assign Production Source ICC")
                .pick_file()
            {
                self.assign_production_source_profile(index, path);
            }
        }
        if let Some(index) = clear_source_profile {
            self.clear_production_source_profile(index);
        }

        if let Some(visible) = requested_candidate_visibility {
            self.set_conversion_candidate_visible(visible, ctx);
        }

        self.color_conversion = state.clone();
        if !open {
            set_conversion_window_open(ctx, false);
            self.clear_conversion_candidate();
            return;
        }

        let current_policy = state.transparency_policies.get(&self.current_face);
        let current_inspection = inspect_conversion_face(self, self.current_face, current_policy);
        match build_conversion_recipe(
            &state.target,
            &current_inspection,
            current_policy.copied(),
        ) {
            Ok(recipe) => self.sync_conversion_candidate(
                &current_inspection,
                &recipe,
                force_candidate_refresh,
                ctx,
            ),
            Err(_) => self.clear_conversion_candidate(),
        }

        if queue_requested {
            let indices = scope_indices(
                state.scope,
                self.current_face,
                self.project.faces.len(),
                &state.selected_faces,
            );
            let inspections = indices
                .iter()
                .map(|index| {
                    inspect_conversion_face(self, *index, state.transparency_policies.get(index))
                })
                .collect::<Vec<_>>();
            let candidates = production_candidates(self);
            let result = state
                .output_folder
                .as_deref()
                .ok_or_else(|| vec!["Choose a Production destination folder.".to_owned()])
                .and_then(|folder| {
                    build_unified_plan(
                        self,
                        state.scope,
                        &inspections,
                        &state.transparency_policies,
                        &state.target,
                        folder,
                        state.destination_mode,
                        state.selected_existing.as_deref(),
                        &candidates,
                        &routes,
                        state.allow_production_work_discard,
                    )
                });
            match result {
                Ok(plan) => match self.queue_unified_conversion_plan(state.scope, &inspections, plan) {
                    Ok(id) => self.report_info(format!(
                        "Queued Production Color Conversion #{id} with {} Face(s)",
                        inspections.len()
                    )),
                    Err(error) => self.report_error(error),
                },
                Err(errors) => self.report_error(errors.join(" ")),
            }
        }
    }

    fn restore_source_bindings_from_route(
        &mut self,
        route: &windows_shade_editor::model::ConversionRouteRecord,
        state: &mut ColorConversionUiState,
    ) -> Result<bool, String> {
        let mut desired = Vec::new();
        for (index, runtime) in self.faces.iter().enumerate() {
            let Some(route_face) = route.face_for_source(&runtime.path) else {
                continue;
            };
            let source_model = runtime.preview.color_model();
            let desired_assignment = if let Some(recorded_path) = route_face.source_profile_path.as_deref() {
                let path = PathBuf::from(recorded_path);
                let verified = IccProfileRegistry.verify_identity(
                    &path,
                    &route_face.provenance.recipe.source_profile_identity,
                )?;
                if !verified.compatible_with_source_model(conversion_color_model(source_model)) {
                    return Err(format!(
                        "Saved Source ICC '{}' no longer matches {} source data for Face {}.",
                        verified.description,
                        source_model.title(),
                        index + 1
                    ));
                }
                Some(model::ProductionSourceProfileAssignment {
                    path: path.to_string_lossy().into_owned(),
                    identity: model::IccProfileIdentity {
                        description: verified.identity.description,
                        sha256: verified.identity.sha256,
                    },
                })
            } else {
                let descriptor = runtime.preview.source_descriptor().ok_or_else(|| {
                    format!("Cannot inspect Source ICC state for Face {}.", index + 1)
                })?;
                let actual = color_management::production_source_profile_identity_or_rgb_fallback_for_runtime(
                    source_model,
                    descriptor.embedded_icc.as_deref(),
                )?
                .ok_or_else(|| format!("Saved route requires a Source ICC for Face {}.", index + 1))?;
                let expected = &route_face.provenance.recipe.source_profile_identity;
                if !actual.sha256.eq_ignore_ascii_case(expected.sha256.trim()) {
                    return Err(format!(
                        "Embedded/fallback Source ICC for Face {} no longer matches the saved route. Relink the original Source ICC before reconversion.",
                        index + 1
                    ));
                }
                None
            };
            desired.push((
                index,
                desired_assignment,
                route_face.provenance.recipe.source_transparency_policy,
            ));
        }

        let mut changed = false;
        for (index, assignment, transparency) in desired {
            if let Some(face) = self.project.faces.get_mut(index) {
                if face.production_source_profile != assignment {
                    face.production_source_profile = assignment;
                    changed = true;
                }
            }
            match transparency {
                Some(policy) => {
                    state.transparency_policies.insert(index, policy);
                }
                None => {
                    state.transparency_policies.remove(&index);
                }
            }
        }
        if changed {
            self.mark_project_dirty();
        }
        Ok(changed)
    }

    fn assign_production_source_profile(&mut self, index: usize, path: PathBuf) {
        let Some(active_face) = self.faces.get(index) else {
            return;
        };
        let source_model = active_face.preview.color_model();
        let face_label = self
            .project
            .faces
            .get(index)
            .map(|face| face.label.clone())
            .unwrap_or_else(|| format!("Face {}", index + 1));
        match IccProfileRegistry.inspect(&path) {
            Ok(profile)
                if profile.compatible_with_source_model(conversion_color_model(source_model)) =>
            {
                let assignment = model::ProductionSourceProfileAssignment {
                    path: path.to_string_lossy().into_owned(),
                    identity: model::IccProfileIdentity {
                        description: profile.identity.description.clone(),
                        sha256: profile.identity.sha256.clone(),
                    },
                };
                let Some(face) = self.project.faces.get_mut(index) else {
                    self.report_error("Cannot bind Source ICC: Face metadata is missing.");
                    return;
                };
                if face.production_source_profile.as_ref() != Some(&assignment) {
                    face.production_source_profile = Some(assignment);
                    self.mark_project_dirty();
                    self.report_info(format!(
                        "Assigned production Source ICC '{}' to {face_label}. Save before final conversion.",
                        profile.description
                    ));
                }
            }
            Ok(profile) => self.report_error(format!(
                "Cannot assign '{}' to {}: profile color space {} does not match source {}.",
                profile.description,
                face_label,
                profile.color_space_label(),
                source_model.title(),
            )),
            Err(error) => self.report_error(error),
        }
    }

    fn clear_production_source_profile(&mut self, index: usize) {
        let Some(face) = self.project.faces.get_mut(index) else {
            return;
        };
        if face.production_source_profile.take().is_some() {
            let label = face.label.clone();
            self.mark_project_dirty();
            self.report_info(format!(
                "Cleared production Source ICC override for {label}; embedded ICC / RGB fallback is active."
            ));
        }
    }
}

fn ensure_installed_profile_catalog(
    state: &mut ColorConversionUiState,
    inspection: &ConversionFaceInspection,
) {
    if !matches!(inspection.source_model, RuntimeColorModel::Rgb | RuntimeColorModel::Cmyk) {
        return;
    }
    let source_model = conversion_color_model(inspection.source_model);
    let key = format!("{:?}|{:?}", state.target.engine_mode, source_model);
    if state.installed_catalog_key == key {
        return;
    }
    match installed_production_profiles(
        IccProfileRegistry,
        state.target.engine_mode,
        source_model,
        "",
        true,
    ) {
        Ok(profiles) => {
            state.installed_target_profiles = profiles;
            state.installed_profiles_error = None;
        }
        Err(error) => {
            state.installed_target_profiles.clear();
            state.installed_profiles_error = Some(error);
        }
    }
    state.installed_catalog_key = key;
}

fn render_face_summary(ui: &mut egui::Ui, inspection: &ConversionFaceInspection) {
    egui::Grid::new("unified-current-source-summary")
        .num_columns(4)
        .striped(true)
        .spacing([12.0, 5.0])
        .show(ui, |ui| {
            ui.strong("Face");
            ui.label(&inspection.label);
            ui.strong("Format");
            ui.label(inspection.source_format.label());
            ui.end_row();
            ui.strong("Color model");
            ui.label(inspection.source_model.title());
            ui.strong("Bit depth");
            ui.label(format!("{}-bit", inspection.bit_depth));
            ui.end_row();
            ui.strong("Channels");
            ui.label(inspection.channel_count.to_string());
            ui.strong("Source ICC");
            ui.label(&inspection.profile_label);
            ui.end_row();
        });
    ui.small(inspection.source_path.display().to_string());
}

fn render_face_findings(ui: &mut egui::Ui, inspection: &ConversionFaceInspection) {
    if let Some(error) = inspection.error.as_deref() {
        ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
    }
    if !inspection.execution_supported {
        ui.label(
            egui::RichText::new(
                "Production execution supports RGB TIFF/PNG/JPEG and CMYK TIFF Sources.",
            )
            .color(egui::Color32::LIGHT_RED),
        );
    }
    for finding in &inspection.report.findings {
        ui.small(
            egui::RichText::new(format!(
                "{}: {} — {}",
                severity_label(finding.severity),
                finding.title,
                finding.detail
            ))
            .color(severity_color(finding.severity)),
        );
    }
}

fn render_scope_face(
    ui: &mut egui::Ui,
    inspection: &ConversionFaceInspection,
    state: &mut ColorConversionUiState,
    assign_source_profile: &mut Option<usize>,
) {
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("Face {} — {}", inspection.index + 1, inspection.label));
            ui.label(format!(
                "{} · {} · {}-bit",
                inspection.source_format.label(),
                inspection.source_model.title(),
                inspection.bit_depth
            ));
            ui.label(if inspection.ready() {
                egui::RichText::new("Ready").color(egui::Color32::LIGHT_GREEN)
            } else {
                egui::RichText::new("Blocked").color(egui::Color32::LIGHT_RED)
            });
        });
        ui.small(format!("Source ICC: {}", inspection.profile_label));
        if inspection.transparency
            == windows_shade_editor::design_source::TransparencyState::PresentUnresolved
        {
            let mut flatten = state.transparency_policies.contains_key(&inspection.index);
            if ui
                .checkbox(
                    &mut flatten,
                    "Flatten this Face on solid white for Production conversion",
                )
                .changed()
            {
                if flatten {
                    state.transparency_policies.insert(
                        inspection.index,
                        SourceTransparencyPolicy::FlattenSolidRgb16 {
                            background_rgb: [u16::MAX; 3],
                        },
                    );
                } else {
                    state.transparency_policies.remove(&inspection.index);
                }
            }
        }
        for finding in &inspection.report.findings {
            if finding.severity != PreflightSeverity::Info {
                ui.small(
                    egui::RichText::new(format!(
                        "{}: {}",
                        severity_label(finding.severity),
                        finding.title
                    ))
                    .color(severity_color(finding.severity)),
                );
            }
        }
        if inspection.report.contains(PreflightCode::MissingSourceProfile)
            && ui.small_button("Assign Source ICC...").clicked()
        {
            *assign_source_profile = Some(inspection.index);
        }
    });
}

fn render_profile_consistency_warning(ui: &mut egui::Ui, inspections: &[ConversionFaceInspection]) {
    let mut groups = BTreeMap::<String, (String, Vec<String>)>::new();
    for inspection in inspections {
        let Some(identity) = inspection.profile_identity.as_ref() else {
            continue;
        };
        groups
            .entry(identity.sha256.trim().to_ascii_lowercase())
            .or_insert_with(|| (identity.description.clone(), Vec::new()))
            .1
            .push(format!("Face {}", inspection.index + 1));
    }
    if groups.len() <= 1 {
        return;
    }
    ui.group(|ui| {
        ui.label(
            egui::RichText::new("Warning: selected Faces use different Source ICC interpretations")
                .color(egui::Color32::YELLOW)
                .strong(),
        );
        ui.small(
            "Conversion is allowed. Each Face keeps its own captured Source ICC / RGB fallback; the shared Production target is not changed.",
        );
        for (_hash, (description, faces)) in groups {
            ui.small(format!("{description}: {}", faces.join(", ")));
        }
    });
}

fn engine_label(mode: ConversionEngineMode) -> &'static str {
    match mode {
        ConversionEngineMode::Icc => "Standard Output ICC",
        ConversionEngineMode::DeviceLink => "DeviceLink",
        ConversionEngineMode::CustomOptimizer => "Custom Optimizer",
    }
}

fn intent_label(intent: ConversionRenderingIntent) -> &'static str {
    match intent {
        ConversionRenderingIntent::Perceptual => "Perceptual",
        ConversionRenderingIntent::RelativeColorimetric => "Relative colorimetric",
        ConversionRenderingIntent::Saturation => "Saturation",
        ConversionRenderingIntent::AbsoluteColorimetric => "Absolute colorimetric",
    }
}

fn destination_status_label(status: ProductionDestinationAvailability) -> &'static str {
    match status {
        ProductionDestinationAvailability::Ready => "Ready",
        ProductionDestinationAvailability::Missing => "Missing",
        ProductionDestinationAvailability::Unreadable => "Unreadable",
        ProductionDestinationAvailability::Incompatible => "Incompatible",
    }
}

fn severity_label(severity: PreflightSeverity) -> &'static str {
    match severity {
        PreflightSeverity::Info => "INFO",
        PreflightSeverity::Warning => "WARNING",
        PreflightSeverity::Blocking => "BLOCKING",
    }
}

fn severity_color(severity: PreflightSeverity) -> egui::Color32 {
    match severity {
        PreflightSeverity::Info => egui::Color32::LIGHT_BLUE,
        PreflightSeverity::Warning => egui::Color32::YELLOW,
        PreflightSeverity::Blocking => egui::Color32::LIGHT_RED,
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn conversion_window_open(ctx: &egui::Context) -> bool {
    ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new(CONVERSION_WINDOW_ID))
            .unwrap_or(false)
    })
}

fn set_conversion_window_open(ctx: &egui::Context, open: bool) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(CONVERSION_WINDOW_ID), open));
}

#[cfg(test)]
mod tests {
    #[test]
    fn unified_window_owns_scope_target_destination_and_preview_controls() {
        let source = include_str!("color_conversion.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        for required in [
            "Production Color Conversion",
            "Current Face",
            "Selected Faces",
            "All Faces",
            "Candidate Preview",
            "Destination folder",
            "Queue Production Conversion",
            "sync_conversion_candidate",
            "queue_unified_conversion_plan",
        ] {
            assert!(runtime.contains(required), "missing unified conversion token: {required}");
        }
        assert!(!runtime.contains("Choose output TIFF"));
        assert!(!runtime.contains("next_versioned_output_path"));
    }

    #[test]
    fn operator_state_contains_one_shared_target() {
        let source = include_str!("color_conversion.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("target: ConversionTargetState"));
        assert!(!runtime.contains("CandidateConfig"));
        assert!(!runtime.contains("ConversionBatchUiConfig"));
    }
}
