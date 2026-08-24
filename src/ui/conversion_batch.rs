use crate::*;
use eframe::egui;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use windows_shade_editor::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use windows_shade_editor::conversion_batch::{
    ConversionBatchCapture, ConversionBatchFaceCapture, ConversionBatchScope,
};
use windows_shade_editor::conversion_batch_queue::{
    ConversionBatchQueue, ConversionBatchQueueCompletion, ConversionBatchQueueCompletionResult,
    ConversionBatchQueueItem, ConversionBatchQueueStatus,
};
use windows_shade_editor::conversion_output::{
    default_converted_filename, next_versioned_output_path, validate_conversion_output_path,
};
use windows_shade_editor::conversion_preflight::{
    ConversionPreflightReport, PreflightSeverity, SourceProfileState,
    build_conversion_preflight_for_source_with_policy,
};
use windows_shade_editor::conversion_transaction::{
    CapturedOutputPolicy, CapturedSourceProfile, ConversionJobCapture,
};
use windows_shade_editor::conversion_workflow::{
    ConversionSaveGate, ConversionSourceState, conversion_save_gate,
};
use windows_shade_editor::design_source::{
    DesignSourceColorModel, SourceImageFormat, TransparencyState,
};
use windows_shade_editor::icc_profile_registry::IccProfileRegistry;
use windows_shade_editor::model::IccProfileIdentity as ConversionIccProfileIdentity;
use windows_shade_editor::production_destination::{
    ProductionDestinationAvailability, ProductionDestinationCandidate,
    inspect_linked_production_destinations,
};
use windows_shade_editor::production_destination_selection::FrozenProductionDestination;
use windows_shade_editor::production_profile_catalog::verify_production_profile_candidate;
use windows_shade_editor::production_project_disposition::ProductionProjectDisposition;
use windows_shade_editor::production_target::{
    ProductionTargetProfileInspection, validate_target_channel_names,
    verify_production_target_profile,
};
use windows_shade_editor::source_transparency::SourceTransparencyPolicy;
use windows_shade_editor::tiff_io::ColorModel as ConversionColorModel;

thread_local! {
    static BATCH_CONTROLLER: RefCell<ConversionBatchUiController> =
        RefCell::new(ConversionBatchUiController::load());
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BatchDestinationMode {
    #[default]
    CreateNew,
    AppendExisting,
}

#[derive(Clone)]
struct ConversionBatchUiConfig {
    scope: ConversionBatchScope,
    selected_faces: BTreeSet<usize>,
    transparency_policies: BTreeMap<usize, SourceTransparencyPolicy>,
    engine_mode: ConversionEngineMode,
    target_profile: Option<ProductionTargetProfileInspection>,
    target_name: String,
    channel_names: Vec<String>,
    channel_names_confirmed: bool,
    output_bit_depth: u8,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
    output_folder: Option<PathBuf>,
    destination_mode: BatchDestinationMode,
    selected_existing: Option<PathBuf>,
}

impl Default for ConversionBatchUiConfig {
    fn default() -> Self {
        Self {
            scope: ConversionBatchScope::AllFaces,
            selected_faces: BTreeSet::new(),
            transparency_policies: BTreeMap::new(),
            engine_mode: ConversionEngineMode::Icc,
            target_profile: None,
            target_name: String::new(),
            channel_names: Vec::new(),
            channel_names_confirmed: false,
            output_bit_depth: 16,
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            output_folder: None,
            destination_mode: BatchDestinationMode::CreateNew,
            selected_existing: None,
        }
    }
}

struct ConversionBatchUiController {
    open: bool,
    config: ConversionBatchUiConfig,
    queue: ConversionBatchQueue,
    startup_error: Option<String>,
    owns_queue_exclusion: bool,
    export_was_paused: bool,
    conversion_was_paused: bool,
}

impl ConversionBatchUiController {
    fn load() -> Self {
        let (queue, startup_error) = match ConversionBatchQueue::load_persistent() {
            Ok(queue) => (queue, None),
            Err(error) => (ConversionBatchQueue::new(), Some(error)),
        };
        Self {
            open: false,
            config: ConversionBatchUiConfig::default(),
            queue,
            startup_error,
            owns_queue_exclusion: false,
            export_was_paused: false,
            conversion_was_paused: false,
        }
    }
}

#[derive(Clone)]
struct BatchFaceInspection {
    index: usize,
    label: String,
    source_path: PathBuf,
    source_model: RuntimeColorModel,
    source_format: SourceImageFormat,
    bit_depth: u8,
    transparency: TransparencyState,
    profile_identity: Option<ConversionIccProfileIdentity>,
    captured_profile: CapturedSourceProfile,
    profile_label: String,
    execution_supported: bool,
    report: ConversionPreflightReport,
    error: Option<String>,
}

impl BatchFaceInspection {
    fn ready(&self) -> bool {
        self.error.is_none() && self.execution_supported && self.report.can_convert()
    }
}

#[derive(Clone)]
struct BatchPlanPreview {
    production_project_path: PathBuf,
    disposition: ProductionProjectDisposition,
    output_paths: Vec<PathBuf>,
    recipes: Vec<ConversionRecipe>,
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
    /// Poll the durable multi-Face queue every frame, even when its window is closed.
    /// The queue takes exclusive ownership of Export/single-conversion scheduling while it has
    /// runnable or recovery work, so one Production project remains a single-writer resource.
    pub(crate) fn ui_conversion_batch_status(&mut self, ui: &mut egui::Ui) {
        self.poll_conversion_batch_queue();

        let (active_count, needs_recovery, startup_error) = BATCH_CONTROLLER.with(|cell| {
            let controller = cell.borrow();
            let active_count = controller
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
                .count();
            let needs_recovery = controller
                .queue
                .items()
                .iter()
                .any(|item| item.status == ConversionBatchQueueStatus::NeedsRecovery);
            (active_count, needs_recovery, controller.startup_error.clone())
        });

        let can_configure = !self.project.faces.is_empty();
        if can_configure || active_count > 0 || startup_error.is_some() {
            ui.separator();
            let label = if active_count > 0 {
                format!("Batch Convert ({active_count})")
            } else {
                "Batch Convert".to_owned()
            };
            if ui.small_button(label).clicked() {
                BATCH_CONTROLLER.with(|cell| {
                    let mut controller = cell.borrow_mut();
                    controller.open = true;
                    if controller.config.output_folder.is_none() {
                        controller.config.output_folder = default_batch_output_folder(
                            self.project_path.as_deref(),
                            self.faces.get(self.current_face).map(|face| face.path.as_path()),
                        );
                    }
                    if controller.config.selected_faces.is_empty()
                        && self.current_face < self.project.faces.len()
                    {
                        controller.config.selected_faces.insert(self.current_face);
                    }
                });
            }
            if needs_recovery {
                ui.label(
                    egui::RichText::new("batch recovery required")
                        .color(egui::Color32::YELLOW)
                        .small(),
                );
            }
        }
    }

    pub(crate) fn ui_conversion_batch_window(&mut self, ctx: &egui::Context) {
        let (mut open, mut config, queue_rows, queue_paused, recovered_waiting, startup_error) =
            BATCH_CONTROLLER.with(|cell| {
                let controller = cell.borrow();
                (
                    controller.open,
                    controller.config.clone(),
                    controller.queue.items().to_vec(),
                    controller.queue.is_paused(),
                    controller.queue.recovered_waiting_count(),
                    controller.startup_error.clone(),
                )
            });
        if !open {
            return;
        }

        if config.output_folder.is_none() {
            config.output_folder = default_batch_output_folder(
                self.project_path.as_deref(),
                self.faces.get(self.current_face).map(|face| face.path.as_path()),
            );
        }
        if config.selected_faces.is_empty() && self.current_face < self.project.faces.len() {
            config.selected_faces.insert(self.current_face);
        }

        let source_face_count = self.project.faces.len();
        let indices = scope_indices(
            config.scope,
            self.current_face,
            source_face_count,
            &config.selected_faces,
        );
        let inspections = indices
            .iter()
            .map(|index| inspect_batch_face(self, *index, &config))
            .collect::<Vec<_>>();
        let production_candidates = batch_production_candidates(self);
        let plan = build_batch_plan_preview(self, &config, &inspections, &production_candidates);

        let mut select_target_profile = false;
        let mut select_output_folder = false;
        let mut queue_batch = false;
        let mut seed_existing: Option<PathBuf> = None;
        let mut queue_actions = Vec::new();

        egui::Window::new("Multi-Face Production Conversion")
            .id(egui::Id::new("multi-face-production-conversion-window"))
            .open(&mut open)
            .resizable(true)
            .default_size([920.0, 780.0])
            .min_width(720.0)
            .show(ctx, |ui| {
                ui.heading("Multi-Face Production Conversion");
                ui.label(
                    "Capture Current / Selected / All Source Faces into one deterministic Production project.",
                );
                ui.small(
                    "One target/engine/separation policy is frozen for the whole batch. Source ICC identity and transparency policy remain per-Face inputs. Successful Faces are checkpointed before the next Face starts.",
                );

                if let Some(error) = startup_error.as_deref() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("Batch queue recovery warning: {error}"))
                            .color(egui::Color32::LIGHT_RED),
                    );
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading("1. Conversion scope & per-Face preflight");
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut config.scope,
                        ConversionBatchScope::CurrentFace,
                        "Current Face",
                    );
                    ui.selectable_value(
                        &mut config.scope,
                        ConversionBatchScope::SelectedFaces,
                        "Selected Faces",
                    );
                    ui.selectable_value(
                        &mut config.scope,
                        ConversionBatchScope::AllFaces,
                        "All Faces",
                    );
                    ui.label(format!("{} of {} Face(s)", indices.len(), source_face_count));
                });

                if config.scope == ConversionBatchScope::SelectedFaces {
                    ui.group(|ui| {
                        ui.strong("Select Source Faces");
                        for index in 0..source_face_count {
                            let label = self
                                .project
                                .faces
                                .get(index)
                                .map(|face| face.label.clone())
                                .filter(|value| !value.trim().is_empty())
                                .unwrap_or_else(|| format!("Face {}", index + 1));
                            let mut selected = config.selected_faces.contains(&index);
                            if ui
                                .checkbox(&mut selected, format!("{} — {}", index + 1, label))
                                .changed()
                            {
                                if selected {
                                    config.selected_faces.insert(index);
                                } else {
                                    config.selected_faces.remove(&index);
                                }
                            }
                        }
                    });
                }

                if indices.is_empty() {
                    ui.label(
                        egui::RichText::new("Select at least one Face.")
                            .color(egui::Color32::LIGHT_RED),
                    );
                }
                for inspection in &inspections {
                    render_batch_face_preflight(ui, inspection, &mut config);
                }
                render_batch_source_profile_consistency(ui, &inspections);

                ui.add_space(8.0);
                ui.separator();
                ui.heading("2. Shared target definition");
                let previous_engine = config.engine_mode;
                egui::ComboBox::from_label("Conversion engine")
                    .selected_text(engine_label(config.engine_mode))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut config.engine_mode,
                            ConversionEngineMode::Icc,
                            "Standard Output ICC",
                        );
                        ui.selectable_value(
                            &mut config.engine_mode,
                            ConversionEngineMode::DeviceLink,
                            "DeviceLink",
                        );
                    });
                if previous_engine != config.engine_mode {
                    config.target_profile = None;
                    config.target_name.clear();
                    config.channel_names.clear();
                    config.channel_names_confirmed = false;
                    config.selected_existing = None;
                    config.black_point_compensation =
                        config.engine_mode == ConversionEngineMode::Icc;
                }

                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(match config.engine_mode {
                            ConversionEngineMode::Icc => "Select Output ICC...",
                            ConversionEngineMode::DeviceLink => "Select DeviceLink...",
                            ConversionEngineMode::CustomOptimizer => "Select target...",
                        })
                        .clicked()
                    {
                        select_target_profile = true;
                    }
                    if let Some(profile) = config.target_profile.as_ref() {
                        ui.label(format!(
                            "{} · {} channel(s)",
                            profile.identity.description, profile.output_channel_count
                        ));
                    }
                });
                if config.target_profile.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Target name");
                        ui.text_edit_singleline(&mut config.target_name);
                        ui.label("Output bit depth");
                        ui.selectable_value(&mut config.output_bit_depth, 8, "8-bit");
                        ui.selectable_value(&mut config.output_bit_depth, 16, "16-bit");
                    });
                    if let Some(profile) = config.target_profile.as_ref() {
                        if profile.channel_names_authoritative {
                            ui.small(format!(
                                "Channel order: {}",
                                config.channel_names.join(" / ")
                            ));
                            config.channel_names_confirmed = true;
                        } else {
                            ui.label(
                                egui::RichText::new(
                                    "Profile does not expose authoritative colorant names. Confirm the real output order.",
                                )
                                .color(egui::Color32::YELLOW),
                            );
                            for (index, name) in config.channel_names.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("Channel {}", index + 1));
                                    ui.text_edit_singleline(name);
                                });
                            }
                            ui.checkbox(
                                &mut config.channel_names_confirmed,
                                "I confirm this production channel order",
                            );
                        }
                    }
                    if config.engine_mode == ConversionEngineMode::Icc {
                        egui::ComboBox::from_label("Rendering intent")
                            .selected_text(intent_label(config.rendering_intent))
                            .show_ui(ui, |ui| {
                                for intent in [
                                    ConversionRenderingIntent::Perceptual,
                                    ConversionRenderingIntent::RelativeColorimetric,
                                    ConversionRenderingIntent::Saturation,
                                    ConversionRenderingIntent::AbsoluteColorimetric,
                                ] {
                                    ui.selectable_value(
                                        &mut config.rendering_intent,
                                        intent,
                                        intent_label(intent),
                                    );
                                }
                            });
                        ui.checkbox(
                            &mut config.black_point_compensation,
                            "Black Point Compensation",
                        );
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading("3. Production destination");
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut config.destination_mode,
                        BatchDestinationMode::CreateNew,
                        "Create new Production project",
                    );
                    ui.selectable_value(
                        &mut config.destination_mode,
                        BatchDestinationMode::AppendExisting,
                        "Add to compatible Production project",
                    );
                });

                match config.destination_mode {
                    BatchDestinationMode::CreateNew => {
                        ui.small(
                            "The first committed Face creates one Production .shade; every later Face appends through a fresh compatibility/SHA check.",
                        );
                    }
                    BatchDestinationMode::AppendExisting => {
                        if production_candidates.is_empty() {
                            ui.label(
                                egui::RichText::new("No linked Production projects found.")
                                    .color(egui::Color32::YELLOW),
                            );
                        }
                        for candidate in &production_candidates {
                            let selected = config.selected_existing.as_deref()
                                == Some(candidate.path.as_path());
                            let title = candidate
                                .project_name
                                .as_deref()
                                .unwrap_or("Linked Production project");
                            let status = destination_status_label(candidate.availability);
                            ui.group(|ui| {
                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .add_enabled(
                                            candidate.can_append(),
                                            egui::Button::selectable(
                                                selected,
                                                format!("{title} · {status}"),
                                            ),
                                        )
                                        .clicked()
                                    {
                                        seed_existing = Some(candidate.path.clone());
                                    }
                                    if let Some(count) = candidate.face_count {
                                        ui.small(format!("{count} existing Face(s)"));
                                    }
                                });
                                ui.small(candidate.path.display().to_string());
                                if let Some(error) = candidate.diagnostic.as_deref() {
                                    ui.label(
                                        egui::RichText::new(error)
                                            .color(egui::Color32::YELLOW),
                                    );
                                }
                            });
                        }
                    }
                }

                ui.horizontal_wrapped(|ui| {
                    ui.label("Batch TIFF folder");
                    ui.label(
                        config
                            .output_folder
                            .as_deref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "Not selected".to_owned()),
                    );
                    if ui.button("Choose folder...").clicked() {
                        select_output_folder = true;
                    }
                });
                ui.small(
                    "Batch outputs are always new/versioned TIFFs. Individual stale Face re-conversion and explicit transactional replacement remain available through the existing single-Face converter.",
                );

                ui.add_space(8.0);
                ui.separator();
                ui.heading("4. Frozen batch review");
                match &plan {
                    Ok(plan) => {
                        ui.label(
                            egui::RichText::new(format!(
                                "Ready: {} Face(s) → one Production project",
                                inspections.len()
                            ))
                            .color(egui::Color32::LIGHT_GREEN)
                            .strong(),
                        );
                        ui.small(format!(
                            "Production project: {}",
                            plan.production_project_path.display()
                        ));
                        for (inspection, output) in inspections.iter().zip(&plan.output_paths) {
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
                                    "Queue {} Face Production Batch",
                                    inspections.len()
                                )),
                            )
                            .on_hover_text(
                                "Hash the one saved Source state and every selected Face, then persist one deterministic batch capture.",
                            )
                            .clicked()
                        {
                            queue_batch = true;
                        }
                    }
                    Err(errors) => {
                        for error in errors {
                            ui.label(
                                egui::RichText::new(format!("• {error}"))
                                    .color(egui::Color32::LIGHT_RED),
                            );
                        }
                        ui.add_enabled(false, egui::Button::new("Queue Production Batch"));
                    }
                }

                render_batch_queue(
                    ui,
                    &queue_rows,
                    queue_paused,
                    recovered_waiting,
                    &mut queue_actions,
                );
            });

        BATCH_CONTROLLER.with(|cell| {
            let mut controller = cell.borrow_mut();
            controller.open = open;
            controller.config = config.clone();
        });

        if select_output_folder {
            let mut dialog = rfd::FileDialog::new().set_title("Select Production Batch TIFF Folder");
            if let Some(folder) = config.output_folder.as_deref() {
                dialog = dialog.set_directory(folder);
            }
            if let Some(folder) = dialog.pick_folder() {
                config.output_folder = Some(folder);
                BATCH_CONTROLLER.with(|cell| cell.borrow_mut().config = config.clone());
            }
        }

        if select_target_profile {
            let first_model = inspections.first().map(|inspection| inspection.source_model);
            if let Some(source_model) = first_model {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("ICC / DeviceLink profiles", &["icc", "icm"])
                    .set_title(match config.engine_mode {
                        ConversionEngineMode::Icc => "Select Production Output ICC",
                        ConversionEngineMode::DeviceLink => "Select Production DeviceLink",
                        ConversionEngineMode::CustomOptimizer => "Select target",
                    })
                    .pick_file()
                {
                    match windows_shade_editor::production_target::inspect_production_target_profile(
                        &path,
                        config.engine_mode,
                        conversion_color_model(source_model),
                    ) {
                        Ok(profile) => {
                            config.target_name = profile.identity.description.clone();
                            config.channel_names = profile.channel_names.clone();
                            config.channel_names_confirmed = profile.channel_names_authoritative;
                            config.target_profile = Some(profile);
                            config.selected_existing = None;
                            BATCH_CONTROLLER.with(|cell| {
                                cell.borrow_mut().config = config.clone();
                            });
                            self.report_info("Selected shared production target for batch conversion");
                        }
                        Err(error) => self.report_error(error),
                    }
                }
            } else {
                self.report_error("Select at least one readable Source Face before choosing a target.");
            }
        }

        if let Some(path) = seed_existing {
            let candidate = production_candidates
                .iter()
                .find(|candidate| candidate.path == path)
                .cloned();
            let first_model = inspections.first().map(|inspection| inspection.source_model);
            match (candidate, first_model) {
                (Some(candidate), Some(source_model)) => {
                    match seed_batch_target_from_existing(&mut config, &candidate, source_model) {
                        Ok(()) => {
                            BATCH_CONTROLLER.with(|cell| {
                                cell.borrow_mut().config = config.clone();
                            });
                        }
                        Err(error) => self.report_error(error),
                    }
                }
                _ => self.report_error("Selected Production project is no longer available."),
            }
        }

        for action in queue_actions {
            self.dispatch_conversion_batch_queue_action(action);
        }

        if queue_batch {
            match self.capture_and_enqueue_conversion_batch(&config, &inspections, &production_candidates)
            {
                Ok(id) => self.report_info(format!(
                    "Queued multi-Face production batch #{id} with {} Face(s)",
                    inspections.len()
                )),
                Err(error) => self.report_error(error),
            }
        }
    }

    fn poll_conversion_batch_queue(&mut self) {
        let (owns_exclusion, has_recovery) = BATCH_CONTROLLER.with(|cell| {
            let controller = cell.borrow();
            (
                controller.owns_queue_exclusion,
                controller
                    .queue
                    .items()
                    .iter()
                    .any(|item| item.status == ConversionBatchQueueStatus::NeedsRecovery),
            )
        });

        let allow_start = self.job.is_none()
            && !self.export.queue.is_active()
            && !self.conversion_queue.is_active()
            && (owns_exclusion
                || (!self.export.queue.has_pending() && !self.conversion_queue.has_pending()));

        let (completions, source_paths, persistence_error, should_block, currently_owns) =
            BATCH_CONTROLLER.with(|cell| {
                let mut controller = cell.borrow_mut();
                let source_paths = controller
                    .queue
                    .items()
                    .iter()
                    .map(|item| (item.id, item.source_project_path.clone()))
                    .collect::<BTreeMap<_, _>>();
                let completions = controller.queue.poll_with_start(allow_start);
                let persistence_error = controller.queue.take_persistence_error();
                let should_block = batch_queue_blocks_other_work(&controller.queue);
                (
                    completions,
                    source_paths,
                    persistence_error,
                    should_block,
                    controller.owns_queue_exclusion,
                )
            });

        if let Some(error) = persistence_error {
            self.log
                .error(&format!("Conversion Batch Queue persistence: {error}"));
        }

        if should_block && !currently_owns {
            let export_was_paused = self.export.queue.is_paused();
            let conversion_was_paused = self.conversion_queue.is_paused();
            self.export.queue.set_paused(true);
            self.conversion_queue.set_paused(true);
            BATCH_CONTROLLER.with(|cell| {
                let mut controller = cell.borrow_mut();
                controller.owns_queue_exclusion = true;
                controller.export_was_paused = export_was_paused;
                controller.conversion_was_paused = conversion_was_paused;
            });
        } else if !should_block && currently_owns && !has_recovery {
            let (export_was_paused, conversion_was_paused) = BATCH_CONTROLLER.with(|cell| {
                let mut controller = cell.borrow_mut();
                controller.owns_queue_exclusion = false;
                (
                    controller.export_was_paused,
                    controller.conversion_was_paused,
                )
            });
            self.export.queue.set_paused(export_was_paused);
            self.conversion_queue.set_paused(conversion_was_paused);
        }

        for completion in completions {
            let source_path = source_paths.get(&completion.id).cloned();
            self.handle_conversion_batch_completion(completion, source_path.as_deref());
        }
    }

    fn dispatch_conversion_batch_queue_action(&mut self, action: BatchQueueUiAction) {
        match action {
            BatchQueueUiAction::ResumeRecovered => {
                let count = BATCH_CONTROLLER.with(|cell| {
                    cell.borrow_mut().queue.resume_recovered()
                });
                self.report_info(format!("Resumed {count} recovered conversion batch(es)"));
            }
            BatchQueueUiAction::TogglePaused => {
                BATCH_CONTROLLER.with(|cell| {
                    let mut controller = cell.borrow_mut();
                    let paused = controller.queue.is_paused();
                    controller.queue.set_paused(!paused);
                });
            }
            BatchQueueUiAction::Cancel(id) => {
                BATCH_CONTROLLER.with(|cell| {
                    cell.borrow_mut().queue.cancel(id);
                });
            }
            BatchQueueUiAction::Retry(id) => {
                BATCH_CONTROLLER.with(|cell| {
                    cell.borrow_mut().queue.retry(id);
                });
            }
            BatchQueueUiAction::Recover(id) => {
                let (source_path, result) = BATCH_CONTROLLER.with(|cell| {
                    let mut controller = cell.borrow_mut();
                    let source_path = controller
                        .queue
                        .items()
                        .iter()
                        .find(|item| item.id == id)
                        .map(|item| item.source_project_path.clone());
                    (source_path, controller.queue.recover(id))
                });
                match result {
                    Ok(completion) => {
                        self.handle_conversion_batch_completion(
                            completion,
                            source_path.as_deref(),
                        );
                    }
                    Err(error) => self.report_error(format!(
                        "Production batch project-only recovery blocked: {error}"
                    )),
                }
            }
            BatchQueueUiAction::ClearFinished => {
                BATCH_CONTROLLER.with(|cell| {
                    cell.borrow_mut().queue.clear_finished();
                });
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
                    match production_project::link_source_project_to_production(
                        &mut self.project,
                        &completed.production_project_path,
                    ) {
                        Ok(()) => {
                            self.mark_project_dirty();
                            self.log.info(
                                "Multi-Face Production linkage changed the open Source project; explicit Save is required.",
                            );
                        }
                        Err(error) => self.log.error(&format!(
                            "Could not mirror batch Production link in the open Source project: {error}"
                        )),
                    }
                }
                if batch_complete {
                    self.report_info(format!(
                        "Production batch #{} complete: {}",
                        completion.id,
                        completed.production_project_path.display()
                    ));
                } else {
                    self.report_info(format!(
                        "Batch #{} Face {} committed (checkpoint {}); continuing",
                        completion.id,
                        completion.source_face_index + 1,
                        ordinal + 1
                    ));
                }
            }
            ConversionBatchQueueCompletionResult::Cancelled { phase, message } => {
                self.report_info(format!(
                    "Production batch #{} cancelled at {phase}: {message}",
                    completion.id
                ));
            }
            ConversionBatchQueueCompletionResult::Failed { phase, error } => {
                self.report_error(format!(
                    "Production batch #{} Face {} failed at {phase}: {error}",
                    completion.id,
                    completion.source_face_index + 1
                ));
            }
            ConversionBatchQueueCompletionResult::NeedsRecovery(recovery) => {
                self.report_error(format!(
                    "Production batch #{} Face {} committed its TIFF but needs project-only recovery before continuing: {}",
                    completion.id,
                    recovery.source_face_index + 1,
                    recovery.recovery.error
                ));
                BATCH_CONTROLLER.with(|cell| cell.borrow_mut().open = true);
            }
        }
    }

    fn capture_and_enqueue_conversion_batch(
        &mut self,
        config: &ConversionBatchUiConfig,
        inspections: &[BatchFaceInspection],
        candidates: &[ProductionDestinationCandidate],
    ) -> Result<u64, String> {
        if self.job.is_some() {
            return Err("Finish the current foreground operation before queueing a batch.".to_owned());
        }
        let source_project_path = self
            .project_path
            .clone()
            .ok_or_else(|| "Save the Source project before queueing a batch.".to_owned())?;
        if self.project_dirty {
            return Err("Save the Source project before queueing a batch.".to_owned());
        }
        if inspections.is_empty() {
            return Err("Select at least one Face for batch conversion.".to_owned());
        }
        if let Some(face) = inspections.iter().find(|face| !face.ready()) {
            return Err(format!(
                "Face {} ('{}') is not ready for production conversion.",
                face.index + 1,
                face.label
            ));
        }

        let plan = build_batch_plan_preview(self, config, inspections, candidates)
            .map_err(|errors| errors.join(" "))?;
        let output_folder = config
            .output_folder
            .as_deref()
            .ok_or_else(|| "Choose a batch TIFF folder.".to_owned())?;
        std::fs::create_dir_all(output_folder).map_err(|error| {
            format!(
                "Cannot create Production batch folder {}: {error}",
                output_folder.display()
            )
        })?;

        let captured_project: windows_shade_editor::model::ShadeProject =
            serde_json::from_value(serde_json::to_value(&self.project).map_err(|error| {
                format!("Cannot serialize Source project for batch capture: {error}")
            })?)
            .map_err(|error| format!("Cannot materialize Source project batch capture: {error}"))?;

        let source_project_sha_before =
            windows_shade_editor::icc_conversion_worker::sha256_file(&source_project_path)?;
        let production_project_name = format!(
            "{} - {}",
            self.project.name,
            config.target_name.trim()
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
                CapturedOutputPolicy::MustNotExist,
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
                "Source project changed while the multi-Face batch was being captured. Save it and queue again."
                    .to_owned(),
            );
        }

        let batch = ConversionBatchCapture::capture(
            config.scope,
            self.project.faces.len(),
            plan.disposition,
            face_captures,
        )?;
        let default_dpi = self.settings.default_dpi;
        BATCH_CONTROLLER.with(|cell| cell.borrow_mut().queue.enqueue(batch, default_dpi))
    }
}

fn inspect_batch_face(
    app: &ShadeApp,
    index: usize,
    config: &ConversionBatchUiConfig,
) -> BatchFaceInspection {
    let label = app
        .project
        .faces
        .get(index)
        .map(|face| face.label.clone())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| format!("Face {}", index + 1));
    let Some(runtime) = app.faces.get(index) else {
        return unavailable_face(index, label, PathBuf::new(), "Runtime Face is unavailable.");
    };
    if !runtime.available {
        return unavailable_face(
            index,
            label,
            runtime.path.clone(),
            "Source Face file is missing or unreadable. Relink it before conversion.",
        );
    }
    let Some(owned_descriptor) = runtime.preview.source_descriptor() else {
        return unavailable_face(
            index,
            label,
            runtime.path.clone(),
            "Source descriptor is unavailable for production preflight.",
        );
    };
    let descriptor = owned_descriptor.as_borrowed();
    let source_model = runtime.preview.color_model();
    let save_gate = conversion_save_gate(ConversionSourceState {
        has_faces: !app.faces.is_empty(),
        has_saved_project_path: app.project_path.is_some(),
        has_unsaved_changes: app.project_dirty,
    });
    let face_ref = app.project.faces.get(index);
    let (profile_state, captured_profile, profile_label) =
        batch_source_profile_state(&descriptor, source_model, face_ref);
    let profile_identity = profile_state.identity().cloned();
    let transparency_policy = config.transparency_policies.get(&index);
    let report = build_conversion_preflight_for_source_with_policy(
        &descriptor,
        profile_state,
        save_gate,
        transparency_policy,
    );

    BatchFaceInspection {
        index,
        label,
        source_path: runtime.path.clone(),
        source_model,
        source_format: descriptor.format,
        bit_depth: descriptor.bit_depth,
        transparency: descriptor.transparency,
        profile_identity,
        captured_profile,
        profile_label,
        execution_supported: batch_execution_supported(descriptor.format, descriptor.color_model),
        report,
        error: None,
    }
}

fn unavailable_face(
    index: usize,
    label: String,
    source_path: PathBuf,
    message: &str,
) -> BatchFaceInspection {
    BatchFaceInspection {
        index,
        label,
        source_path,
        source_model: RuntimeColorModel::Other,
        source_format: SourceImageFormat::Tiff,
        bit_depth: 0,
        transparency: TransparencyState::None,
        profile_identity: None,
        captured_profile: CapturedSourceProfile::Embedded,
        profile_label: "Unavailable".to_owned(),
        execution_supported: false,
        report: ConversionPreflightReport::default(),
        error: Some(message.to_owned()),
    }
}

fn batch_source_profile_state(
    descriptor: &windows_shade_editor::design_source::DesignSourceDescriptor<'_>,
    source_model: RuntimeColorModel,
    face: Option<&model::FaceRef>,
) -> (SourceProfileState, CapturedSourceProfile, String) {
    if let Some(assignment) = face.and_then(|face| face.production_source_profile.as_ref()) {
        let path = PathBuf::from(&assignment.path);
        let expected = ConversionIccProfileIdentity {
            description: assignment.identity.description.clone(),
            sha256: assignment.identity.sha256.clone(),
        };
        return match IccProfileRegistry.verify_identity(&path, &expected) {
            Ok(profile)
                if profile.compatible_with_source_model(conversion_color_model(source_model)) =>
            {
                (
                    SourceProfileState::Assigned(profile.identity.clone()),
                    CapturedSourceProfile::External { path },
                    format!("Assigned: {}", profile.description),
                )
            }
            Ok(profile) => (
                SourceProfileState::Invalid(format!(
                    "Assigned production Source ICC '{}' declares {} but Face is {}.",
                    profile.description,
                    profile.color_space_label(),
                    source_model.title(),
                )),
                CapturedSourceProfile::External { path },
                format!("Invalid assigned ICC: {}", assignment.identity.description),
            ),
            Err(error) => (
                SourceProfileState::Invalid(error),
                CapturedSourceProfile::External { path },
                format!("Invalid assigned ICC: {}", assignment.identity.description),
            ),
        };
    }

    match color_management::production_source_profile_identity_or_rgb_fallback_for_runtime(
        source_model,
        descriptor.embedded_icc,
    ) {
        Ok(Some(identity)) => {
            let identity = ConversionIccProfileIdentity {
                description: identity.description,
                sha256: identity.sha256,
            };
            let profile_label = if windows_shade_editor::source_profile_fallback::is_srgb_fallback_identity(&identity) {
                format!("No Source ICC · fallback: {}", identity.description)
            } else {
                format!("Embedded: {}", identity.description)
            };
            (
                SourceProfileState::Embedded(identity),
                CapturedSourceProfile::Embedded,
                profile_label,
            )
        }
        Ok(None) => (
            SourceProfileState::Missing,
            CapturedSourceProfile::Embedded,
            "Missing production Source ICC".to_owned(),
        ),
        Err(error) => (
            SourceProfileState::Invalid(error),
            CapturedSourceProfile::Embedded,
            "Invalid embedded production Source ICC".to_owned(),
        ),
    }
}

fn render_batch_face_preflight(
    ui: &mut egui::Ui,
    inspection: &BatchFaceInspection,
    config: &mut ConversionBatchUiConfig,
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
        ui.small(inspection.source_path.display().to_string());
        ui.small(format!("Source ICC: {}", inspection.profile_label));

        if inspection.transparency == TransparencyState::PresentUnresolved {
            let mut flatten = config.transparency_policies.contains_key(&inspection.index);
            if ui
                .checkbox(
                    &mut flatten,
                    "Flatten this Face on solid white for production conversion",
                )
                .changed()
            {
                if flatten {
                    config.transparency_policies.insert(
                        inspection.index,
                        SourceTransparencyPolicy::FlattenSolidRgb16 {
                            background_rgb: [u16::MAX; 3],
                        },
                    );
                } else {
                    config.transparency_policies.remove(&inspection.index);
                }
            }
        }

        if let Some(error) = inspection.error.as_deref() {
            ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
        }
        if !inspection.execution_supported {
            ui.label(
                egui::RichText::new(
                    "Execution supports RGB TIFF/PNG/JPEG and CMYK TIFF Sources only.",
                )
                .color(egui::Color32::LIGHT_RED),
            );
        }
        for finding in &inspection.report.findings {
            let color = match finding.severity {
                PreflightSeverity::Info => egui::Color32::GRAY,
                PreflightSeverity::Warning => egui::Color32::YELLOW,
                PreflightSeverity::Blocking => egui::Color32::LIGHT_RED,
            };
            ui.small(egui::RichText::new(format!(
                "{}: {} — {}",
                severity_label(finding.severity),
                finding.title,
                finding.detail
            )).color(color));
        }
    });
}

fn render_batch_source_profile_consistency(
    ui: &mut egui::Ui,
    inspections: &[BatchFaceInspection],
) {
    let mut groups = BTreeMap::<String, (String, Vec<String>)>::new();
    for inspection in inspections {
        let Some(identity) = inspection.profile_identity.as_ref() else {
            continue;
        };
        let key = identity.sha256.trim().to_ascii_lowercase();
        let entry = groups
            .entry(key)
            .or_insert_with(|| (identity.description.clone(), Vec::new()));
        entry.1.push(format!("Face {} ({})", inspection.index + 1, inspection.label));
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
            "Batch conversion is allowed. Each Face keeps its own captured Source ICC or sRGB fallback; profiles are not forced to match the first Face.",
        );
        for (_hash, (description, faces)) in groups {
            ui.small(format!("{description}: {}", faces.join(", ")));
        }
    });
}

fn build_batch_plan_preview(
    app: &ShadeApp,
    config: &ConversionBatchUiConfig,
    inspections: &[BatchFaceInspection],
    candidates: &[ProductionDestinationCandidate],
) -> Result<BatchPlanPreview, Vec<String>> {
    let mut errors = Vec::new();
    if inspections.is_empty() {
        return Err(vec!["Select at least one Source Face.".to_owned()]);
    }
    for inspection in inspections {
        if !inspection.ready() {
            errors.push(format!(
                "Face {} ('{}') has blocking source preflight findings.",
                inspection.index + 1,
                inspection.label
            ));
        }
    }
    let Some(folder) = config.output_folder.as_deref() else {
        errors.push("Choose the Production batch TIFF folder.".to_owned());
        return Err(errors);
    };

    let mut recipes = Vec::with_capacity(inspections.len());
    for inspection in inspections {
        match build_batch_recipe(config, inspection) {
            Ok(recipe) => recipes.push(recipe),
            Err(error) => errors.push(format!(
                "Face {} ('{}'): {error}",
                inspection.index + 1,
                inspection.label
            )),
        }
    }
    if recipes.len() != inspections.len() {
        return Err(errors);
    }

    let output_paths = match batch_output_paths(folder, inspections, &config.target_name) {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };

    let frozen = match config.destination_mode {
        BatchDestinationMode::CreateNew => {
            let project_path = unique_production_project_path(
                folder,
                &format!("{} - {}", app.project.name, config.target_name.trim()),
            );
            FrozenProductionDestination::create_new(project_path)
        }
        BatchDestinationMode::AppendExisting => {
            let candidate = config
                .selected_existing
                .as_ref()
                .and_then(|path| candidates.iter().find(|candidate| candidate.path == *path))
                .ok_or_else(|| "Select a compatible linked Production project.".to_owned())
                .and_then(|candidate| {
                    FrozenProductionDestination::append_existing(candidate, &recipes[0])
                });
            candidate
        }
    };
    let frozen = match frozen {
        Ok(frozen) => Some(frozen),
        Err(error) => {
            errors.push(error);
            None
        }
    };

    if !errors.is_empty() {
        return Err(errors);
    }
    let frozen = frozen.expect("frozen destination exists when validation passes");
    Ok(BatchPlanPreview {
        production_project_path: frozen.production_project_path,
        disposition: frozen.disposition,
        output_paths,
        recipes,
    })
}

fn build_batch_recipe(
    config: &ConversionBatchUiConfig,
    inspection: &BatchFaceInspection,
) -> Result<ConversionRecipe, String> {
    let stored_profile = config
        .target_profile
        .as_ref()
        .ok_or_else(|| "Select a production Output ICC or DeviceLink.".to_owned())?;
    verify_production_profile_candidate(
        IccProfileRegistry,
        &stored_profile.path,
        &stored_profile.identity,
        config.engine_mode,
        conversion_color_model(inspection.source_model),
    )?;
    let verified = verify_production_target_profile(
        &stored_profile.path,
        &stored_profile.identity,
        config.engine_mode,
        conversion_color_model(inspection.source_model),
    )?;
    if config.target_name.trim().is_empty() {
        return Err("Target name cannot be empty.".to_owned());
    }
    validate_target_channel_names(&config.channel_names, verified.output_channel_count)?;
    if !verified.channel_names_authoritative && !config.channel_names_confirmed {
        return Err("Confirm the real production channel order.".to_owned());
    }
    if !matches!(config.output_bit_depth, 8 | 16) {
        return Err("Output bit depth must be 8 or 16.".to_owned());
    }
    let source_profile_identity = inspection
        .profile_identity
        .clone()
        .ok_or_else(|| "Source ICC identity is not ready.".to_owned())?;
    let profile_path = verified.path.to_string_lossy().into_owned();
    let profile_identity = verified.identity.clone();
    let (output_profile_path, output_profile_identity, device_link_path, device_link_identity) =
        match config.engine_mode {
            ConversionEngineMode::Icc => (Some(profile_path), Some(profile_identity), None, None),
            ConversionEngineMode::DeviceLink => {
                (None, None, Some(profile_path), Some(profile_identity))
            }
            ConversionEngineMode::CustomOptimizer => {
                return Err("Custom Optimizer batch execution is not enabled.".to_owned());
            }
        };
    let recipe = ConversionRecipe {
        source_transparency_policy: config
            .transparency_policies
            .get(&inspection.index)
            .copied(),
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: config.engine_mode,
        source_profile_identity,
        target: ConversionTargetDefinition {
            name: config.target_name.trim().to_owned(),
            channels: config
                .channel_names
                .iter()
                .map(|name| TargetChannelDefinition {
                    name: name.trim().to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: None,
                })
                .collect(),
            bit_depth: config.output_bit_depth,
            output_profile_identity,
            output_profile_path,
            device_link_identity,
            device_link_path,
            characterization_id: None,
            total_ink_limit: None,
        },
        rendering_intent: config.rendering_intent,
        black_point_compensation: config.engine_mode == ConversionEngineMode::Icc
            && config.black_point_compensation,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: None,
    };
    recipe
        .validate()
        .map_err(|errors| errors.join(" "))?;
    Ok(recipe)
}

fn seed_batch_target_from_existing(
    config: &mut ConversionBatchUiConfig,
    candidate: &ProductionDestinationCandidate,
    source_model: RuntimeColorModel,
) -> Result<(), String> {
    if !candidate.can_append() {
        return Err(candidate
            .diagnostic
            .clone()
            .unwrap_or_else(|| "Selected Production project is not append-compatible.".to_owned()));
    }
    let recipe = candidate
        .baseline_recipe
        .as_ref()
        .ok_or_else(|| "Selected Production project has no baseline recipe.".to_owned())?
        .clone();
    let (profile_path, identity) = match recipe.engine_mode {
        ConversionEngineMode::Icc => (
            recipe.target.output_profile_path.as_deref(),
            recipe.target.output_profile_identity.as_ref(),
        ),
        ConversionEngineMode::DeviceLink => (
            recipe.target.device_link_path.as_deref(),
            recipe.target.device_link_identity.as_ref(),
        ),
        ConversionEngineMode::CustomOptimizer => {
            return Err("Custom Optimizer batch append requires its dedicated engine.".to_owned());
        }
    };
    let profile_path = profile_path.ok_or_else(|| {
        "Selected Production recipe no longer contains its external target profile path.".to_owned()
    })?;
    let identity = identity.ok_or_else(|| {
        "Selected Production recipe no longer contains its target profile identity.".to_owned()
    })?;
    let verified = verify_production_target_profile(
        Path::new(profile_path),
        identity,
        recipe.engine_mode,
        conversion_color_model(source_model),
    )?;

    config.engine_mode = recipe.engine_mode;
    config.target_profile = Some(verified);
    config.target_name = recipe.target.name.clone();
    config.channel_names = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect();
    config.channel_names_confirmed = true;
    config.output_bit_depth = recipe.target.bit_depth;
    config.rendering_intent = recipe.rendering_intent;
    config.black_point_compensation = recipe.black_point_compensation;
    config.destination_mode = BatchDestinationMode::AppendExisting;
    config.selected_existing = Some(candidate.path.clone());
    if let Some(parent) = candidate.path.parent() {
        config.output_folder = Some(parent.to_path_buf());
    }
    Ok(())
}

fn batch_output_paths(
    folder: &Path,
    inspections: &[BatchFaceInspection],
    target_name: &str,
) -> Result<Vec<PathBuf>, String> {
    let suffix = if target_name.trim().is_empty() {
        "Production"
    } else {
        target_name.trim()
    };
    let mut reserved = BTreeSet::new();
    let mut outputs = Vec::with_capacity(inspections.len());
    for inspection in inspections {
        let filename = default_converted_filename(&inspection.source_path, suffix)
            .map_err(|error| format!("Cannot build output name: {error:?}"))?;
        let mut preferred = folder.join(filename);
        if reserved.contains(&path_key(&preferred)) {
            preferred = append_face_suffix(&preferred, inspection.index + 1);
        }
        let mut candidate = next_versioned_output_path(&preferred)
            .map_err(|error| format!("Cannot reserve versioned output: {error:?}"))?;
        if reserved.contains(&path_key(&candidate)) {
            candidate = next_versioned_output_path(&append_face_suffix(
                &preferred,
                inspection.index + 1,
            ))
            .map_err(|error| format!("Cannot disambiguate batch output: {error:?}"))?;
        }
        validate_conversion_output_path(&inspection.source_path, &candidate)
            .map_err(|error| format!("Unsafe batch output path: {error:?}"))?;
        let candidate = crate::tiff_output::canonical_destination(&candidate);
        if !reserved.insert(path_key(&candidate)) {
            return Err(format!(
                "Batch output collision remains after disambiguation: {}",
                candidate.display()
            ));
        }
        outputs.push(candidate);
    }
    Ok(outputs)
}

fn append_face_suffix(path: &Path, face_number: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Face".to_owned());
    let extension = path.extension().map(|ext| ext.to_os_string());
    let mut result = path.with_file_name(format!("{stem}_F{face_number:02}"));
    if let Some(extension) = extension {
        result.set_extension(extension);
    }
    result
}

fn unique_production_project_path(folder: &Path, label: &str) -> PathBuf {
    let base = safe_component(label, "Production");
    let first = folder.join(format!("{base}.shade"));
    if !first.exists() {
        return first;
    }
    for version in 2..=9999 {
        let candidate = folder.join(format!("{base}_v{version}.shade"));
        if !candidate.exists() {
            return candidate;
        }
    }
    folder.join(format!("{base}_{}.shade", std::process::id()))
}

fn safe_component(value: &str, fallback: &str) -> String {
    let mut output = String::new();
    let mut last_separator = false;
    for character in value.trim().chars() {
        let mapped = if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            character
        } else {
            '_'
        };
        if mapped == '_' {
            if last_separator {
                continue;
            }
            last_separator = true;
        } else {
            last_separator = false;
        }
        output.push(mapped);
        if output.len() >= 96 {
            break;
        }
    }
    let output = output.trim_matches('_').to_owned();
    if output.is_empty() {
        fallback.to_owned()
    } else {
        output
    }
}

fn render_batch_queue(
    ui: &mut egui::Ui,
    rows: &[ConversionBatchQueueItem],
    paused: bool,
    recovered_waiting: usize,
    actions: &mut Vec<BatchQueueUiAction>,
) {
    ui.add_space(10.0);
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.heading("Multi-Face Conversion Queue");
        if ui
            .button(if paused { "Resume batch queue" } else { "Pause batch queue" })
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
        ui.small("No multi-Face production batches queued.");
        return;
    }
    for row in rows {
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("#{} {}", row.id, row.label));
                ui.label(row.status.label());
                ui.small(format!(
                    "{} / {} Face(s) committed",
                    row.completed_face_count, row.face_count
                ));
                if row.requires_resume {
                    ui.label(
                        egui::RichText::new("Recovered / explicit resume")
                            .color(egui::Color32::YELLOW),
                    );
                }
            });
            ui.add(
                egui::ProgressBar::new(row.progress.clamp(0.0, 1.0))
                    .show_percentage()
                    .text(if row.phase.is_empty() {
                        row.detail.clone()
                    } else {
                        format!("{} — {}", row.phase, row.detail)
                    }),
            );
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

fn batch_production_candidates(app: &ShadeApp) -> Vec<ProductionDestinationCandidate> {
    let Some(source_project_path) = app.project_path.as_deref() else {
        return Vec::new();
    };
    let Ok(value) = serde_json::to_value(&app.project) else {
        return Vec::new();
    };
    let Ok(source_project) =
        serde_json::from_value::<windows_shade_editor::model::ShadeProject>(value)
    else {
        return Vec::new();
    };
    inspect_linked_production_destinations(&source_project, source_project_path)
}

fn default_batch_output_folder(
    project_path: Option<&Path>,
    face_path: Option<&Path>,
) -> Option<PathBuf> {
    project_path
        .and_then(Path::parent)
        .or_else(|| face_path.and_then(Path::parent))
        .map(|parent| parent.join("Production"))
}

fn scope_indices(
    scope: ConversionBatchScope,
    current_face: usize,
    source_face_count: usize,
    selected: &BTreeSet<usize>,
) -> Vec<usize> {
    match scope {
        ConversionBatchScope::CurrentFace => {
            (current_face < source_face_count).then_some(current_face).into_iter().collect()
        }
        ConversionBatchScope::SelectedFaces => selected
            .iter()
            .copied()
            .filter(|index| *index < source_face_count)
            .collect(),
        ConversionBatchScope::AllFaces => (0..source_face_count).collect(),
    }
}

fn batch_execution_supported(format: SourceImageFormat, model: DesignSourceColorModel) -> bool {
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
        PreflightSeverity::Info => "Info",
        PreflightSeverity::Warning => "Warning",
        PreflightSeverity::Blocking => "Blocking",
    }
}

fn engine_label(engine: ConversionEngineMode) -> &'static str {
    match engine {
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
    use super::*;

    #[test]
    fn selected_scope_is_deterministic_and_filters_stale_indices() {
        let selected = BTreeSet::from([5, 1, 3, 99]);
        assert_eq!(
            scope_indices(ConversionBatchScope::SelectedFaces, 0, 6, &selected),
            vec![1, 3, 5]
        );
        assert_eq!(
            scope_indices(ConversionBatchScope::AllFaces, 4, 4, &selected),
            vec![0, 1, 2, 3]
        );
        assert_eq!(
            scope_indices(ConversionBatchScope::CurrentFace, 2, 4, &selected),
            vec![2]
        );
    }

    #[test]
    fn production_project_component_is_windows_safe_and_bounded() {
        let component = safe_component("Source: A / Durst 7C * target?", "Production");
        assert_eq!(component, "Source_A_Durst_7C_target");
        assert!(component.len() <= 96);
        assert_eq!(safe_component("***", "Production"), "Production");
    }

    #[test]
    fn duplicate_output_name_gets_face_identity_suffix() {
        let path = Path::new(r"C:\Production\Face_Durst.tif");
        assert_eq!(
            append_face_suffix(path, 3),
            PathBuf::from(r"C:\Production\Face_Durst_F03.tif")
        );
    }
}