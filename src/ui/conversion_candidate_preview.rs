use crate::*;
use eframe::egui;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use windows_shade_editor::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use windows_shade_editor::conversion_candidate_preview::{
    CandidatePreviewInput, CandidatePreviewResult, render_candidate_preview,
};
use windows_shade_editor::conversion_output::{
    default_converted_filename, next_versioned_output_path, validate_conversion_output_path,
};
use windows_shade_editor::conversion_recipe::recipe_sha256;
use windows_shade_editor::conversion_transaction::{
    CapturedOutputPolicy, CapturedSourceProfile, ConversionCancellation, ConversionJobCapture,
};
use windows_shade_editor::icc_conversion::IccSourceModel;
use windows_shade_editor::production_target::{
    ProductionTargetProfileInspection, inspect_production_target_profile,
    validate_target_channel_names, verify_production_target_profile,
};
use windows_shade_editor::tiff_io::ColorModel as ConversionColorModel;

const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(220);

thread_local! {
    static CANDIDATE_CONTROLLER: RefCell<CandidateController> =
        RefCell::new(CandidateController::default());
}

#[derive(Clone)]
struct CandidateConfig {
    engine_mode: ConversionEngineMode,
    target_profile: Option<ProductionTargetProfileInspection>,
    target_name: String,
    channel_names: Vec<String>,
    channel_names_confirmed: bool,
    output_bit_depth: u8,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
    output_path: Option<PathBuf>,
}

impl Default for CandidateConfig {
    fn default() -> Self {
        Self {
            engine_mode: ConversionEngineMode::Icc,
            target_profile: None,
            target_name: String::new(),
            channel_names: Vec::new(),
            channel_names_confirmed: false,
            output_bit_depth: 16,
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            output_path: None,
        }
    }
}

struct PendingCandidate {
    generation: u64,
    key: String,
    cancellation: ConversionCancellation,
    rx: mpsc::Receiver<Result<CandidatePreviewResult, String>>,
}

struct ActiveCandidate {
    key: String,
    face_index: usize,
    project_revision: u64,
    recipe: ConversionRecipe,
    result: CandidatePreviewResult,
    solo_channel: Option<usize>,
    texture: egui::TextureHandle,
}

struct CandidateController {
    open: bool,
    config: CandidateConfig,
    desired_key: Option<String>,
    desired_recipe: Option<ConversionRecipe>,
    debounce_started: Option<Instant>,
    generation: u64,
    pending: Option<PendingCandidate>,
    active: Option<ActiveCandidate>,
    error: Option<String>,
}

impl Default for CandidateController {
    fn default() -> Self {
        Self {
            open: false,
            config: CandidateConfig::default(),
            desired_key: None,
            desired_recipe: None,
            debounce_started: None,
            generation: 0,
            pending: None,
            active: None,
            error: None,
        }
    }
}

#[derive(Clone)]
struct CandidateSource {
    face_index: usize,
    face_label: String,
    source_path: PathBuf,
    source_model: IccSourceModel,
    conversion_color_model: ConversionColorModel,
    profile_identity: windows_shade_editor::model::IccProfileIdentity,
    captured_profile: CapturedSourceProfile,
    embedded_source_icc: Option<Vec<u8>>,
    width: usize,
    height: usize,
    adjusted_planes: Vec<Vec<u16>>,
}

impl ShadeApp {
    pub(crate) fn ui_conversion_candidate_status(&mut self, ui: &mut egui::Ui) {
        self.poll_candidate_preview(ui.ctx());
        self.apply_candidate_texture();

        let (active, pending, open) = CANDIDATE_CONTROLLER.with(|cell| {
            let controller = cell.borrow();
            (
                controller.active.is_some(),
                controller.pending.is_some(),
                controller.open,
            )
        });
        if self.faces.get(self.current_face).is_none() && !open {
            return;
        }
        ui.separator();
        let label = if pending {
            "Candidate Preview…"
        } else if active {
            "Candidate Preview ✓"
        } else {
            "Candidate Preview"
        };
        if ui
            .small_button(label)
            .on_hover_text(
                "Preview the exact ICC/DeviceLink production separation in the main viewport before committing output.",
            )
            .clicked()
        {
            CANDIDATE_CONTROLLER.with(|cell| cell.borrow_mut().open = true);
        }
    }

    pub(crate) fn ui_conversion_candidate_window(&mut self, ctx: &egui::Context) {
        let (mut open, mut config, active_summary, error) = CANDIDATE_CONTROLLER.with(|cell| {
            let controller = cell.borrow();
            (
                controller.open,
                controller.config.clone(),
                controller.active.as_ref().map(|active| {
                    (
                        active.result.recipe_sha256.clone(),
                        active.result.channel_count(),
                        active.solo_channel,
                    )
                }),
                controller.error.clone(),
            )
        });
        if !open {
            return;
        }

        if config.output_path.is_none() {
            config.output_path = self
                .faces
                .get(self.current_face)
                .and_then(|face| recommended_candidate_output(&face.path, &config.target_name, config.channel_names.len()).ok());
        }

        let source = self.candidate_source();
        let recipe = source
            .as_ref()
            .and_then(|source| build_candidate_recipe(&config, source).ok());
        let mut select_profile = false;
        let mut select_output = false;
        let mut queue_exact = false;
        let mut return_source = false;
        let mut force_refresh = false;
        let mut requested_solo: Option<Option<usize>> = None;

        egui::Window::new("Live Production Candidate")
            .id(egui::Id::new("live-production-candidate-preview"))
            .open(&mut open)
            .resizable(true)
            .default_size([860.0, 760.0])
            .min_width(640.0)
            .show(ctx, |ui| {
                ui.heading("Live Production Candidate");
                ui.label(
                    "The main viewport shows the candidate target separation. No TIFF or Production project is created until you explicitly queue the conversion.",
                );
                ui.small(
                    "Preview samples use the production ICC / N-channel / DeviceLink transform primitives. Target profile bytes and Source ICC identity are reverified before every candidate render.",
                );

                ui.add_space(8.0);
                ui.separator();
                ui.strong("Source readiness");
                match &source {
                    Ok(source) => {
                        ui.label(format!(
                            "Face {} · {} · {}×{} preview raster",
                            source.face_index + 1,
                            source.face_label,
                            source.width,
                            source.height
                        ));
                        if self.project_dirty {
                            ui.label(
                                egui::RichText::new(
                                    "Save the Source project before previewing/queueing so candidate adjustments and the final captured source state are identical.",
                                )
                                .color(egui::Color32::LIGHT_RED),
                            );
                        } else if self.project_path.is_none() {
                            ui.label(
                                egui::RichText::new("Save the Source project first.")
                                    .color(egui::Color32::LIGHT_RED),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("Saved Source state is ready")
                                    .color(egui::Color32::LIGHT_GREEN),
                            );
                        }
                    }
                    Err(message) => {
                        ui.label(egui::RichText::new(message).color(egui::Color32::LIGHT_RED));
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading("Production target");
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
                    config.output_path = None;
                    config.black_point_compensation =
                        config.engine_mode == ConversionEngineMode::Icc;
                }

                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(match config.engine_mode {
                            ConversionEngineMode::Icc => "Select Output ICC…",
                            ConversionEngineMode::DeviceLink => "Select DeviceLink…",
                            ConversionEngineMode::CustomOptimizer => "Select target…",
                        })
                        .clicked()
                    {
                        select_profile = true;
                    }
                    if let Some(profile) = config.target_profile.as_ref() {
                        ui.label(format!(
                            "{} · {} · {} channels",
                            profile.identity.description,
                            profile.output_space_label,
                            profile.output_channel_count
                        ));
                    }
                });

                if config.target_profile.is_some() {
                    ui.horizontal(|ui| {
                        ui.label("Target name");
                        ui.text_edit_singleline(&mut config.target_name);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Output bit depth");
                        ui.selectable_value(&mut config.output_bit_depth, 8, "8-bit");
                        ui.selectable_value(&mut config.output_bit_depth, 16, "16-bit");
                    });
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
                    if config.engine_mode == ConversionEngineMode::Icc {
                        ui.checkbox(
                            &mut config.black_point_compensation,
                            "Black Point Compensation",
                        );
                    }

                    ui.add_space(5.0);
                    ui.strong("Validated target topology");
                    for (index, name) in config.channel_names.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            let rgb = target_channel_rgb(name, index);
                            ui.colored_label(egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]), "■");
                            ui.label(format!("{}", index + 1));
                            ui.text_edit_singleline(name);
                        });
                    }
                    if config
                        .target_profile
                        .as_ref()
                        .is_some_and(|profile| !profile.channel_names_authoritative)
                    {
                        ui.checkbox(
                            &mut config.channel_names_confirmed,
                            "I confirm this real production channel order",
                        );
                    } else {
                        config.channel_names_confirmed = true;
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading("Candidate analysis");
                match recipe.as_ref() {
                    Some(recipe) if !self.project_dirty && self.project_path.is_some() => {
                        let sha = recipe_sha256(recipe).unwrap_or_default();
                        ui.small(format!("Immutable recipe SHA-256: {sha}"));
                        if ui.button("Refresh candidate now").clicked() {
                            force_refresh = true;
                        }
                    }
                    Some(_) => {
                        ui.label(
                            egui::RichText::new("Save the Source project to activate candidate preview.")
                                .color(egui::Color32::YELLOW),
                        );
                    }
                    None => {
                        ui.label(
                            egui::RichText::new("Complete a valid production target setup.")
                                .color(egui::Color32::LIGHT_RED),
                        );
                    }
                }

                if let Some(message) = error.as_deref() {
                    ui.label(egui::RichText::new(message).color(egui::Color32::LIGHT_RED));
                }

                if let Some((sha, channel_count, solo)) = active_summary.as_ref() {
                    ui.group(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Candidate active · {channel_count} target channels"
                                ))
                                .color(egui::Color32::LIGHT_GREEN)
                                .strong(),
                            );
                            ui.small(format!("recipe {}", short_hash(sha)));
                        });
                        if ui
                            .selectable_label(solo.is_none(), "Composite converted preview")
                            .clicked()
                        {
                            requested_solo = Some(None);
                        }
                    });
                    render_active_candidate_analysis(ui, &mut requested_solo);
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading("Commit this exact recipe");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Choose output TIFF…").clicked() {
                        select_output = true;
                    }
                    if let Some(path) = config.output_path.as_ref() {
                        ui.label(path.display().to_string());
                    }
                });
                let can_queue = recipe.is_some()
                    && active_summary.is_some()
                    && !self.project_dirty
                    && self.project_path.is_some()
                    && self.job.is_none()
                    && !self.export.queue.has_pending();
                if ui
                    .add_enabled(can_queue, egui::Button::new("Queue this exact conversion"))
                    .on_hover_text(
                        "Captures the saved Source state and queues the same immutable recipe currently represented by the candidate preview.",
                    )
                    .clicked()
                {
                    queue_exact = true;
                }
                if self.export.queue.has_pending() {
                    ui.small("Finish or cancel Export Queue before queueing production conversion.");
                }

                ui.add_space(6.0);
                if ui.button("Return to Source-adjusted view").clicked() {
                    return_source = true;
                }
            });

        if select_profile {
            if let Some(source) = source.as_ref().ok() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("ICC / DeviceLink profiles", &["icc", "icm"])
                    .set_title(match config.engine_mode {
                        ConversionEngineMode::Icc => "Select Production Output ICC",
                        ConversionEngineMode::DeviceLink => "Select Production DeviceLink",
                        ConversionEngineMode::CustomOptimizer => "Select target",
                    })
                    .pick_file()
                {
                    match inspect_production_target_profile(
                        &path,
                        config.engine_mode,
                        source.conversion_color_model,
                    ) {
                        Ok(profile) => {
                            config.target_name = profile.identity.description.clone();
                            config.channel_names = profile.channel_names.clone();
                            config.channel_names_confirmed = profile.channel_names_authoritative;
                            config.output_path = recommended_candidate_output(
                                &source.source_path,
                                &config.target_name,
                                profile.output_channel_count,
                            )
                            .ok();
                            config.target_profile = Some(profile);
                        }
                        Err(error) => self.report_error(error),
                    }
                }
            }
        }

        if select_output {
            let mut dialog = rfd::FileDialog::new()
                .add_filter("Production TIFF", &["tif", "tiff"])
                .set_title("Select candidate Production TIFF destination");
            if let Some(path) = config.output_path.as_deref() {
                if let Some(parent) = path.parent() {
                    dialog = dialog.set_directory(parent);
                }
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    dialog = dialog.set_file_name(name);
                }
            }
            if let Some(path) = dialog.save_file() {
                if let Some(source) = source.as_ref().ok() {
                    match validate_conversion_output_path(&source.source_path, &path) {
                        Ok(()) => config.output_path = Some(path),
                        Err(error) => self.report_error(error.to_string()),
                    }
                }
            }
        }

        CANDIDATE_CONTROLLER.with(|cell| {
            let mut controller = cell.borrow_mut();
            controller.open = open;
            controller.config = config.clone();
        });

        if return_source || !open {
            self.clear_candidate_preview();
            CANDIDATE_CONTROLLER.with(|cell| cell.borrow_mut().open = open && !return_source);
            return;
        }

        if let (Ok(source), Some(recipe)) = (source.as_ref(), recipe.clone()) {
            if !self.project_dirty && self.project_path.is_some() {
                let key = candidate_key(
                    self.current_face,
                    self.project_revision,
                    &recipe,
                    &source.source_path,
                );
                self.observe_candidate_desire(key, recipe, force_refresh, ctx);
            } else {
                self.invalidate_candidate_desire();
            }
        } else {
            self.invalidate_candidate_desire();
        }

        if let Some(solo) = requested_solo {
            self.set_candidate_solo(solo, ctx);
        }
        if queue_exact {
            if let (Ok(source), Some(recipe)) = (source, recipe) {
                self.queue_candidate_conversion(source, recipe);
            }
        }
    }

    fn candidate_source(&self) -> Result<CandidateSource, String> {
        let face = self
            .faces
            .get(self.current_face)
            .ok_or_else(|| "No active Source Face.".to_owned())?;
        if !face.available {
            return Err("Active Source Face is missing; relink it before candidate preview.".to_owned());
        }
        let descriptor = face
            .preview
            .source_descriptor()
            .ok_or_else(|| "Active Face has no production source descriptor.".to_owned())?;
        if descriptor.transparency != windows_shade_editor::design_source::TransparencyState::None {
            return Err(
                "Candidate preview currently requires an opaque source. Resolve PNG alpha with an explicit production flatten policy in Converter before previewing."
                    .to_owned(),
            );
        }
        if !matches!(descriptor.bit_depth, 8 | 16) {
            return Err(format!(
                "Candidate preview requires 8/16-bit Source data; found {}-bit.",
                descriptor.bit_depth
            ));
        }
        let (source_model, conversion_color_model) = match face.preview.color_model() {
            RuntimeColorModel::Rgb => (IccSourceModel::Rgb, ConversionColorModel::Rgb),
            RuntimeColorModel::Cmyk => (IccSourceModel::Cmyk, ConversionColorModel::Cmyk),
            other => {
                return Err(format!(
                    "Candidate production conversion requires RGB or CMYK Source data; found {}.",
                    other.title()
                ));
            }
        };
        let face_ref = self.project.faces.get(self.current_face);
        let (profile_identity, captured_profile, embedded_source_icc) =
            if let Some(assignment) = face_ref.and_then(|face| face.production_source_profile.as_ref()) {
                let inspected = color_management::inspect_production_source_profile_runtime(
                    Path::new(&assignment.path),
                    &assignment.identity,
                    face.preview.color_model(),
                )?;
                (
                    conversion_identity(inspected.identity()),
                    CapturedSourceProfile::External {
                        path: PathBuf::from(&assignment.path),
                    },
                    None,
                )
            } else {
                let identity = color_management::production_embedded_profile_identity_for_runtime(
                    face.preview.color_model(),
                    face.preview.embedded_icc(),
                )?
                .ok_or_else(|| {
                    "Source Face has no embedded production ICC. Assign a production Source ICC first."
                        .to_owned()
                })?;
                (
                    conversion_identity(&identity),
                    CapturedSourceProfile::Embedded,
                    face.preview.embedded_icc().map(ToOwned::to_owned),
                )
            };
        let adjusted_planes = render::adjusted_planes(face.preview.as_ref(), &self.project);
        let face_label = face_ref
            .map(|face| face.label.clone())
            .filter(|label| !label.trim().is_empty())
            .or_else(|| {
                face.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| format!("Face {}", self.current_face + 1));
        Ok(CandidateSource {
            face_index: self.current_face,
            face_label,
            source_path: face.path.clone(),
            source_model,
            conversion_color_model,
            profile_identity,
            captured_profile,
            embedded_source_icc,
            width: face.preview.width(),
            height: face.preview.height(),
            adjusted_planes,
        })
    }

    fn observe_candidate_desire(
        &mut self,
        key: String,
        recipe: ConversionRecipe,
        force_refresh: bool,
        ctx: &egui::Context,
    ) {
        let mut should_start = false;
        CANDIDATE_CONTROLLER.with(|cell| {
            let mut controller = cell.borrow_mut();
            let changed = controller.desired_key.as_deref() != Some(key.as_str());
            if changed || force_refresh {
                if let Some(pending) = controller.pending.take() {
                    pending.cancellation.request();
                }
                controller.desired_key = Some(key.clone());
                controller.desired_recipe = Some(recipe.clone());
                controller.debounce_started = Some(if force_refresh {
                    Instant::now() - PREVIEW_DEBOUNCE
                } else {
                    Instant::now()
                });
                controller.error = None;
                if controller.active.as_ref().is_some_and(|active| active.key != key) {
                    controller.active = None;
                }
            }
            if controller.active.as_ref().is_some_and(|active| active.key == key) {
                controller.debounce_started = None;
            } else if controller.pending.is_none()
                && controller
                    .debounce_started
                    .is_some_and(|started| started.elapsed() >= PREVIEW_DEBOUNCE)
            {
                should_start = true;
                controller.debounce_started = None;
            }
        });
        if should_start {
            self.start_candidate_preview(key, recipe, ctx);
        } else {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn start_candidate_preview(
        &mut self,
        key: String,
        recipe: ConversionRecipe,
        ctx: &egui::Context,
    ) {
        let source = match self.candidate_source() {
            Ok(source) => source,
            Err(error) => {
                CANDIDATE_CONTROLLER.with(|cell| cell.borrow_mut().error = Some(error));
                return;
            }
        };
        let input = CandidatePreviewInput {
            width: source.width,
            height: source.height,
            source_model: source.source_model,
            source_planes: source.adjusted_planes,
            source_profile: source.captured_profile,
            embedded_source_icc: source.embedded_source_icc,
            recipe: recipe.clone(),
        };
        let cancellation = ConversionCancellation::default();
        let worker_cancellation = cancellation.clone();
        let (tx, rx) = mpsc::channel();
        let generation = CANDIDATE_CONTROLLER.with(|cell| {
            let mut controller = cell.borrow_mut();
            controller.generation = controller.generation.wrapping_add(1).max(1);
            controller.generation
        });
        thread::spawn(move || {
            let result = render_candidate_preview(input, &worker_cancellation);
            let _ = tx.send(result);
        });
        CANDIDATE_CONTROLLER.with(|cell| {
            cell.borrow_mut().pending = Some(PendingCandidate {
                generation,
                key,
                cancellation,
                rx,
            });
        });
        ctx.request_repaint_after(Duration::from_millis(30));
    }

    fn poll_candidate_preview(&mut self, ctx: &egui::Context) {
        let completion = CANDIDATE_CONTROLLER.with(|cell| {
            let mut controller = cell.borrow_mut();
            let pending = controller.pending.as_mut()?;
            match pending.rx.try_recv() {
                Ok(result) => {
                    let generation = pending.generation;
                    let key = pending.key.clone();
                    controller.pending = None;
                    Some((generation, key, result))
                }
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    controller.pending = None;
                    Some((0, String::new(), Err("Candidate preview worker disconnected.".to_owned())))
                }
            }
        });
        let Some((generation, key, result)) = completion else {
            return;
        };
        let desired = CANDIDATE_CONTROLLER.with(|cell| cell.borrow().desired_key.clone());
        if desired.as_deref() != Some(key.as_str()) {
            return;
        }
        match result {
            Ok(result) => {
                let recipe = CANDIDATE_CONTROLLER.with(|cell| cell.borrow().desired_recipe.clone());
                let Some(recipe) = recipe else {
                    return;
                };
                let rgba = candidate_rgba(&result, None);
                let texture = candidate_texture(ctx, generation, None, &result, rgba);
                CANDIDATE_CONTROLLER.with(|cell| {
                    let mut controller = cell.borrow_mut();
                    controller.error = None;
                    controller.active = Some(ActiveCandidate {
                        key,
                        face_index: self.current_face,
                        project_revision: self.project_revision,
                        recipe,
                        result,
                        solo_channel: None,
                        texture,
                    });
                });
                self.report_info("Production candidate preview ready");
            }
            Err(error) => {
                CANDIDATE_CONTROLLER.with(|cell| cell.borrow_mut().error = Some(error));
            }
        }
    }

    fn apply_candidate_texture(&mut self) {
        let active = CANDIDATE_CONTROLLER.with(|cell| {
            cell.borrow().active.as_ref().map(|active| {
                (
                    active.face_index,
                    active.project_revision,
                    active.texture.clone(),
                )
            })
        });
        let Some((face_index, revision, texture)) = active else {
            return;
        };
        if face_index != self.current_face || revision != self.project_revision || self.project_dirty {
            self.clear_candidate_preview();
            return;
        }
        if let Some(face) = self.faces.get_mut(face_index) {
            face.texture = Some(texture);
        }
    }

    fn set_candidate_solo(&mut self, solo: Option<usize>, ctx: &egui::Context) {
        CANDIDATE_CONTROLLER.with(|cell| {
            let mut controller = cell.borrow_mut();
            let generation = controller.generation;
            if let Some(active) = controller.active.as_mut() {
                let solo = solo.filter(|index| *index < active.result.channel_count());
                active.solo_channel = solo;
                let rgba = candidate_rgba(&active.result, solo);
                active.texture = candidate_texture(ctx, generation, solo, &active.result, rgba);
            }
        });
        self.apply_candidate_texture();
    }

    fn invalidate_candidate_desire(&mut self) {
        let had_active = CANDIDATE_CONTROLLER.with(|cell| {
            let mut controller = cell.borrow_mut();
            if let Some(pending) = controller.pending.take() {
                pending.cancellation.request();
            }
            controller.desired_key = None;
            controller.desired_recipe = None;
            controller.debounce_started = None;
            controller.active.take().is_some()
        });
        if had_active {
            self.force_source_preview_refresh();
        }
    }

    fn clear_candidate_preview(&mut self) {
        let had_active = CANDIDATE_CONTROLLER.with(|cell| {
            let mut controller = cell.borrow_mut();
            if let Some(pending) = controller.pending.take() {
                pending.cancellation.request();
            }
            controller.desired_key = None;
            controller.desired_recipe = None;
            controller.debounce_started = None;
            controller.error = None;
            controller.active.take().is_some()
        });
        if had_active {
            self.force_source_preview_refresh();
        }
    }

    fn force_source_preview_refresh(&mut self) {
        if let Some(face) = self.faces.get_mut(self.current_face) {
            face.texture = None;
            face.generation = face.generation.wrapping_add(1).max(1);
        }
    }

    fn queue_candidate_conversion(&mut self, source: CandidateSource, recipe: ConversionRecipe) {
        let active_matches = CANDIDATE_CONTROLLER.with(|cell| {
            cell.borrow().active.as_ref().is_some_and(|active| {
                active.face_index == source.face_index
                    && active.project_revision == self.project_revision
                    && active.result.recipe_sha256 == recipe_sha256(&recipe).unwrap_or_default()
            })
        });
        if !active_matches {
            self.report_error(
                "Candidate preview is stale. Wait for the current recipe preview before queueing conversion.",
            );
            return;
        }
        if self.project_dirty {
            self.report_error("Save the Source project before queueing candidate conversion.");
            return;
        }
        if self.job.is_some() || self.export.queue.has_pending() {
            self.report_info("Finish the current foreground/Export operation before queueing conversion.");
            return;
        }
        let Some(source_project_path) = self.project_path.clone() else {
            self.report_error("Save the Source project before queueing candidate conversion.");
            return;
        };
        let output_path = CANDIDATE_CONTROLLER.with(|cell| cell.borrow().config.output_path.clone());
        let Some(preferred_output) = output_path else {
            self.report_error("Choose a Production TIFF destination first.");
            return;
        };
        let effective_output = match next_versioned_output_path(&preferred_output) {
            Ok(path) => path,
            Err(error) => {
                self.report_error(error.to_string());
                return;
            }
        };
        if let Err(error) = validate_conversion_output_path(&source.source_path, &effective_output) {
            self.report_error(error.to_string());
            return;
        }
        let production_project_path = effective_output.with_extension("shade");
        if production_project_path.exists() {
            self.report_error(format!(
                "Production project already exists: {}. Choose another destination.",
                production_project_path.display()
            ));
            return;
        }
        let captured_project: windows_shade_editor::model::ShadeProject =
            match serde_json::to_value(&self.project)
                .map_err(|error| error.to_string())
                .and_then(|value| serde_json::from_value(value).map_err(|error| error.to_string()))
            {
                Ok(project) => project,
                Err(error) => {
                    self.report_error(format!("Cannot capture Source project: {error}"));
                    return;
                }
            };
        let source_project_sha =
            match windows_shade_editor::icc_conversion_worker::sha256_file(&source_project_path) {
                Ok(hash) => hash,
                Err(error) => {
                    self.report_error(error);
                    return;
                }
            };
        let source_file_sha =
            match windows_shade_editor::icc_conversion_worker::sha256_file(&source.source_path) {
                Ok(hash) => hash,
                Err(error) => {
                    self.report_error(error);
                    return;
                }
            };
        let target_name = recipe.target.name.clone();
        let capture = match ConversionJobCapture::capture(
            &captured_project,
            source_project_path,
            source_project_sha,
            source.source_path,
            self.project.active_snapshot_id,
            source_file_sha,
            source.captured_profile,
            recipe,
            CapturedOutputPolicy::MustNotExist,
            effective_output,
            production_project_path,
            format!("{} - {target_name}", self.project.name),
            format!("{} - {target_name}", source.face_label),
        ) {
            Ok(capture) => capture,
            Err(error) => {
                self.report_error(error);
                return;
            }
        };
        match self.conversion_queue.enqueue(capture, self.settings.default_dpi) {
            Ok(id) => self.report_info(format!(
                "Queued production conversion #{id} from the exact candidate recipe."
            )),
            Err(error) => self.report_error(error),
        }
    }
}

fn build_candidate_recipe(
    config: &CandidateConfig,
    source: &CandidateSource,
) -> Result<ConversionRecipe, String> {
    let stored = config
        .target_profile
        .as_ref()
        .ok_or_else(|| "Select a production Output ICC or DeviceLink.".to_owned())?;
    let verified = verify_production_target_profile(
        &stored.path,
        &stored.identity,
        config.engine_mode,
        source.conversion_color_model,
    )?;
    validate_target_channel_names(&config.channel_names, verified.output_channel_count)?;
    if !verified.channel_names_authoritative && !config.channel_names_confirmed {
        return Err("Confirm the real production channel order before previewing.".to_owned());
    }
    if config.target_name.trim().is_empty() {
        return Err("Target name cannot be empty.".to_owned());
    }
    if !matches!(config.output_bit_depth, 8 | 16) {
        return Err("Output bit depth must be 8 or 16.".to_owned());
    }
    let profile_path = verified.path.to_string_lossy().into_owned();
    let profile_identity = verified.identity.clone();
    let (output_profile_path, output_profile_identity, device_link_path, device_link_identity) =
        match config.engine_mode {
            ConversionEngineMode::Icc => (
                Some(profile_path),
                Some(profile_identity),
                None,
                None,
            ),
            ConversionEngineMode::DeviceLink => (
                None,
                None,
                Some(profile_path),
                Some(profile_identity),
            ),
            ConversionEngineMode::CustomOptimizer => {
                return Err("Custom Optimizer candidate preview requires characterized-target authorization.".to_owned());
            }
        };
    let recipe = ConversionRecipe {
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: config.engine_mode,
        source_profile_identity: source.profile_identity.clone(),
        source_transparency_policy: None,
        target: ConversionTargetDefinition {
            name: config.target_name.trim().to_owned(),
            channels: config
                .channel_names
                .iter()
                .enumerate()
                .map(|(index, name)| TargetChannelDefinition {
                    name: name.trim().to_owned(),
                    display_rgb: Some(target_channel_rgb(name, index)),
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

fn render_active_candidate_analysis(
    ui: &mut egui::Ui,
    requested_solo: &mut Option<Option<usize>>,
) {
    CANDIDATE_CONTROLLER.with(|cell| {
        let controller = cell.borrow();
        let Some(active) = controller.active.as_ref() else {
            return;
        };
        ui.add_space(5.0);
        for (index, channel) in active.result.channels.iter().enumerate() {
            let rgb = channel
                .display_rgb
                .unwrap_or_else(|| target_channel_rgb(&channel.name, index));
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        active.solo_channel == Some(index),
                        format!("{}  {}", index + 1, channel.name),
                    )
                    .clicked()
                {
                    *requested_solo = Some(Some(index));
                }
                ui.colored_label(egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]), "■");
                ui.small(format!("Solidity {:.0}%", channel.solidity * 100.0));
            });
            if let Some(histogram) = active.result.histograms.get(index) {
                draw_candidate_histogram(ui, histogram, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            }
        }
        if active.solo_channel.is_some() && ui.small_button("Return to composite").clicked() {
            *requested_solo = Some(None);
        }
    });
}

fn draw_candidate_histogram(ui: &mut egui::Ui, histogram: &[u32; 256], color: egui::Color32) {
    let desired = egui::vec2(ui.available_width().max(120.0), 62.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let max = histogram.iter().copied().max().unwrap_or(1).max(1) as f32;
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let points = histogram
        .iter()
        .enumerate()
        .map(|(index, count)| {
            let x = rect.left() + index as f32 / 255.0 * rect.width();
            let y = rect.bottom() - (*count as f32 / max) * rect.height();
            egui::pos2(x, y)
        })
        .collect::<Vec<_>>();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.4, color)));
}

fn candidate_rgba(result: &CandidatePreviewResult, solo: Option<usize>) -> Vec<u8> {
    let pixel_count = result.width.saturating_mul(result.height);
    let mut rgba = Vec::with_capacity(pixel_count.saturating_mul(4));
    if let Some(channel) = solo.filter(|index| *index < result.planes.len()) {
        for sample in &result.planes[channel] {
            let gray = 255u8.saturating_sub((*sample >> 8) as u8);
            rgba.extend_from_slice(&[gray, gray, gray, 255]);
        }
        return rgba;
    }
    for pixel in 0..pixel_count {
        let mut rgb = [1.0f32; 3];
        for (index, channel) in result.channels.iter().enumerate() {
            let Some(plane) = result.planes.get(index) else {
                continue;
            };
            let coverage = plane[pixel] as f32 / u16::MAX as f32;
            if coverage <= 0.000_1 {
                continue;
            }
            let tint = channel
                .display_rgb
                .unwrap_or_else(|| target_channel_rgb(&channel.name, index));
            let tint = [
                tint[0] as f32 / 255.0,
                tint[1] as f32 / 255.0,
                tint[2] as f32 / 255.0,
            ];
            let strength = (coverage * channel.solidity).clamp(0.0, 1.0);
            for component in 0..3 {
                rgb[component] = rgb[component] * (1.0 - strength) + tint[component] * strength;
            }
        }
        rgba.extend_from_slice(&[
            (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            255,
        ]);
    }
    rgba
}

fn candidate_texture(
    ctx: &egui::Context,
    generation: u64,
    solo: Option<usize>,
    result: &CandidatePreviewResult,
    rgba: Vec<u8>,
) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied([result.width, result.height], &rgba);
    ctx.load_texture(
        format!("production-candidate-{generation}-{:?}", solo),
        image,
        egui::TextureOptions::LINEAR,
    )
}

fn candidate_key(
    face_index: usize,
    project_revision: u64,
    recipe: &ConversionRecipe,
    source_path: &Path,
) -> String {
    format!(
        "{}|{}|{}|{}",
        face_index,
        project_revision,
        source_path.to_string_lossy().to_ascii_lowercase(),
        recipe_sha256(recipe).unwrap_or_default()
    )
}

fn recommended_candidate_output(
    source: &Path,
    target_name: &str,
    channel_count: usize,
) -> Result<PathBuf, windows_shade_editor::conversion_output::OutputPathError> {
    let suffix = if target_name.trim().is_empty() {
        if channel_count == 4 {
            "CMYK".to_owned()
        } else if channel_count > 0 {
            format!("{channel_count}C")
        } else {
            "Production".to_owned()
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

fn conversion_identity(identity: &model::IccProfileIdentity) -> windows_shade_editor::model::IccProfileIdentity {
    windows_shade_editor::model::IccProfileIdentity {
        description: identity.description.clone(),
        sha256: identity.sha256.clone(),
    }
}

fn target_channel_rgb(name: &str, index: usize) -> [u8; 3] {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.contains("cyan") {
        [0, 174, 239]
    } else if normalized.contains("magenta") || normalized.contains("pink") {
        [236, 0, 140]
    } else if normalized.contains("yellow") {
        [255, 221, 0]
    } else if normalized.contains("black") || normalized == "k" {
        [28, 28, 28]
    } else if normalized.contains("blue") {
        [33, 102, 214]
    } else if normalized.contains("green") {
        [34, 160, 90]
    } else if normalized.contains("brown") {
        [139, 90, 43]
    } else if normalized.contains("beige") {
        [211, 184, 142]
    } else if normalized.contains("orange") {
        [239, 126, 34]
    } else if normalized.contains("red") {
        [214, 51, 63]
    } else if normalized.contains("violet") || normalized.contains("purple") {
        [133, 82, 196]
    } else {
        const FALLBACK: [[u8; 3]; 8] = [
            [39, 126, 220],
            [214, 65, 75],
            [45, 164, 103],
            [224, 151, 38],
            [145, 91, 201],
            [36, 166, 181],
            [191, 96, 51],
            [105, 113, 127],
        ];
        FALLBACK[index % FALLBACK.len()]
    }
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

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_shade_editor::color_conversion::{
        ConversionTargetDefinition, TargetChannelDefinition,
    };

    #[test]
    fn candidate_key_changes_with_recipe_or_source_revision() {
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: windows_shade_editor::model::IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "a".repeat(64),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Target".to_owned(),
                channels: ["Cyan", "Magenta", "Yellow", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: Some(target_channel_rgb(name, 0)),
                        solidity: 1.0,
                        max_coverage: None,
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: Some(windows_shade_editor::model::IccProfileIdentity {
                    description: "Target".to_owned(),
                    sha256: "b".repeat(64),
                }),
                output_profile_path: Some(r"C:\Color\Target.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: ConversionRenderingIntent::Perceptual,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        };
        let first = candidate_key(0, 7, &recipe, Path::new(r"C:\Face.tif"));
        let second = candidate_key(0, 8, &recipe, Path::new(r"C:\Face.tif"));
        assert_ne!(first, second);
    }

    #[test]
    fn solo_candidate_uses_ink_coverage_polarity() {
        let result = CandidatePreviewResult {
            width: 2,
            height: 1,
            recipe_sha256: "a".repeat(64),
            channels: vec![TargetChannelDefinition {
                name: "Black".to_owned(),
                display_rgb: Some([0, 0, 0]),
                solidity: 1.0,
                max_coverage: None,
            }],
            planes: vec![vec![0, u16::MAX]],
            histograms: vec![[0; 256]],
        };
        let rgba = candidate_rgba(&result, Some(0));
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn seven_channel_topology_keeps_authoritative_order() {
        let names = ["Blue", "Brown", "Beige", "Black", "Yellow", "Pink", "Green"];
        let channels = names
            .iter()
            .enumerate()
            .map(|(index, name)| TargetChannelDefinition {
                name: (*name).to_owned(),
                display_rgb: Some(target_channel_rgb(name, index)),
                solidity: 1.0,
                max_coverage: None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            channels.iter().map(|channel| channel.name.as_str()).collect::<Vec<_>>(),
            names
        );
    }
}
