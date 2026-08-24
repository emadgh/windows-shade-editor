use super::actions::NavigationUiAction;
use crate::*;
use eframe::egui;
use windows_shade_editor::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use windows_shade_editor::conversion_capabilities::{ControlAvailability, capabilities_for_engine};
use windows_shade_editor::conversion_output::{
    OutputCollisionPolicy, OutputPathError, default_converted_filename, next_versioned_output_path,
    validate_conversion_output_path,
};
use windows_shade_editor::conversion_preflight::{
    ConversionPreflightReport, PreflightCode, PreflightSeverity, SourceProfileState,
    build_conversion_preflight_for_source_with_policy,
};
use windows_shade_editor::conversion_recipe::recipe_sha256;
use windows_shade_editor::conversion_transaction::{
    CapturedOutputPolicy, CapturedSourceProfile, ConversionJobCapture,
};
use windows_shade_editor::conversion_workflow::{
    ConversionSaveGate, ConversionSourceState, conversion_save_gate,
};
use windows_shade_editor::design_source::{
    DesignSourceColorModel, SourceImageFormat, SourceLossiness, TransparencyState,
};
use windows_shade_editor::icc_profile_registry::IccProfileRegistry;
use windows_shade_editor::model::IccProfileIdentity as ConversionIccProfileIdentity;
use windows_shade_editor::production_destination::{
    ProductionDestinationCandidate, inspect_linked_production_destinations,
};
use windows_shade_editor::production_destination_selection::FrozenProductionDestination;
use windows_shade_editor::production_profile_catalog::{
    ProductionProfileCandidate, inspect_production_profile_candidate,
    installed_production_profiles, verify_production_profile_candidate,
};
use windows_shade_editor::production_project_disposition::ProductionProjectDisposition;
use windows_shade_editor::production_target::{
    ProductionTargetProfileInspection, validate_target_channel_names,
    verify_production_target_profile,
};
use windows_shade_editor::source_transparency::SourceTransparencyPolicy;
use windows_shade_editor::tiff_io::ColorModel as ConversionColorModel;

const CONVERSION_WINDOW_ID: &str = "shade-editor-color-conversion-preflight-open";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ConversionStage {
    #[default]
    SourcePreflight,
    TargetSetup,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProductionDestinationMode {
    #[default]
    CreateNew,
    AppendExisting,
}

pub(crate) struct ColorConversionUiState {
    source_key: Option<PathBuf>,
    stage: ConversionStage,
    engine_mode: ConversionEngineMode,
    target_profile: Option<ProductionTargetProfileInspection>,
    installed_target_profiles: Vec<ProductionProfileCandidate>,
    installed_profiles_loaded: bool,
    installed_profiles_error: Option<String>,
    target_profile_query: String,
    show_incompatible_profiles: bool,
    target_name: String,
    channel_names: Vec<String>,
    channel_names_confirmed: bool,
    output_bit_depth: u8,
    output_path: Option<PathBuf>,
    collision_policy: OutputCollisionPolicy,
    destination_mode: ProductionDestinationMode,
    selected_existing: Option<ProductionDestinationCandidate>,
    destination_error: Option<String>,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
    source_transparency_policy: Option<SourceTransparencyPolicy>,
}

impl Default for ColorConversionUiState {
    fn default() -> Self {
        Self {
            source_key: None,
            stage: ConversionStage::SourcePreflight,
            engine_mode: ConversionEngineMode::Icc,
            target_profile: None,
            installed_target_profiles: Vec::new(),
            installed_profiles_loaded: false,
            installed_profiles_error: None,
            target_profile_query: String::new(),
            show_incompatible_profiles: false,
            target_name: String::new(),
            channel_names: Vec::new(),
            channel_names_confirmed: false,
            output_bit_depth: 16,
            output_path: None,
            collision_policy: OutputCollisionPolicy::Versioned,
            destination_mode: ProductionDestinationMode::CreateNew,
            selected_existing: None,
            destination_error: None,
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            source_transparency_policy: None,
        }
    }
}

impl ColorConversionUiState {
    fn bind_source(&mut self, source_path: &Path) {
        if self.source_key.as_deref() == Some(source_path) {
            return;
        }
        *self = Self {
            source_key: Some(source_path.to_path_buf()),
            ..Self::default()
        };
    }

    fn clear_target_profile(&mut self) {
        self.target_profile = None;
        self.target_name.clear();
        self.channel_names.clear();
        self.channel_names_confirmed = false;
        self.output_path = None;
    }

    fn clear_installed_profile_catalog(&mut self) {
        self.installed_target_profiles.clear();
        self.installed_profiles_loaded = false;
        self.installed_profiles_error = None;
        self.target_profile_query.clear();
    }

    fn accept_target_profile(
        &mut self,
        profile: ProductionTargetProfileInspection,
        source_path: &Path,
    ) {
        self.target_name = profile.identity.description.clone();
        self.channel_names = profile.channel_names.clone();
        self.channel_names_confirmed = profile.channel_names_authoritative;
        self.output_path =
            recommended_output_path(source_path, &self.target_name, profile.output_channel_count)
                .ok();
        self.target_profile = Some(profile);
        self.installed_profiles_error = None;
    }
}

#[derive(Clone, Debug)]
struct CurrentConversionSource {
    face_label: String,
    source_path: PathBuf,
    source_format_label: &'static str,
    source_model: RuntimeColorModel,
    transparency: TransparencyState,
    lossiness: SourceLossiness,
    execution_supported: bool,
    color_model_label: &'static str,
    bit_depth: u8,
    channel_count: usize,
    profile_identity: Option<ConversionIccProfileIdentity>,
    profile_label: String,
    production_profile_path: Option<String>,
    has_assigned_profile: bool,
    snapshot_id: Option<u64>,
    save_gate: ConversionSaveGate,
    report: ConversionPreflightReport,
}

#[derive(Clone)]
struct TargetSetupReview {
    recipe: ConversionRecipe,
    recipe_sha256: String,
    effective_output_path: PathBuf,
    production_project_path: PathBuf,
    production_project_disposition: ProductionProjectDisposition,
}

#[derive(Clone)]
struct ConversionQueueRow {
    id: u64,
    label: String,
    status: windows_shade_editor::conversion_queue::ConversionQueueStatus,
    progress: f32,
    phase: String,
    detail: String,
    error: Option<String>,
    requires_resume: bool,
}

enum ConversionQueueUiAction {
    ResumeRecovered,
    TogglePaused,
    Cancel(u64),
    Retry(u64),
    RecoverProject(u64),
    ClearFinished,
}

impl ShadeApp {
    pub(crate) fn ui_color_conversion_status(&mut self, ui: &mut egui::Ui) {
        let Some(source) = self.current_conversion_source() else {
            return;
        };

        let is_rgb = source
            .report
            .contains(PreflightCode::RgbNotProductionSeparated);
        let supported_source = matches!(
            self.faces
                .get(self.current_face)
                .map(|face| face.preview.color_model()),
            Some(RuntimeColorModel::Rgb | RuntimeColorModel::Cmyk)
        );

        if is_rgb {
            ui.separator();
            ui.label(
                egui::RichText::new("RGB source — not production separated")
                    .color(egui::Color32::YELLOW)
                    .small(),
            )
            .on_hover_text(
                "Convert this Source project to the target CMYK/Multichannel printing space before production-separated output.",
            );
        }

        if supported_source
            && ui
                .small_button(app_features::COLOR_CONVERSION_LABEL)
                .on_hover_text(
                    "Inspect production color-conversion prerequisites. Source files remain unchanged.",
                )
                .clicked()
        {
            set_conversion_window_open(ui.ctx(), true);
        }
    }

    pub(crate) fn ui_color_conversion_window(&mut self, ctx: &egui::Context) {
        if !conversion_window_open(ctx) {
            return;
        }

        let Some(source) = self.current_conversion_source() else {
            set_conversion_window_open(ctx, false);
            return;
        };

        self.color_conversion.bind_source(&source.source_path);
        if !source.report.can_convert() {
            self.color_conversion.stage = ConversionStage::SourcePreflight;
        }

        let mut open = true;
        let mut navigation_action = None;
        let mut open_preview_color_management = false;
        let mut assign_production_profile = false;
        let mut clear_production_profile = false;
        let mut select_target_profile = false;
        let mut select_output_path = false;
        let mut start_conversion = false;
        let production_candidates = self
            .project_path
            .as_deref()
            .and_then(|source_project_path| {
                serde_json::to_value(&self.project)
                    .ok()
                    .and_then(|value| {
                        serde_json::from_value::<windows_shade_editor::model::ShadeProject>(value)
                            .ok()
                    })
                    .map(|source_project| {
                        inspect_linked_production_destinations(&source_project, source_project_path)
                    })
            })
            .unwrap_or_default();
        let queue_rows = self
            .conversion_queue
            .items()
            .iter()
            .map(|item| ConversionQueueRow {
                id: item.id,
                label: item.label.clone(),
                status: item.status,
                progress: item.progress,
                phase: item.phase.clone(),
                detail: item.detail.clone(),
                error: item.error.clone(),
                requires_resume: item.requires_resume,
            })
            .collect::<Vec<_>>();
        let queue_paused = self.conversion_queue.is_paused();
        let recovered_waiting = self.conversion_queue.recovered_waiting_count();
        let mut queue_actions = Vec::new();

        let state = &mut self.color_conversion;
        // Keep the content region bounded so the outer conversion UI always scrolls.
        // A fixed cap avoids depending on viewport APIs that vary across egui releases.
        let scroll_height = 620.0;
        egui::Window::new("Production Color Conversion")
            .id(egui::Id::new("production-color-conversion-window"))
            .open(&mut open)
            .resizable(true)
            .default_size([900.0, 680.0])
            .min_width(640.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("production-color-conversion-scroll")
                    .auto_shrink([false, false])
                    .max_height(scroll_height)
                    .show(ui, |ui| {
                ui.heading("Production Color Conversion");
                ui.label(
                    "Saved RGB/CMYK Source → characterized CMYK/Multichannel Production output.",
                );
                ui.small(
                    "Source ICC, target setup and output destination are production-only. Preview Color Management never changes the recipe or output samples.",
                );
                ui.add_space(8.0);

                match state.stage {
                    ConversionStage::SourcePreflight => {
                        egui::CollapsingHeader::new("Source information")
                            .id_salt("conversion-source-information-preflight")
                            .default_open(true)
                            .show(ui, |ui| {
                                render_source_summary(ui, &source);
                                ui.add_space(4.0);
                                render_source_profile_actions(
                                    ui,
                                    &source,
                                    &mut assign_production_profile,
                                    &mut clear_production_profile,
                                );
                                render_transparency_policy(ui, state, &source);
                            });

                        egui::CollapsingHeader::new("Source preflight")
                            .id_salt("conversion-source-preflight-foldout")
                            .default_open(true)
                            .show(ui, |ui| {
                                render_source_preflight(
                                    ui,
                                    &source,
                                    &mut navigation_action,
                                    &mut assign_production_profile,
                                    &mut open_preview_color_management,
                                );
                            });

                        ui.add_space(6.0);
                        let ready = source.report.can_convert();
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(ready, egui::Button::new("Continue to Target Setup"))
                                .on_hover_text(if ready {
                                    "Configure a verified Output ICC or DeviceLink, target topology and safe TIFF destination."
                                } else {
                                    "Resolve all blocking source-preflight findings first."
                                })
                                .clicked()
                            {
                                state.stage = ConversionStage::TargetSetup;
                            }
                            readiness_label(ui, ready, "Source preflight ready", "Conversion blocked");
                        });
                    }
                    ConversionStage::TargetSetup => {
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("← Back to Source Preflight").clicked() {
                                state.stage = ConversionStage::SourcePreflight;
                            }
                            ui.label(
                                egui::RichText::new("Source preflight ready")
                                    .color(egui::Color32::LIGHT_GREEN),
                            );
                        });
                        ui.add_space(6.0);
                        egui::CollapsingHeader::new("Source information")
                            .id_salt("conversion-source-information-target")
                            .default_open(false)
                            .show(ui, |ui| render_source_summary(ui, &source));
                        render_target_setup(
                            ui,
                            state,
                            &source,
                            &production_candidates,
                            &mut select_target_profile,
                            &mut select_output_path,
                            &mut start_conversion,
                        );
                    }
                }

                ui.add_space(6.0);
                ui.small(
                    "Target Setup does not write pixels. The original Source file remains byte-identical; production conversion starts only through the transactional worker.",
                );
                egui::CollapsingHeader::new(format!("Conversion Queue ({})", queue_rows.len()))
                    .id_salt("conversion-queue-foldout")
                    .default_open(false)
                    .show(ui, |ui| {
                        render_conversion_queue(
                            ui,
                            &queue_rows,
                            queue_paused,
                            recovered_waiting,
                            &mut queue_actions,
                        );
                    });
                    });
            });

        if !open {
            set_conversion_window_open(ctx, false);
        }
        if let Some(action) = navigation_action {
            self.dispatch_navigation_ui_action(action, ctx);
        }
        if open_preview_color_management {
            self.color.show = true;
        }
        if assign_production_profile {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .set_title("Assign Production Source ICC")
                .pick_file()
            {
                self.assign_production_source_profile(path);
            }
        }
        if clear_production_profile {
            self.clear_production_source_profile();
        }
        if select_target_profile {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC / DeviceLink profiles", &["icc", "icm"])
                .set_title(match self.color_conversion.engine_mode {
                    ConversionEngineMode::Icc => "Select Production Output ICC",
                    ConversionEngineMode::DeviceLink => "Select Production DeviceLink",
                    ConversionEngineMode::CustomOptimizer => "Select Target Characterization",
                })
                .pick_file()
            {
                match accept_target_profile_path(&mut self.color_conversion, &source, &path) {
                    Ok((description, channels)) => self.report_info(format!(
                        "Selected production target '{description}' ({channels} channels)."
                    )),
                    Err(err) => self.report_error(err),
                }
            }
        }
        if select_output_path {
            let mut dialog = rfd::FileDialog::new()
                .add_filter("Production TIFF", &["tif", "tiff"])
                .set_title("Select Production Conversion Output");
            if let Some(current) = self.color_conversion.output_path.as_deref() {
                if let Some(parent) = current.parent() {
                    dialog = dialog.set_directory(parent);
                }
                if let Some(name) = current.file_name().and_then(|name| name.to_str()) {
                    dialog = dialog.set_file_name(name);
                }
            }
            if let Some(path) = dialog.save_file() {
                match validate_conversion_output_path(&source.source_path, &path) {
                    Ok(()) => self.color_conversion.output_path = Some(path),
                    Err(err) => self.report_error(output_path_error(err)),
                }
            }
        }
        for action in queue_actions {
            match action {
                ConversionQueueUiAction::ResumeRecovered => {
                    let count = self.conversion_queue.resume_recovered();
                    self.report_info(format!("Resumed {count} recovered conversion(s)"));
                }
                ConversionQueueUiAction::TogglePaused => {
                    self.conversion_queue.set_paused(!queue_paused);
                }
                ConversionQueueUiAction::Cancel(id) => {
                    self.conversion_queue.cancel(id);
                }
                ConversionQueueUiAction::Retry(id) => {
                    self.conversion_queue.retry(id);
                }
                ConversionQueueUiAction::RecoverProject(id) => {
                    match self.conversion_queue.recover_project(id) {
                        Ok(Some(completion)) => {
                            let is_current_source =
                                self.project_path.as_ref().is_some_and(|path| {
                                    path.to_string_lossy().eq_ignore_ascii_case(
                                        &completion.capture.source_project_path.to_string_lossy(),
                                    )
                                });
                            match completion.result {
                                windows_shade_editor::conversion_queue::ConversionQueueCompletionResult::Completed(completed) => {
                                    if is_current_source {
                                        match production_project::link_source_project_to_production(
                                            &mut self.project,
                                            &completed.production_project_path,
                                        ) {
                                            Ok(()) => {
                                                self.mark_project_dirty();
                                                self.log.info(
                                                    "Recovered Production linkage changed the open Source project; explicit Save is required.",
                                                );
                                            }
                                            Err(error) => self.log.error(&format!(
                                                "Could not mirror recovered Production link in the open Source project: {error}"
                                            )),
                                        }
                                    }
                                    self.report_info(format!(
                                        "Recovered Production project: {}",
                                        completed.production_project_path.display()
                                    ));
                                }
                                _ => self.report_error(
                                    "Project-only recovery returned an unexpected conversion result.",
                                ),
                            }
                        }
                        Ok(None) => self
                            .report_info("This conversion item has no project-only recovery work."),
                        Err(error) => self
                            .report_error(format!("Production project recovery blocked: {error}")),
                    }
                }
                ConversionQueueUiAction::ClearFinished => {
                    self.conversion_queue.clear_finished();
                }
            }
        }
        if start_conversion {
            match build_target_setup_review(&self.color_conversion, &source) {
                Ok(review) => self.capture_conversion_job(&source, review),
                Err(errors) => self.report_error(errors.join(" ")),
            }
        }
    }

    fn current_conversion_source(&self) -> Option<CurrentConversionSource> {
        let face = self.faces.get(self.current_face)?;
        if !face.available {
            return None;
        }
        let owned_descriptor = face.preview.source_descriptor()?;
        let descriptor = owned_descriptor.as_borrowed();
        let source_model = face.preview.color_model();
        let save_gate = conversion_save_gate(ConversionSourceState {
            has_faces: !self.faces.is_empty(),
            has_saved_project_path: self.project_path.is_some(),
            has_unsaved_changes: self.project_dirty,
        });
        let face_ref = self.project.faces.get(self.current_face);
        let (profile, profile_label, production_profile_path, has_assigned_profile) =
            production_source_profile_state(&descriptor, source_model, face_ref);
        let profile_identity = profile.identity().cloned();
        let transparency_policy =
            if self.color_conversion.source_key.as_deref() == Some(face.path.as_path()) {
                self.color_conversion.source_transparency_policy.as_ref()
            } else {
                None
            };
        let report = build_conversion_preflight_for_source_with_policy(
            &descriptor,
            profile,
            save_gate,
            transparency_policy,
        );

        let face_label = self
            .project
            .faces
            .get(self.current_face)
            .map(|item| item.label.clone())
            .filter(|label| !label.trim().is_empty())
            .or_else(|| {
                face.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| format!("Face {}", self.current_face + 1));

        Some(CurrentConversionSource {
            face_label,
            source_path: face.path.clone(),
            source_format_label: descriptor.format.label(),
            source_model,
            transparency: descriptor.transparency,
            lossiness: descriptor.lossiness,
            execution_supported: production_execution_supported(
                descriptor.format,
                descriptor.color_model,
            ),
            color_model_label: source_model.title(),
            bit_depth: descriptor.bit_depth,
            channel_count: descriptor.channel_count,
            profile_identity,
            profile_label,
            production_profile_path,
            has_assigned_profile,
            snapshot_id: self.project.active_snapshot_id,
            save_gate,
            report,
        })
    }

    fn assign_production_source_profile(&mut self, path: PathBuf) {
        let Some(active_face) = self.faces.get(self.current_face) else {
            return;
        };
        let source_model = active_face.preview.color_model();
        let face_label = self
            .project
            .faces
            .get(self.current_face)
            .map(|face| face.label.clone())
            .unwrap_or_else(|| format!("Face {}", self.current_face + 1));

        match IccProfileRegistry.inspect(&path) {
            Ok(profile)
                if profile.compatible_with_source_model(conversion_color_model(source_model)) =>
            {
                let assignment = model::ProductionSourceProfileAssignment {
                    path: path.to_string_lossy().into_owned(),
                    identity: local_profile_identity(&profile.identity),
                };
                let Some(face) = self.project.faces.get_mut(self.current_face) else {
                    self.report_error(
                        "Cannot bind the production Source ICC: Face metadata is missing.",
                    );
                    return;
                };
                if face.production_source_profile.as_ref() != Some(&assignment) {
                    face.production_source_profile = Some(assignment);
                    self.mark_project_dirty();
                    self.report_info(format!(
                        "Assigned production Source ICC '{}' to {face_label}. Save the Source project before conversion.",
                        profile.description
                    ));
                }
            }
            Ok(profile) => self.report_error(format!(
                "Cannot assign '{}' for production conversion: profile color space {} does not match source {}.",
                profile.description,
                profile.color_space_label(),
                source_model.title(),
            )),
            Err(err) => self.report_error(err),
        }
    }

    fn clear_production_source_profile(&mut self) {
        let Some(face) = self.project.faces.get_mut(self.current_face) else {
            return;
        };
        if face.production_source_profile.take().is_some() {
            let label = face.label.clone();
            self.mark_project_dirty();
            self.report_info(format!(
                "Cleared the production Source ICC override for {label}. Embedded ICC preflight is active again."
            ));
        }
    }

    fn capture_conversion_job(
        &mut self,
        source: &CurrentConversionSource,
        review: TargetSetupReview,
    ) {
        if !source.execution_supported {
            self.report_error(
                "Production execution supports RGB TIFF/PNG/JPEG and CMYK TIFF sources; this source topology is not executable.",
            );
            return;
        }
        if self.job.is_some() {
            self.report_info("Finish the current foreground operation before queueing conversion.");
            return;
        }
        if self.export.queue.has_pending() {
            self.report_info(
                "Finish or cancel Export Queue before queueing production conversion.",
            );
            return;
        }
        let Some(source_project_path) = self.project_path.clone() else {
            self.report_error("Save the Source project before queueing production conversion.");
            return;
        };
        if self
            .export
            .queue
            .reserved_destination_keys()
            .contains(&path_safety::path_key(&review.effective_output_path))
        {
            self.report_error("The selected TIFF is already reserved by Export Queue.");
            return;
        }
        let source_profile = source
            .production_profile_path
            .as_deref()
            .map(PathBuf::from)
            .map(|path| CapturedSourceProfile::External { path })
            .unwrap_or(CapturedSourceProfile::Embedded);
        let project = self.project.clone();
        let source_face_path = source.source_path.clone();
        let source_snapshot_id = source.snapshot_id;
        let output_tiff_path = review.effective_output_path;
        let production_project_path = review.production_project_path;
        let production_project_disposition = review.production_project_disposition;
        let target_name = review.recipe.target.name.clone();
        let production_project_name = format!("{} - {target_name}", project.name);
        let output_face_label = format!("{} - {target_name}", source.face_label);
        let recipe = review.recipe;
        let output_policy = match self.color_conversion.collision_policy {
            OutputCollisionPolicy::Versioned => CapturedOutputPolicy::MustNotExist,
            OutputCollisionPolicy::TransactionalReplace => {
                CapturedOutputPolicy::TransactionalReplace
            }
        };
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Capturing production conversion", move |progress| {
            Self::set_progress(
                &progress,
                Some(0.05),
                "Capturing production conversion",
                "Hashing saved Source project",
            );
            let result = (|| {
                let captured_project: windows_shade_editor::model::ShadeProject =
                    serde_json::from_value(serde_json::to_value(&project).map_err(|error| {
                        format!("Cannot serialize Source project for conversion capture: {error}")
                    })?)
                    .map_err(|error| {
                        format!("Cannot materialize Source project capture: {error}")
                    })?;
                let source_project_file_sha256 =
                    windows_shade_editor::icc_conversion_worker::sha256_file(&source_project_path)?;
                Self::set_progress(
                    &progress,
                    Some(0.45),
                    "Capturing production conversion",
                    "Hashing immutable source artwork",
                );
                let source_file_sha256 =
                    windows_shade_editor::icc_conversion_worker::sha256_file(&source_face_path)?;
                Self::set_progress(
                    &progress,
                    Some(0.90),
                    "Capturing production conversion",
                    "Freezing saved recipe and destinations",
                );
                ConversionJobCapture::capture(
                    &captured_project,
                    source_project_path,
                    source_project_file_sha256,
                    source_face_path,
                    source_snapshot_id,
                    source_file_sha256,
                    source_profile,
                    recipe,
                    output_policy,
                    output_tiff_path,
                    production_project_path,
                    production_project_name,
                    output_face_label,
                )
            })();
            Self::set_progress(
                &progress,
                Some(1.0),
                "Capturing production conversion",
                "Complete",
            );
            JobResult::ConversionCapture {
                result,
                production_project_disposition,
                default_dpi,
            }
        });
    }
}

fn render_source_summary(ui: &mut egui::Ui, source: &CurrentConversionSource) {
    let snapshot = source
        .snapshot_id
        .map(|id| format!("#{id}"))
        .unwrap_or_else(|| "Current saved project state".to_owned());
    let rows = [
        (("Face", source.face_label.clone()), ("Source format", source.source_format_label.to_owned())),
        (("Color model", source.color_model_label.to_owned()), ("Bit depth", format!("{}-bit", source.bit_depth))),
        (("Channels", source.channel_count.to_string()), ("Compression", source.lossiness.label().to_owned())),
        (("Transparency", source.transparency.label().to_owned()), ("Saved state", save_gate_label(source.save_gate).to_owned())),
        (("Production Source ICC", source.profile_label.clone()), ("Snapshot", snapshot)),
    ];

    egui::Grid::new("conversion-source-summary")
        .num_columns(4)
        .striped(true)
        .spacing([12.0, 5.0])
        .show(ui, |ui| {
            for ((left_label, left_value), (right_label, right_value)) in rows {
                ui.strong(left_label);
                ui.label(left_value);
                ui.strong(right_label);
                ui.label(right_value);
                ui.end_row();
            }
        });
    ui.horizontal_wrapped(|ui| {
        ui.strong("Source");
        ui.small(source.source_path.display().to_string())
            .on_hover_text(source.source_path.display().to_string());
    });
}

fn render_source_profile_actions(
    ui: &mut egui::Ui,
    source: &CurrentConversionSource,
    assign: &mut bool,
    clear: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        let label = if source.has_assigned_profile {
            "Reassign Production Source ICC..."
        } else {
            "Assign Production Source ICC..."
        };
        if ui.button(label).clicked() {
            *assign = true;
        }
        if ui
            .add_enabled(
                source.has_assigned_profile,
                egui::Button::new("Use embedded Source ICC"),
            )
            .on_hover_text(
                "Clear the explicit production assignment and return to the Face's embedded ICC.",
            )
            .clicked()
        {
            *clear = true;
        }
    });
    if let Some(path) = source.production_profile_path.as_deref() {
        ui.small(format!("Assigned profile path: {path}"));
    }
}

fn render_transparency_policy(
    ui: &mut egui::Ui,
    state: &mut ColorConversionUiState,
    source: &CurrentConversionSource,
) {
    if source.transparency == TransparencyState::None {
        state.source_transparency_policy = None;
        return;
    }

    ui.add_space(8.0);
    ui.group(|ui| {
        ui.strong("PNG transparency handling");
        ui.small(
            "Alpha is not an ink channel. Production conversion requires an explicit solid background; the source file is never modified.",
        );

        let mut enabled = state.source_transparency_policy.is_some();
        if ui
            .checkbox(&mut enabled, "Flatten alpha against an explicit solid RGB background")
            .changed()
        {
            state.source_transparency_policy = if enabled {
                Some(SourceTransparencyPolicy::FlattenSolidRgb16 {
                    background_rgb: [u16::MAX; 3],
                })
            } else {
                None
            };
        }

        if enabled {
            let background = state
                .source_transparency_policy
                .unwrap_or(SourceTransparencyPolicy::FlattenSolidRgb16 {
                    background_rgb: [u16::MAX; 3],
                })
                .background_rgb();
            let mut rgb8 = background.map(|value| ((u32::from(value) + 128) / 257) as u8);
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                ui.label("Background RGB");
                for (label, value) in ["R", "G", "B"].into_iter().zip(rgb8.iter_mut()) {
                    ui.label(label);
                    changed |= ui
                        .add(egui::DragValue::new(value).range(0..=255))
                        .changed();
                }
            });
            if changed || state.source_transparency_policy.is_none() {
                state.source_transparency_policy =
                    Some(SourceTransparencyPolicy::FlattenSolidRgb16 {
                        background_rgb: rgb8.map(|value| u16::from(value) * 257),
                    });
            }
            if let Some(policy) = state.source_transparency_policy {
                ui.small(format!("Recipe policy: {}", policy.label()));
            }
        } else {
            ui.label(
                egui::RichText::new("No flatten policy selected — conversion remains blocked")
                    .color(egui::Color32::LIGHT_RED),
            );
        }
    });
    ui.add_space(4.0);
}

fn render_source_preflight(
    ui: &mut egui::Ui,
    source: &CurrentConversionSource,
    navigation_action: &mut Option<NavigationUiAction>,
    assign_profile: &mut bool,
    open_preview: &mut bool,
) {
    if source.report.findings.is_empty() {
        ui.label(egui::RichText::new("Ready").color(egui::Color32::LIGHT_GREEN));
        return;
    }
    for finding in &source.report.findings {
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(severity_label(finding.severity))
                        .color(severity_color(finding.severity))
                        .strong(),
                );
                ui.strong(finding.title);
            });
            ui.label(&finding.detail);

            match finding.code {
                PreflightCode::UnsavedSourceProject => match source.save_gate {
                    ConversionSaveGate::SaveAsRequired => {
                        if ui.button("Save Source Project As...").clicked() {
                            *navigation_action = Some(NavigationUiAction::SaveAs);
                        }
                    }
                    ConversionSaveGate::SaveRequired => {
                        if ui.button("Save & Continue").clicked() {
                            *navigation_action = Some(NavigationUiAction::Save);
                        }
                    }
                    ConversionSaveGate::Ready | ConversionSaveGate::NoSourceFaces => {}
                },
                PreflightCode::MissingSourceProfile | PreflightCode::InvalidSourceProfile => {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Assign Production Source ICC...").clicked() {
                            *assign_profile = true;
                        }
                        if ui.button("Open Color Management / ICC Preview").clicked() {
                            *open_preview = true;
                        }
                        ui.small(
                            "ICC Preview is display-only and never satisfies production Source ICC preflight.",
                        );
                    });
                }
                _ => {}
            }
        });
        ui.add_space(4.0);
    }
}

fn refresh_installed_target_profiles(
    state: &mut ColorConversionUiState,
    source: &CurrentConversionSource,
) {
    state.installed_profiles_loaded = true;
    match installed_production_profiles(
        IccProfileRegistry,
        state.engine_mode,
        conversion_color_model(source.source_model),
        "",
        true,
    ) {
        Ok(rows) => {
            state.installed_target_profiles = rows;
            state.installed_profiles_error = None;
        }
        Err(error) => {
            state.installed_target_profiles.clear();
            state.installed_profiles_error = Some(error);
        }
    }
}

fn accept_target_profile_path(
    state: &mut ColorConversionUiState,
    source: &CurrentConversionSource,
    path: &Path,
) -> Result<(String, usize), String> {
    inspect_production_profile_candidate(
        IccProfileRegistry,
        path,
        state.engine_mode,
        conversion_color_model(source.source_model),
    )?;
    let profile = windows_shade_editor::production_target::inspect_production_target_profile(
        path,
        state.engine_mode,
        conversion_color_model(source.source_model),
    )?;
    let description = profile.identity.description.clone();
    let channels = profile.output_channel_count;
    state.accept_target_profile(profile, &source.source_path);
    Ok((description, channels))
}

fn render_target_setup(
    ui: &mut egui::Ui,
    state: &mut ColorConversionUiState,
    source: &CurrentConversionSource,
    production_candidates: &[ProductionDestinationCandidate],
    select_target_profile: &mut bool,
    select_output_path: &mut bool,
    start_conversion: &mut bool,
) {
    egui::CollapsingHeader::new("Target setup")
        .id_salt("conversion-target-setup-foldout")
        .default_open(true)
        .show(ui, |ui| {
    let previous_mode = state.engine_mode;
    egui::ComboBox::from_label("Conversion engine")
        .selected_text(engine_mode_label(state.engine_mode))
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut state.engine_mode,
                ConversionEngineMode::Icc,
                "Standard Output ICC",
            );
            ui.selectable_value(
                &mut state.engine_mode,
                ConversionEngineMode::DeviceLink,
                "DeviceLink",
            );
            ui.add_enabled(
                false,
                egui::Button::selectable(
                    false,
                    "Custom N-ink optimizer — needs characterization",
                ),
            )
            .on_hover_text(
                "Custom optimizer cannot be enabled until versioned measured target characterization is available.",
            );
        });
    if previous_mode != state.engine_mode {
        state.clear_target_profile();
        state.clear_installed_profile_catalog();
        state.black_point_compensation = state.engine_mode == ConversionEngineMode::Icc;
    }

    ui.group(|ui| {
        ui.strong("Production target profile");
        ui.horizontal_wrapped(|ui| {
            let installed_label = if state.installed_profiles_loaded {
                "Refresh installed profiles"
            } else {
                "Browse installed profiles"
            };
            if ui
                .add_enabled(
                    state.engine_mode != ConversionEngineMode::CustomOptimizer,
                    egui::Button::new(installed_label),
                )
                .clicked()
            {
                refresh_installed_target_profiles(state, source);
            }
            if ui
                .button(match state.engine_mode {
                    ConversionEngineMode::Icc => "Browse Output ICC file...",
                    ConversionEngineMode::DeviceLink => "Browse DeviceLink file...",
                    ConversionEngineMode::CustomOptimizer => "Select characterization...",
                })
                .clicked()
            {
                *select_target_profile = true;
            }
        });

        if state.installed_profiles_loaded {
            ui.horizontal_wrapped(|ui| {
                ui.label("Filter");
                ui.add(
                    egui::TextEdit::singleline(&mut state.target_profile_query)
                        .desired_width(260.0)
                        .hint_text("description, filename, role, color space"),
                );
                ui.checkbox(
                    &mut state.show_incompatible_profiles,
                    "Show incompatible",
                );
            });

            let mut selected_installed: Option<PathBuf> = None;
            let mut visible_count = 0usize;
            egui::ScrollArea::vertical()
                .id_salt("conversion-installed-production-profiles")
                .max_height(160.0)
                .show(ui, |ui| {
                    for candidate in &state.installed_target_profiles {
                        if !candidate.profile.matches_query(&state.target_profile_query) {
                            continue;
                        }
                        if !state.show_incompatible_profiles && !candidate.selectable() {
                            continue;
                        }
                        visible_count += 1;
                        let topology = if candidate.profile.is_device_link() {
                            format!(
                                "{} → {} · {}ch",
                                candidate.profile.color_space_label(),
                                candidate.profile.pcs_space_label(),
                                candidate.profile.pcs_space_channels(),
                            )
                        } else {
                            format!(
                                "{} · {}ch",
                                candidate.profile.color_space_label(),
                                candidate.profile.color_space_channels(),
                            )
                        };
                        let label = format!(
                            "{} · {} · {}",
                            candidate.profile.description,
                            candidate.profile.role.label(),
                            topology,
                        );
                        let selected = state.target_profile.as_ref().is_some_and(|current| {
                            current.path == candidate.profile.path
                                && current
                                    .identity
                                    .sha256
                                    .eq_ignore_ascii_case(&candidate.profile.identity.sha256)
                        });
                        let response = ui.add_enabled(
                            candidate.selectable(),
                            egui::Button::selectable(selected, label),
                        );
                        let response = if let Some(rejection) = candidate.rejection {
                            response.on_hover_text(rejection.label())
                        } else {
                            response.on_hover_text(candidate.profile.path.display().to_string())
                        };
                        if response.clicked() {
                            selected_installed = Some(candidate.profile.path.clone());
                        }
                    }
                });
            ui.small(format!("{visible_count} installed profile(s) shown"));

            if let Some(path) = selected_installed {
                if let Err(error) = accept_target_profile_path(state, source, &path) {
                    state.installed_profiles_error = Some(error);
                }
            }
        }

        if let Some(error) = state.installed_profiles_error.as_deref() {
            ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
        }

        if let Some(profile) = state.target_profile.as_ref() {
            ui.label(
                egui::RichText::new(format!("Selected: {}", profile.identity.description))
                    .color(egui::Color32::LIGHT_GREEN),
            );
        } else {
            ui.label(egui::RichText::new("No target selected").color(egui::Color32::LIGHT_RED));
        }
    });

    if let Some(profile) = state.target_profile.as_ref() {
        egui::Grid::new("conversion-target-profile-summary")
            .num_columns(2)
            .striped(true)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for (label, value) in [
                    ("Profile path", profile.path.display().to_string()),
                    ("Profile class", profile.device_class_label.clone()),
                    (
                        "Input space",
                        profile
                            .source_space_label
                            .clone()
                            .unwrap_or_else(|| "Source ICC → PCS".to_owned()),
                    ),
                    ("Output space", profile.output_space_label.clone()),
                    ("Output channels", profile.output_channel_count.to_string()),
                    (
                        "Profile SHA-256",
                        short_hash(&profile.identity.sha256).to_owned(),
                    ),
                ] {
                    ui.strong(label);
                    ui.label(value);
                    ui.end_row();
                }
            });

        ui.horizontal(|ui| {
            ui.label("Target name");
            ui.text_edit_singleline(&mut state.target_name);
        });

        ui.strong("Authoritative output topology");
        if profile.channel_names_authoritative {
            ui.small("Channel order is defined by standard CMYK semantics or the profile colorant table.");
        } else {
            ui.label(
                egui::RichText::new(
                    "The profile does not carry a complete colorant-name table. Enter the real RIP/ink order and confirm it explicitly.",
                )
                .color(egui::Color32::YELLOW),
            );
        }
        egui::Grid::new("conversion-target-channel-topology")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (index, name) in state.channel_names.iter_mut().enumerate() {
                    ui.label(format!("{}", index + 1));
                    if profile.channel_names_authoritative {
                        ui.label(name.as_str());
                    } else if ui.text_edit_singleline(name).changed() {
                        state.channel_names_confirmed = false;
                    }
                    ui.end_row();
                }
            });
        if !profile.channel_names_authoritative {
            ui.checkbox(
                &mut state.channel_names_confirmed,
                "I confirm this is the real production channel order for the selected target",
            );
        }
    }

    ui.horizontal_wrapped(|ui| {
        ui.strong("Output precision");
        ui.radio_value(&mut state.output_bit_depth, 8, "8-bit");
        ui.radio_value(&mut state.output_bit_depth, 16, "16-bit");
    });

    let capabilities = capabilities_for_engine(state.engine_mode);
    ui.horizontal_wrapped(|ui| {
        ui.strong("Rendering intent");
        if capabilities.rendering_intent == ControlAvailability::Available {
            egui::ComboBox::from_id_salt("conversion-rendering-intent")
                .selected_text(rendering_intent_label(state.rendering_intent))
                .show_ui(ui, |ui| {
                    for intent in [
                        ConversionRenderingIntent::Perceptual,
                        ConversionRenderingIntent::RelativeColorimetric,
                        ConversionRenderingIntent::Saturation,
                        ConversionRenderingIntent::AbsoluteColorimetric,
                    ] {
                        ui.selectable_value(
                            &mut state.rendering_intent,
                            intent,
                            rendering_intent_label(intent),
                        );
                    }
                });
        } else {
            ui.label("Fixed by DeviceLink");
        }
        ui.add_enabled(
            capabilities.black_point_compensation == ControlAvailability::Available,
            egui::Checkbox::new(&mut state.black_point_compensation, "Black Point Compensation"),
        )
        .on_hover_text(if capabilities.black_point_compensation == ControlAvailability::Available {
            "Apply LittleCMS black-point compensation to the standard ICC transform."
        } else {
            "DeviceLink separation behavior is fixed by the link; BPC is not an additional runtime control."
        });
    });

    });

    egui::CollapsingHeader::new("Production destination")
        .id_salt("conversion-production-destination-foldout")
        .default_open(true)
        .show(ui, |ui| {
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(
            &mut state.destination_mode,
            ProductionDestinationMode::CreateNew,
            "Create new Production project",
        );
        ui.radio_value(
            &mut state.destination_mode,
            ProductionDestinationMode::AppendExisting,
            "Add to compatible Production project",
        );
    });

    if state.destination_mode == ProductionDestinationMode::AppendExisting {
        state.collision_policy = OutputCollisionPolicy::Versioned;
        if production_candidates.is_empty() {
            ui.label(
                egui::RichText::new(
                    "No linked Production projects are recorded for this Source project.",
                )
                .color(egui::Color32::YELLOW),
            );
        }
        for candidate in production_candidates {
            let selected = state.selected_existing.as_ref().is_some_and(|current| {
                current
                    .path
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&candidate.path.to_string_lossy())
                    && current.project_sha256 == candidate.project_sha256
            });
            let title = format!(
                "{} · {} Face(s) · {}",
                candidate
                    .project_name
                    .as_deref()
                    .unwrap_or("Production project"),
                candidate.face_count.unwrap_or(0),
                candidate.path.display()
            );
            let response = ui.add_enabled(
                candidate.can_append(),
                egui::Button::selectable(selected, title),
            );
            let response = if let Some(diagnostic) = candidate.diagnostic.as_deref() {
                response.on_hover_text(diagnostic)
            } else {
                response
            };
            if response.clicked() {
                match seed_state_from_existing_destination(state, source, candidate) {
                    Ok(()) => state.destination_error = None,
                    Err(error) => {
                        state.selected_existing = None;
                        state.destination_error = Some(error);
                    }
                }
            }
            if !candidate.can_append() {
                if let Some(diagnostic) = candidate.diagnostic.as_deref() {
                    ui.small(egui::RichText::new(diagnostic).color(egui::Color32::LIGHT_RED));
                }
            }
        }
        ui.small(
            "Append Existing always creates a new/versioned TIFF. Replacing a prior Production Face is a separate explicit re-conversion workflow.",
        );
    } else {
        state.selected_existing = None;
    }
    if let Some(error) = state.destination_error.as_deref() {
        ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
    }

    ui.horizontal_wrapped(|ui| {
        if ui.button("Select TIFF output...").clicked() {
            *select_output_path = true;
        }
        ui.label(
            state
                .output_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "No destination selected".to_owned()),
        );
    });
    if state.destination_mode == ProductionDestinationMode::CreateNew {
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(
                &mut state.collision_policy,
                OutputCollisionPolicy::Versioned,
                "Create versioned output (safe default)",
            );
            ui.radio_value(
                &mut state.collision_policy,
                OutputCollisionPolicy::TransactionalReplace,
                "Explicit transactional replacement",
            );
        });
    }

    });

    egui::CollapsingHeader::new("Recipe review")
        .id_salt("conversion-recipe-review-foldout")
        .default_open(true)
        .show(ui, |ui| {
    match build_target_setup_review(state, source) {
        Ok(review) => {
            egui::Grid::new("conversion-recipe-review")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (label, value) in [
                        (
                            "Engine",
                            engine_mode_label(review.recipe.engine_mode).to_owned(),
                        ),
                        ("Target", review.recipe.target.name.clone()),
                        (
                            "Topology",
                            format!(
                                "{} channels · {}-bit",
                                review.recipe.target.channels.len(),
                                review.recipe.target.bit_depth
                            ),
                        ),
                        (
                            "Effective output",
                            review.effective_output_path.display().to_string(),
                        ),
                        (
                            "Production project",
                            review.production_project_path.display().to_string(),
                        ),
                        (
                            "Transparency policy",
                            review
                                .recipe
                                .source_transparency_policy
                                .map(SourceTransparencyPolicy::label)
                                .unwrap_or_else(|| "No flattening".to_owned()),
                        ),
                        ("Recipe SHA-256", review.recipe_sha256),
                    ] {
                        ui.strong(label);
                        ui.label(value);
                        ui.end_row();
                    }
                });
            ui.label(
                egui::RichText::new("Target setup ready")
                    .color(egui::Color32::LIGHT_GREEN)
                    .strong(),
            );
            let engine_executable = matches!(
                review.recipe.engine_mode,
                ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink
            );
            let executable = engine_executable && source.execution_supported;
            if ui
                .add_enabled(
                    executable,
                    egui::Button::new("Queue Production Conversion"),
                )
                .on_hover_text(if executable {
                    "Capture the exact saved Source state and add it to the persistent conversion queue."
                } else if !source.execution_supported {
                    "Production execution currently supports RGB TIFF/PNG/JPEG and CMYK TIFF sources."
                } else {
                    "Custom Optimizer execution requires its dedicated engine."
                })
                .clicked()
            {
                *start_conversion = true;
            }
        }
        Err(errors) => {
            for error in errors {
                ui.label(egui::RichText::new(format!("• {error}")).color(egui::Color32::LIGHT_RED));
            }
            ui.add_enabled(false, egui::Button::new("Start Production Conversion"))
                .on_hover_text("Complete all target setup fields first.");
        }
    }
    });
}

fn seed_state_from_existing_destination(
    state: &mut ColorConversionUiState,
    source: &CurrentConversionSource,
    candidate: &ProductionDestinationCandidate,
) -> Result<(), String> {
    let recipe = candidate
        .baseline_recipe
        .as_ref()
        .ok_or_else(|| "Selected Production project has no validated baseline recipe.".to_owned())?
        .clone();
    let (profile_path, profile_identity) = match recipe.engine_mode {
        ConversionEngineMode::Icc => (
            recipe.target.output_profile_path.as_deref(),
            recipe.target.output_profile_identity.as_ref(),
        ),
        ConversionEngineMode::DeviceLink => (
            recipe.target.device_link_path.as_deref(),
            recipe.target.device_link_identity.as_ref(),
        ),
        ConversionEngineMode::CustomOptimizer => {
            return Err(
                "Custom Optimizer Production destinations require the dedicated characterized-target UI."
                    .to_owned(),
            );
        }
    };
    let profile_path = profile_path.ok_or_else(|| {
        "Selected Production recipe does not contain its external target profile path.".to_owned()
    })?;
    let profile_identity = profile_identity.ok_or_else(|| {
        "Selected Production recipe does not contain its target profile identity.".to_owned()
    })?;
    verify_production_profile_candidate(
        IccProfileRegistry,
        Path::new(profile_path),
        profile_identity,
        recipe.engine_mode,
        conversion_color_model(source.source_model),
    )?;
    let verified = verify_production_target_profile(
        Path::new(profile_path),
        profile_identity,
        recipe.engine_mode,
        conversion_color_model(source.source_model),
    )?;

    state.engine_mode = recipe.engine_mode;
    state.accept_target_profile(verified, &source.source_path);
    state.target_name = recipe.target.name.clone();
    state.channel_names = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect();
    state.channel_names_confirmed = true;
    state.output_bit_depth = recipe.target.bit_depth;
    state.rendering_intent = recipe.rendering_intent;
    state.black_point_compensation = recipe.black_point_compensation;
    state.collision_policy = OutputCollisionPolicy::Versioned;
    state.selected_existing = Some(candidate.clone());
    Ok(())
}

fn build_target_setup_review(
    state: &ColorConversionUiState,
    source: &CurrentConversionSource,
) -> Result<TargetSetupReview, Vec<String>> {
    let mut errors = Vec::new();
    let Some(stored_profile) = state.target_profile.as_ref() else {
        return Err(vec![
            "Select a production Output ICC or DeviceLink.".to_owned(),
        ]);
    };

    if let Err(err) = verify_production_profile_candidate(
        IccProfileRegistry,
        &stored_profile.path,
        &stored_profile.identity,
        state.engine_mode,
        conversion_color_model(source.source_model),
    ) {
        errors.push(err);
    }
    let inspected = match verify_production_target_profile(
        &stored_profile.path,
        &stored_profile.identity,
        state.engine_mode,
        conversion_color_model(source.source_model),
    ) {
        Ok(profile) => profile,
        Err(err) => {
            errors.push(err);
            stored_profile.clone()
        }
    };

    if state.target_name.trim().is_empty() {
        errors.push("Target name cannot be empty.".to_owned());
    }
    if let Err(err) =
        validate_target_channel_names(&state.channel_names, inspected.output_channel_count)
    {
        errors.push(err);
    }
    if !inspected.channel_names_authoritative && !state.channel_names_confirmed {
        errors.push(
            "Confirm the real production channel order because the profile does not provide authoritative colorant names."
                .to_owned(),
        );
    }
    if !matches!(state.output_bit_depth, 8 | 16) {
        errors.push("Output bit depth must be 8 or 16.".to_owned());
    }

    let Some(source_profile_identity) = source.profile_identity.clone() else {
        errors.push("Source ICC identity is not ready.".to_owned());
        return Err(errors);
    };
    let Some(preferred_output) = state.output_path.as_deref() else {
        errors.push("Select a production TIFF destination.".to_owned());
        return Err(errors);
    };
    let preferred_output = crate::tiff_output::canonical_destination(preferred_output);
    if let Err(err) = validate_conversion_output_path(&source.source_path, &preferred_output) {
        errors.push(output_path_error(err));
    }
    let effective_output_path = match state.collision_policy {
        OutputCollisionPolicy::Versioned => match next_versioned_output_path(&preferred_output) {
            Ok(path) => path,
            Err(err) => {
                errors.push(output_path_error(err));
                preferred_output.clone()
            }
        },
        OutputCollisionPolicy::TransactionalReplace => preferred_output,
    };
    let create_new_project_path = effective_output_path.with_extension("shade");
    if state.destination_mode == ProductionDestinationMode::CreateNew
        && state.collision_policy == OutputCollisionPolicy::Versioned
        && create_new_project_path.exists()
    {
        errors.push(format!(
            "Production project already exists: {}. Select another TIFF name or explicitly choose transactional replacement.",
            create_new_project_path.display()
        ));
    }

    let profile_path = inspected.path.to_string_lossy().into_owned();
    let profile_identity = inspected.identity.clone();
    let (output_profile_path, output_profile_identity, device_link_path, device_link_identity) =
        match state.engine_mode {
            ConversionEngineMode::Icc => (Some(profile_path), Some(profile_identity), None, None),
            ConversionEngineMode::DeviceLink => {
                (None, None, Some(profile_path), Some(profile_identity))
            }
            ConversionEngineMode::CustomOptimizer => (None, None, None, None),
        };
    let recipe = ConversionRecipe {
        source_transparency_policy: state.source_transparency_policy,
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: state.engine_mode,
        source_profile_identity,
        target: ConversionTargetDefinition {
            name: state.target_name.trim().to_owned(),
            channels: state
                .channel_names
                .iter()
                .map(|name| TargetChannelDefinition {
                    name: name.trim().to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: None,
                })
                .collect(),
            bit_depth: state.output_bit_depth,
            output_profile_identity,
            output_profile_path,
            device_link_identity,
            device_link_path,
            characterization_id: None,
            total_ink_limit: None,
        },
        rendering_intent: state.rendering_intent,
        black_point_compensation: state.engine_mode == ConversionEngineMode::Icc
            && state.black_point_compensation,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: (state.engine_mode == ConversionEngineMode::CustomOptimizer).then(
            windows_shade_editor::custom_optimizer_config::CustomOptimizerSolverConfig::default,
        ),
    };
    if let Err(recipe_errors) = recipe.validate() {
        errors.extend(recipe_errors);
    }
    let frozen_destination = match state.destination_mode {
        ProductionDestinationMode::CreateNew => {
            FrozenProductionDestination::create_new(create_new_project_path)
        }
        ProductionDestinationMode::AppendExisting => state
            .selected_existing
            .as_ref()
            .ok_or_else(|| "Select a compatible linked Production project.".to_owned())
            .and_then(|candidate| FrozenProductionDestination::append_existing(candidate, &recipe)),
    };
    let frozen_destination = match frozen_destination {
        Ok(destination) => Some(destination),
        Err(error) => {
            errors.push(error);
            None
        }
    };
    if !errors.is_empty() {
        return Err(errors);
    }
    let frozen_destination =
        frozen_destination.expect("destination validated when errors are empty");
    let recipe_sha256 = recipe_sha256(&recipe).map_err(|err| vec![err])?;
    Ok(TargetSetupReview {
        recipe,
        recipe_sha256,
        effective_output_path,
        production_project_path: frozen_destination.production_project_path,
        production_project_disposition: frozen_destination.disposition,
    })
}

fn render_conversion_queue(
    ui: &mut egui::Ui,
    rows: &[ConversionQueueRow],
    paused: bool,
    recovered_waiting: usize,
    actions: &mut Vec<ConversionQueueUiAction>,
) {
    use windows_shade_editor::conversion_queue::ConversionQueueStatus;

    ui.horizontal_wrapped(|ui| {
        if ui
            .button(if paused {
                "Resume queue"
            } else {
                "Pause queue"
            })
            .clicked()
        {
            actions.push(ConversionQueueUiAction::TogglePaused);
        }
        if recovered_waiting > 0
            && ui
                .button(format!("Resume {recovered_waiting} recovered"))
                .clicked()
        {
            actions.push(ConversionQueueUiAction::ResumeRecovered);
        }
        if rows.iter().any(|row| {
            matches!(
                row.status,
                ConversionQueueStatus::Done
                    | ConversionQueueStatus::Failed
                    | ConversionQueueStatus::Cancelled
            )
        }) && ui.button("Clear finished").clicked()
        {
            actions.push(ConversionQueueUiAction::ClearFinished);
        }
    });
    if rows.is_empty() {
        ui.small("No production conversions queued.");
        return;
    }
    for row in rows {
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("#{} {}", row.id, row.label));
                ui.label(row.status.label());
                if row.requires_resume {
                    ui.label(
                        egui::RichText::new("Recovered / paused").color(egui::Color32::YELLOW),
                    );
                }
            });
            if row.status == ConversionQueueStatus::Processing {
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
            if let Some(error) = &row.error {
                ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
            }
            ui.horizontal(|ui| match row.status {
                ConversionQueueStatus::Waiting | ConversionQueueStatus::Processing => {
                    if ui.button("Cancel").clicked() {
                        actions.push(ConversionQueueUiAction::Cancel(row.id));
                    }
                }
                ConversionQueueStatus::Failed | ConversionQueueStatus::Cancelled => {
                    if ui.button("Retry safely").clicked() {
                        actions.push(ConversionQueueUiAction::Retry(row.id));
                    }
                }
                ConversionQueueStatus::NeedsRecovery => {
                    if ui
                        .button("Recover Project")
                        .on_hover_text(
                            "Complete only the Production .shade save. The committed TIFF is verified and never rendered again.",
                        )
                        .clicked()
                    {
                        actions.push(ConversionQueueUiAction::RecoverProject(row.id));
                    }
                }
                ConversionQueueStatus::Done => {}
            });
        });
    }
}

fn recommended_output_path(
    source: &Path,
    target_name: &str,
    channel_count: usize,
) -> Result<PathBuf, OutputPathError> {
    let suffix = if target_name.trim().is_empty() {
        if channel_count == 4 {
            "CMYK".to_owned()
        } else {
            format!("{channel_count}C")
        }
    } else {
        target_name.to_owned()
    };
    let filename = default_converted_filename(source, &suffix)?;
    Ok(source
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join("Production")
        .join(filename))
}

fn production_execution_supported(
    format: SourceImageFormat,
    model: DesignSourceColorModel,
) -> bool {
    matches!(
        (format, model),
        (SourceImageFormat::Tiff, DesignSourceColorModel::Rgb)
            | (SourceImageFormat::Tiff, DesignSourceColorModel::Cmyk)
            | (SourceImageFormat::Png, DesignSourceColorModel::Rgb)
            | (SourceImageFormat::Jpeg, DesignSourceColorModel::Rgb)
    )
}

fn conversion_color_model(model: RuntimeColorModel) -> ConversionColorModel {
    match model {
        RuntimeColorModel::Gray => ConversionColorModel::Gray,
        RuntimeColorModel::Rgb => ConversionColorModel::Rgb,
        RuntimeColorModel::Cmyk => ConversionColorModel::Cmyk,
        RuntimeColorModel::Other => ConversionColorModel::Other,
    }
}

fn production_source_profile_state(
    descriptor: &windows_shade_editor::design_source::DesignSourceDescriptor<'_>,
    source_model: RuntimeColorModel,
    face: Option<&model::FaceRef>,
) -> (SourceProfileState, String, Option<String>, bool) {
    if let Some(assignment) = face.and_then(|face| face.production_source_profile.as_ref()) {
        let path = PathBuf::from(&assignment.path);
        let expected = conversion_profile_identity(&assignment.identity);
        return match IccProfileRegistry.verify_identity(&path, &expected) {
            Ok(profile)
                if profile.compatible_with_source_model(conversion_color_model(source_model)) =>
            {
                (
                    SourceProfileState::Assigned(profile.identity.clone()),
                    format!("Assigned: {}", profile.description),
                    Some(assignment.path.clone()),
                    true,
                )
            }
            Ok(profile) => (
                SourceProfileState::Invalid(format!(
                    "Assigned production Source ICC '{}' declares {} but the source Face is {}.",
                    profile.description,
                    profile.color_space_label(),
                    source_model.title(),
                )),
                format!(
                    "Assigned profile invalid: {}",
                    assignment.identity.description
                ),
                Some(assignment.path.clone()),
                true,
            ),
            Err(err) => (
                SourceProfileState::Invalid(err),
                format!(
                    "Assigned profile invalid: {}",
                    assignment.identity.description
                ),
                Some(assignment.path.clone()),
                true,
            ),
        };
    }

    match color_management::production_source_profile_identity_or_rgb_fallback_for_runtime(
        source_model,
        descriptor.embedded_icc,
    ) {
        Ok(Some(identity)) => {
            let converted = conversion_profile_identity(&identity);
            let label = if windows_shade_editor::source_profile_fallback::is_srgb_fallback_identity(&converted) {
                format!("No Source ICC · fallback: {}", identity.description)
            } else {
                format!("Embedded: {}", identity.description)
            };
            (
                SourceProfileState::Embedded(converted),
                label,
                None,
                false,
            )
        },
        Ok(None) => (
            SourceProfileState::Missing,
            "Missing production Source ICC".to_owned(),
            None,
            false,
        ),
        Err(err) => (
            SourceProfileState::Invalid(err),
            "Embedded production Source ICC is invalid".to_owned(),
            None,
            false,
        ),
    }
}

fn conversion_profile_identity(
    identity: &model::IccProfileIdentity,
) -> ConversionIccProfileIdentity {
    ConversionIccProfileIdentity {
        description: identity.description.clone(),
        sha256: identity.sha256.clone(),
    }
}

fn local_profile_identity(
    identity: &ConversionIccProfileIdentity,
) -> model::IccProfileIdentity {
    model::IccProfileIdentity {
        description: identity.description.clone(),
        sha256: identity.sha256.clone(),
    }
}

fn readiness_label(ui: &mut egui::Ui, ready: bool, ready_text: &str, blocked_text: &str) {
    ui.label(
        egui::RichText::new(if ready { ready_text } else { blocked_text }).color(if ready {
            egui::Color32::LIGHT_GREEN
        } else {
            egui::Color32::LIGHT_RED
        }),
    );
}

fn engine_mode_label(mode: ConversionEngineMode) -> &'static str {
    match mode {
        ConversionEngineMode::Icc => "Standard Output ICC",
        ConversionEngineMode::DeviceLink => "DeviceLink",
        ConversionEngineMode::CustomOptimizer => "Custom N-ink optimizer",
    }
}

fn rendering_intent_label(intent: ConversionRenderingIntent) -> &'static str {
    match intent {
        ConversionRenderingIntent::Perceptual => "Perceptual",
        ConversionRenderingIntent::RelativeColorimetric => "Relative colorimetric",
        ConversionRenderingIntent::Saturation => "Saturation",
        ConversionRenderingIntent::AbsoluteColorimetric => "Absolute colorimetric",
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn output_path_error(error: OutputPathError) -> String {
    match error {
        OutputPathError::SameAsSource => {
            "Production destination cannot be the original Source path.".to_owned()
        }
        OutputPathError::UnsupportedExtension => {
            "Production conversion output must use .tif or .tiff.".to_owned()
        }
        OutputPathError::MissingFileName => {
            "Production destination must include a valid file name.".to_owned()
        }
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

fn save_gate_label(gate: ConversionSaveGate) -> &'static str {
    match gate {
        ConversionSaveGate::Ready => "Saved / reproducible",
        ConversionSaveGate::NoSourceFaces => "No source Face",
        ConversionSaveGate::SaveAsRequired => "Save As required",
        ConversionSaveGate::SaveRequired => "Save required",
    }
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
    use super::*;

    #[test]
    fn recommended_output_uses_production_subfolder_and_target_suffix() {
        let path =
            recommended_output_path(Path::new(r"C:\Design\Face01.tif"), "Durst 7C", 7).unwrap();
        assert_eq!(
            path,
            PathBuf::from(r"C:\Design\Production\Face01_Durst_7C.tif")
        );
    }

    #[test]
    fn conversion_window_layout_stays_scrollable_and_foldable() {
        let source = include_str!("color_conversion.rs");
        for required in [
            "production-color-conversion-scroll",
            "conversion-source-information-preflight",
            "conversion-source-preflight-foldout",
            "conversion-target-setup-foldout",
            "conversion-production-destination-foldout",
            "conversion-recipe-review-foldout",
            "conversion-queue-foldout",
            ".num_columns(4)",
        ] {
            assert!(source.contains(required), "missing compact conversion layout token: {required}");
        }
    }

    #[test]
    fn production_execution_support_matrix_keeps_non_rgb_non_tiff_fail_closed() {
        assert!(production_execution_supported(
            SourceImageFormat::Tiff,
            DesignSourceColorModel::Rgb
        ));
        assert!(production_execution_supported(
            SourceImageFormat::Tiff,
            DesignSourceColorModel::Cmyk
        ));
        assert!(production_execution_supported(
            SourceImageFormat::Png,
            DesignSourceColorModel::Rgb
        ));
        assert!(production_execution_supported(
            SourceImageFormat::Jpeg,
            DesignSourceColorModel::Rgb
        ));
        assert!(!production_execution_supported(
            SourceImageFormat::Png,
            DesignSourceColorModel::Gray
        ));
        assert!(!production_execution_supported(
            SourceImageFormat::Jpeg,
            DesignSourceColorModel::Gray
        ));
        assert!(!production_execution_supported(
            SourceImageFormat::Png,
            DesignSourceColorModel::Cmyk
        ));
    }
}
