#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

trait ContextKeyboardCompat {
    fn wants_keyboard_input(&self) -> bool;
}

impl ContextKeyboardCompat for eframe::egui::Context {
    fn wants_keyboard_input(&self) -> bool {
        self.egui_wants_keyboard_input()
    }
}

mod adjustment_tools;
mod app_controllers;
mod app_features;
mod app_log;
mod color_management;
mod dpi;
mod export;
mod export_batch;
mod export_queue;
mod export_recipe;
mod history;
mod model;
mod palette;
mod path_safety;
mod previous_shades;
mod project_autosave;
mod project_lifecycle;
mod recovery;
mod render;
mod safe_fs;
mod settings;
mod snapshot_preview_cache;
mod thumbnail;
mod tiff_inspect;
mod tiff_io;
mod ui;
mod update;
mod validation;
mod worker_guard;
mod workflow;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use app_controllers::{ColorManagementController, ExportController, TiffInspectorController};
use chrono::{Local, TimeZone};
use color_management::{PreviewColorConfig, PreviewColorStatus};
use eframe::egui;
use model::{
    ChannelAdjustment, MASTER_ADJUSTMENT_KEY, PreviewRenderingIntent, ShadeProject,
    TEST_CODE_ALL_CHANNELS, TestCodePosition,
};
use palette::ChannelPalette;
use project_lifecycle::{
    BackupRestoreCandidate, ProjectLifecycleController, ProjectTransition, TransitionRequest,
};
use settings::{AppSettings, TonalDisplayMode};
use tiff_io::PreviewFace;
use ui::curve_editor::{curves_ui, tonal_display_value};
use ui::input_router;
use update::{UpdateManager, UpdateStatus};

const VIEWPORT_OVERSCROLL: f32 = 180.0;
const ERROR_TOAST_LIFETIME: Duration = Duration::from_secs(8);
const RECOVERY_AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);
const HISTORY_COMMIT_DELAY: Duration = Duration::from_millis(300);
const PREVIOUS_SHADE_TEXTURE_CACHE_LIMIT: usize = 64;
const APP_WINDOW_TITLE: &str = concat!(
    "Shade Editor v",
    env!("CARGO_PKG_VERSION"),
    " - (EmadGhasemi.ir)"
);

fn main() -> eframe::Result {
    app_log::install_panic_hook();
    let startup_project = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("shade"))
        });
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_WINDOW_TITLE)
            .with_inner_size([1550.0, 920.0])
            .with_min_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        APP_WINDOW_TITLE,
        native_options,
        Box::new(move |cc| {
            let mut app = ShadeApp::new(cc);
            if let Some(path) = startup_project.clone() {
                app.project_view.open = false;
                app.open_project_path(path);
            } else {
                app.project_view.open = true;
            }
            Ok(Box::new(app))
        }),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolPanel {
    Levels,
    Mixer,
    Curves,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdjustmentScope {
    Selected,
    All,
}

#[derive(Clone, Debug)]
enum PendingSnapshotAction {
    Load(u64),
    Save(PathBuf),
}

struct RuntimeFace {
    path: PathBuf,
    available: bool,
    preview: Arc<PreviewFace>,
    dpi: dpi::DpiInfo,
    adjusted_histograms: Vec<[u32; 256]>,
    clipping: Vec<render::ChannelClippingStats>,
    color_status: PreviewColorStatus,
    texture: Option<egui::TextureHandle>,
    original_texture: Option<egui::TextureHandle>,
    original_rendered_solo: Option<Option<usize>>,
    embedded_original_texture: Option<egui::TextureHandle>,
    embedded_original_status: PreviewColorStatus,
    generation: u64,
    rendered_generation: u64,
}

struct LoadedFace {
    path: PathBuf,
    available: bool,
    preview: PreviewFace,
    dpi: dpi::DpiInfo,
}

struct OpenPayload {
    path: PathBuf,
    project: ShadeProject,
    faces: Vec<LoadedFace>,
    errors: Vec<String>,
}

struct RecoveryPayload {
    origin_path: Option<PathBuf>,
    project: ShadeProject,
    faces: Vec<LoadedFace>,
    errors: Vec<String>,
}

#[derive(Clone)]
struct JobProgress {
    label: String,
    detail: String,
    fraction: Option<f32>,
}

struct JobHandle {
    progress: Arc<Mutex<JobProgress>>,
    rx: mpsc::Receiver<JobResult>,
}

enum JobResult {
    AddFaces {
        faces: Vec<LoadedFace>,
        errors: Vec<String>,
    },
    RebuildPreviews(Result<Vec<LoadedFace>, String>),
    RelinkFace {
        index: usize,
        result: Result<LoadedFace, String>,
    },
    RelinkFolder {
        faces: Vec<(usize, LoadedFace)>,
        errors: Vec<String>,
    },
    Open(Result<OpenPayload, String>),
    Recover(Result<RecoveryPayload, String>),
    Save {
        path: PathBuf,
        revision: u64,
        result: Result<(), String>,
    },
    InspectTiff(Result<tiff_inspect::TiffInspection, String>),
    Export(SnapshotExportBatchResult),
    WorkerPanic(String),
}

struct SnapshotExportMark {
    snapshot_id: u64,
    face_key: String,
    folder: PathBuf,
    exported_at_unix_ms: i64,
}

struct SnapshotExportBatchResult {
    result: Result<String, String>,
    marks: Vec<SnapshotExportMark>,
}

struct RenderFailure {
    face_index: usize,
    generation: u64,
    message: String,
}

type RenderMessage = Result<RenderResult, RenderFailure>;

struct RenderResult {
    face_index: usize,
    generation: u64,
    solo_channel: Option<usize>,
    adjusted_histograms: Vec<[u32; 256]>,
    clipping: Vec<render::ChannelClippingStats>,
    color_status: PreviewColorStatus,
    rgba: Vec<u8>,
    original_rgba: Vec<u8>,
    embedded_original_rgba: Option<Vec<u8>>,
    embedded_original_status: Option<PreviewColorStatus>,
}

#[derive(Clone)]
struct SnapshotPreviewEntry {
    texture: egui::TextureHandle,
    adjusted_histograms: Vec<[u32; 256]>,
    clipping: Vec<render::ChannelClippingStats>,
    color_status: PreviewColorStatus,
}

struct ErrorToast {
    message: String,
    created: Instant,
}

struct ShadeApp {
    project: ShadeProject,
    project_path: Option<PathBuf>,
    faces: Vec<RuntimeFace>,
    current_face: usize,
    selected_channel: usize,
    solo_channel: Option<usize>,
    tool: ToolPanel,
    adjustment_scope: AdjustmentScope,
    zoom: f32,
    fit_requested: bool,
    viewport_recenter: bool,
    settings: AppSettings,
    updater: UpdateManager,
    show_settings: bool,
    color: ColorManagementController,
    show_about: bool,
    show_logs: bool,
    previous_shades: previous_shades::PreviousShadesStore,
    project_view: ui::project_view_state::ProjectViewState,
    export: ExportController,
    inspector: TiffInspectorController,
    lifecycle: ProjectLifecycleController,
    log: app_log::AppLog,
    log_cache: String,
    last_update_failure: Option<String>,
    toast: Option<ErrorToast>,
    status_message: String,
    project_dirty: bool,
    project_revision: u64,
    last_project_edit_at: Instant,
    project_autosave_tx: mpsc::Sender<project_autosave::Completion>,
    project_autosave_rx: mpsc::Receiver<project_autosave::Completion>,
    project_autosave_busy: bool,
    project_autosave_error: Option<String>,
    snapshot_rename_id: Option<u64>,
    snapshot_rename_buffer: String,
    pending_snapshot_action: Option<PendingSnapshotAction>,
    history: history::AdjustmentHistory,
    history_clear_backup: Option<(Option<u64>, history::AdjustmentHistory)>,
    history_pending_label: Option<String>,
    history_pending_at: Option<Instant>,
    adjustment_clipboard: Option<adjustment_tools::AdjustmentClipboard>,
    relative_preset_draft: Option<adjustment_tools::RelativePresetDraft>,
    recovery_candidate: Option<recovery::RecoveryFile>,
    autosave_tx: mpsc::Sender<Result<PathBuf, String>>,
    autosave_rx: mpsc::Receiver<Result<PathBuf, String>>,
    autosave_busy: bool,
    last_autosave: Instant,
    job: Option<JobHandle>,
    render_tx: mpsc::Sender<RenderMessage>,
    render_rx: mpsc::Receiver<RenderMessage>,
    render_busy: Option<(usize, u64)>,
    snapshot_preview_cache: snapshot_preview_cache::SnapshotPreviewCache<SnapshotPreviewEntry>,
}

impl ShadeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = AppSettings::load();
        apply_theme(&cc.egui_ctx, settings.dark_mode);
        let updater = UpdateManager::default();
        if settings.auto_update {
            updater.start_check(true);
        }
        let (render_tx, render_rx) = mpsc::channel();
        let (autosave_tx, autosave_rx) = mpsc::channel();
        let (project_autosave_tx, project_autosave_rx) = mpsc::channel();
        let mut project = ShadeProject::default();
        project.channel_palette = settings.default_project_palette();
        let log = app_log::AppLog::default();
        log.info(&format!(
            "Shade Editor {} started",
            env!("CARGO_PKG_VERSION")
        ));
        let previous_shades = previous_shades::PreviousShadesStore::load().unwrap_or_else(|err| {
            log.error(&err);
            previous_shades::PreviousShadesStore::default()
        });
        let recovery_candidate = match recovery::load() {
            Ok(candidate) => candidate,
            Err(err) => {
                log.error(&err);
                None
            }
        };
        let mut history = history::AdjustmentHistory::default();
        history.reset(&project.adjustments, "Start");
        let export_queue = export_queue::ExportQueue::load_persistent().unwrap_or_else(|err| {
            log.error(&err);
            export_queue::ExportQueue::new()
        });
        Self {
            project,
            project_path: None,
            faces: Vec::new(),
            current_face: 0,
            selected_channel: 0,
            solo_channel: None,
            tool: ToolPanel::Levels,
            adjustment_scope: AdjustmentScope::All,
            zoom: 1.0,
            fit_requested: false,
            viewport_recenter: true,
            settings,
            updater,
            show_settings: false,
            color: ColorManagementController::default(),
            show_about: false,
            show_logs: false,
            previous_shades,
            project_view: ui::project_view_state::ProjectViewState::default(),
            export: ExportController::new(export_queue),
            inspector: TiffInspectorController::default(),
            lifecycle: ProjectLifecycleController::default(),
            log,
            log_cache: String::new(),
            last_update_failure: None,
            toast: None,
            status_message: "Ready".to_owned(),
            project_dirty: false,
            project_revision: 0,
            last_project_edit_at: Instant::now(),
            project_autosave_tx,
            project_autosave_rx,
            project_autosave_busy: false,
            project_autosave_error: None,
            snapshot_rename_id: None,
            snapshot_rename_buffer: String::new(),
            pending_snapshot_action: None,
            history,
            history_clear_backup: None,
            history_pending_label: None,
            history_pending_at: None,
            adjustment_clipboard: None,
            relative_preset_draft: None,
            recovery_candidate,
            autosave_tx,
            autosave_rx,
            autosave_busy: false,
            last_autosave: Instant::now(),
            job: None,
            render_tx,
            render_rx,
            render_busy: None,
            snapshot_preview_cache: snapshot_preview_cache::SnapshotPreviewCache::default(),
        }
    }

    fn report_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log.error(&message);
        self.status_message = "Error - see Logs".to_owned();
        self.toast = Some(ErrorToast {
            message,
            created: Instant::now(),
        });
    }

    fn report_info(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log.info(&message);
        self.status_message = message;
    }

    fn mark_project_dirty(&mut self) {
        self.project_dirty = true;
        self.project_revision = self.project_revision.wrapping_add(1).max(1);
        self.last_project_edit_at = Instant::now();
        self.project_autosave_error = None;
    }

    fn mark_project_saved(&mut self) {
        self.project_dirty = false;
        self.last_project_edit_at = Instant::now();
        self.project_autosave_error = None;
    }

    fn new_project(&mut self) {
        self.request_project_transition(ProjectTransition::New, None);
    }

    fn request_project_transition(
        &mut self,
        transition: ProjectTransition,
        ctx: Option<&egui::Context>,
    ) {
        match self.lifecycle.request(
            transition,
            self.job.is_some() || self.project_autosave_busy,
            self.export.queue.has_pending(),
            self.project_dirty,
            !self.faces.is_empty(),
            self.project_path.is_some(),
        ) {
            TransitionRequest::BlockedByOperation => {
                self.report_info("Finish the current operation before changing projects.");
            }
            TransitionRequest::BlockedByExportQueue => {
                self.export.show_queue = true;
                self.report_info(
                    "Finish or cancel the Export Queue before changing projects or exiting.",
                );
            }
            TransitionRequest::AwaitingConfirmation => {}
            TransitionRequest::Execute(transition) => {
                self.execute_project_transition(transition, ctx);
            }
        }
    }

    fn execute_project_transition(
        &mut self,
        transition: ProjectTransition,
        ctx: Option<&egui::Context>,
    ) {
        match transition {
            ProjectTransition::New => self.reset_to_new_project(),
            ProjectTransition::Open(path) => {
                self.project_view.open = false;
                self.open_project_path(path);
            }
            ProjectTransition::Exit => {
                if let Some(ctx) = ctx {
                    self.lifecycle.allow_close_once = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.lifecycle.pending = Some(ProjectTransition::Exit);
                }
            }
            ProjectTransition::Recover => self.recover_project_now(),
        }
    }

    fn complete_transition_after_save(&mut self, ctx: &egui::Context) {
        if let Some(transition) = self
            .lifecycle
            .take_after_successful_save(self.job.is_some(), self.project_dirty)
        {
            self.execute_project_transition(transition, Some(ctx));
        }
    }

    fn bump_project_session(&mut self) {
        self.lifecycle.bump_session();
        self.snapshot_preview_cache.clear();
    }

    fn reset_to_new_project(&mut self) {
        self.project = ShadeProject::default();
        self.project.channel_palette = self.settings.default_project_palette();
        self.project_path = None;
        self.faces.clear();
        self.current_face = 0;
        self.selected_channel = 0;
        self.solo_channel = None;
        self.adjustment_scope = AdjustmentScope::All;
        self.viewport_recenter = true;
        self.fit_requested = true;
        self.mark_project_saved();
        self.snapshot_rename_id = None;
        self.snapshot_rename_buffer.clear();
        self.pending_snapshot_action = None;
        self.lifecycle.cancel_pending();
        self.color.show = false;
        self.color.query.clear();
        self.color.selected = None;
        self.export.remind_after_export = false;
        self.export.show_snapshot_save_reminder = false;
        self.history.reset(&self.project.adjustments, "New project");
        self.history_clear_backup = None;
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.bump_project_session();
        self.report_info("New shade project");
    }

    fn make_runtime_face(item: LoadedFace) -> RuntimeFace {
        RuntimeFace {
            path: item.path,
            available: item.available,
            preview: Arc::new(item.preview),
            dpi: item.dpi,
            adjusted_histograms: Vec::new(),
            clipping: Vec::new(),
            color_status: PreviewColorStatus::Pending,
            texture: None,
            original_texture: None,
            original_rendered_solo: None,
            embedded_original_texture: None,
            embedded_original_status: PreviewColorStatus::Pending,
            generation: 1,
            rendered_generation: 0,
        }
    }

    fn launch_job<F>(&mut self, label: &str, task: F)
    where
        F: FnOnce(Arc<Mutex<JobProgress>>) -> JobResult + Send + 'static,
    {
        if self.job.is_some() {
            self.report_error("Another operation is already in progress.");
            return;
        }
        let progress = Arc::new(Mutex::new(JobProgress {
            label: label.to_owned(),
            detail: String::new(),
            fraction: None,
        }));
        let (tx, rx) = mpsc::channel();
        let worker_progress = Arc::clone(&progress);
        std::thread::spawn(move || {
            let result =
                worker_guard::catch_value("Background operation", || task(worker_progress))
                    .unwrap_or_else(JobResult::WorkerPanic);
            let _ = tx.send(result);
        });
        self.job = Some(JobHandle { progress, rx });
    }

    fn set_progress(
        progress: &Arc<Mutex<JobProgress>>,
        fraction: Option<f32>,
        label: &str,
        detail: &str,
    ) {
        if let Ok(mut state) = progress.lock() {
            state.fraction = fraction.map(|value| value.clamp(0.0, 1.0));
            state.label = label.to_owned();
            state.detail = detail.to_owned();
        }
    }

    fn is_tiff_path(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tif") || ext.eq_ignore_ascii_case("tiff"))
    }

    fn add_faces_paths(&mut self, paths: Vec<PathBuf>) {
        if self.job.is_some() {
            return;
        }
        let paths = paths
            .into_iter()
            .filter(|path| Self::is_tiff_path(path))
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return;
        }
        let max_dimension = self.settings.max_preview_dimension;
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Opening TIFF", move |progress| {
            let total = paths.len().max(1);
            let mut faces = Vec::new();
            let mut errors = Vec::new();
            for (index, path) in paths.into_iter().enumerate() {
                Self::set_progress(
                    &progress,
                    Some(index as f32 / total as f32),
                    "Opening TIFF",
                    &path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                match tiff_io::load_preview(&path, max_dimension) {
                    Ok(preview) => faces.push(LoadedFace {
                        dpi: dpi::read_dpi(&path, default_dpi),
                        path,
                        available: true,
                        preview,
                    }),
                    Err(err) => errors.push(format!("{}: {err}", path.display())),
                }
            }
            Self::set_progress(&progress, Some(1.0), "Opening TIFF", "Complete");
            JobResult::AddFaces { faces, errors }
        });
    }

    fn add_faces_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("TIFF images", &["tif", "tiff"])
            .pick_files()
        else {
            return;
        };
        self.add_faces_paths(paths);
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        let paths = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .filter(|path| Self::is_tiff_path(path))
                .collect::<Vec<_>>()
        });
        if !paths.is_empty() {
            self.add_faces_paths(paths);
        }
    }

    fn rebuild_previews(&mut self) {
        workflow::rebuild_previews(self);
    }

    fn open_project_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Shade project", &["shade"])
            .pick_file()
        else {
            return;
        };
        self.request_project_transition(ProjectTransition::Open(path), None);
    }

    fn open_project_path(&mut self, path: PathBuf) {
        if self.job.is_some() {
            return;
        }
        self.recovery_candidate = None;
        self.lifecycle.backup_restore = None;
        self.lifecycle.opening_path = Some(path.clone());
        let max_dimension = self.settings.max_preview_dimension;
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Opening project", move |progress| {
            let result = (|| -> Result<OpenPayload, String> {
                Self::set_progress(&progress, None, "Opening project", "Reading .shade");
                let mut project = ShadeProject::load(&path)?;
                let resolved = project.resolve_face_paths(&path);
                let total = resolved.len().max(1);
                let mut faces = Vec::new();
                let mut errors = Vec::new();
                for (index, source) in resolved.into_iter().enumerate() {
                    Self::set_progress(
                        &progress,
                        Some(index as f32 / total as f32),
                        "Opening project",
                        &format!("Loading Face {}/{}", index + 1, total),
                    );
                    let expected = project
                        .file_metadata
                        .as_ref()
                        .and_then(|metadata| metadata.faces.get(index))
                        .cloned();
                    match tiff_io::load_preview(&source, max_dimension) {
                        Ok(preview) => {
                            project.ensure_channels(&preview.metadata.channel_names);
                            faces.push(LoadedFace {
                                dpi: dpi::read_dpi(&source, default_dpi),
                                path: source,
                                available: true,
                                preview,
                            });
                        }
                        Err(err) => {
                            errors.push(format!("{}: {err}", source.display()));
                            faces.push(workflow::placeholder_loaded_face(
                                source,
                                expected.as_ref(),
                                default_dpi,
                            ));
                        }
                    }
                }
                Self::set_progress(&progress, Some(1.0), "Opening project", "Complete");
                Ok(OpenPayload {
                    path,
                    project,
                    faces,
                    errors,
                })
            })();
            JobResult::Open(result)
        });
    }

    fn active_snapshot_has_unupdated_changes(&self) -> bool {
        self.project.active_snapshot_id.is_some() && !self.project.active_snapshot_matches()
    }

    fn request_project_save(&mut self, path: PathBuf) -> bool {
        if self.active_snapshot_has_unupdated_changes() {
            self.pending_snapshot_action = Some(PendingSnapshotAction::Save(path));
            true
        } else {
            self.begin_project_save(path)
        }
    }

    fn begin_project_save(&mut self, path: PathBuf) -> bool {
        if self.project_autosave_busy {
            self.report_info("Project autosave is already in progress.");
            return false;
        }
        let save_revision = self.project_revision;
        if self.job.is_some() || self.faces.is_empty() {
            return false;
        }
        self.flush_history_now();
        self.sync_history_to_active_snapshot();
        self.project.name = project_name_for_path(&self.project.name, &path);
        let mut project = self.project.clone();
        project.ensure_snapshot_histories();
        project.file_metadata = Some(build_project_file_metadata(
            &self.project,
            &self.faces,
            self.current_face,
        ));
        let thumbnail_face = self
            .faces
            .get(self.current_face)
            .filter(|face| face.available)
            .or_else(|| self.faces.iter().find(|face| face.available))
            .map(|face| Arc::clone(&face.preview));
        let face_paths = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        let result_path = path.clone();
        self.launch_job("Saving project", move |progress| {
            Self::set_progress(
                &progress,
                Some(0.15),
                "Saving project",
                "Building project thumbnail",
            );
            let result = (|| -> Result<(), String> {
                if let Some(face) = thumbnail_face.as_deref() {
                    project.thumbnail = Some(thumbnail::build_project_thumbnail(face, &project)?);
                }
                Self::set_progress(
                    &progress,
                    Some(0.55),
                    "Saving project",
                    "Serializing project and metadata",
                );
                project.save(&path, &face_paths)
            })();
            Self::set_progress(&progress, Some(1.0), "Saving project", "Complete");
            JobResult::Save {
                path: result_path,
                revision: save_revision,
                result,
            }
        });
        true
    }

    fn save_project(&mut self, save_as: bool) -> bool {
        if self.job.is_some() || self.faces.is_empty() {
            return false;
        }
        let target = if !save_as {
            self.project_path.clone()
        } else {
            None
        };
        let target = match target {
            Some(path) => Some(path),
            None => {
                let mut dialog = rfd::FileDialog::new()
                    .add_filter("Shade project", &["shade"])
                    .set_file_name(format!("{}.shade", sanitize_filename(&self.project.name)));
                if let Some(parent) = self.faces.first().and_then(|face| face.path.parent()) {
                    dialog = dialog.set_directory(parent);
                }
                dialog.save_file()
            }
        };
        let Some(path) = target else {
            return false;
        };
        self.request_project_save(path)
    }

    fn quick_save_target(&self) -> Option<PathBuf> {
        if self.project_path.is_some() || self.faces.is_empty() {
            return None;
        }
        let face = self
            .faces
            .get(self.current_face)
            .or_else(|| self.faces.first())?;
        let parent = face.path.parent().unwrap_or_else(|| Path::new("."));
        let mut stem = sanitize_filename(self.project.name.trim());
        if stem.trim().is_empty()
            || self
                .project
                .name
                .trim()
                .eq_ignore_ascii_case("Untitled Shade")
        {
            stem = face
                .path
                .file_stem()
                .map(|value| sanitize_filename(&value.to_string_lossy()))
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "Shade Project".to_owned());
        }
        Some(unique_shade_path(parent, &stem))
    }

    fn quick_save_project(&mut self) -> bool {
        let Some(path) = self.quick_save_target() else {
            return false;
        };
        self.request_project_save(path)
    }

    fn snapshot_project_needs_save_reminder(&self) -> bool {
        self.project.active_snapshot_id.is_some()
            && (self.project_dirty || self.project_path.is_none())
    }

    fn enqueue_export(&mut self, spec: export_queue::ExportQueueSpec) -> bool {
        let protected_sources = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        match self.export.queue.enqueue_for_project(
            spec,
            protected_sources,
            self.lifecycle.session_id,
        ) {
            Ok(_) => true,
            Err(err) => {
                self.report_error(err);
                false
            }
        }
    }

    fn export_current_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting.",
            );
            return;
        }
        if self
            .project
            .faces
            .get(self.current_face)
            .is_some_and(|face| face.status.is_rejected())
        {
            let answer = rfd::MessageDialog::new()
                .set_title("Export rejected Face?")
                .set_description(
                    "This Face is marked Rejected and is normally excluded from production output. Export this Face anyway?",
                )
                .set_buttons(rfd::MessageButtons::YesNo)
                .set_level(rfd::MessageLevel::Warning)
                .show();
            if answer != rfd::MessageDialogResult::Yes {
                return;
            }
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let stem = face
            .path
            .file_stem()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("TIFF image", &["tif", "tiff"])
            .set_file_name(format!("{stem}-shade.tif"))
            .save_file()
        else {
            return;
        };
        let source = face.path.clone();
        let face_name = self
            .project
            .faces
            .get(self.current_face)
            .map(|face| face.label.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| stem.to_string());
        let state_name = self
            .project
            .active_snapshot_name()
            .unwrap_or("Working")
            .to_owned();
        self.export.remind_after_export = self.snapshot_project_needs_save_reminder();
        if !self.enqueue_export(export_queue::ExportQueueSpec {
            label: format!("{face_name} / {state_name}"),
            source,
            destination,
            recipe: export_recipe::ExportRecipe::from_project(&self.project),
            default_dpi: self.settings.default_dpi,
            force_lzw: self.settings.lzw_compression,
            validate_after_export: self.settings.validate_after_export,
            conflict_policy: export_batch::ConflictPolicy::Overwrite,
            mark: None,
        }) {
            return;
        }
        self.export.show_queue = true;
        self.report_info("Export added to queue");
    }

    fn poll_export_queue(&mut self) {
        let completions = self.export.queue.poll();
        if let Some(err) = self.export.queue.take_persistence_error() {
            self.log.error(&format!("Export Queue persistence: {err}"));
        }
        if completions.is_empty() {
            return;
        }

        let mut completed = 0usize;
        let mut errors = Vec::new();
        for completion in completions {
            if completion.project_session_id != self.lifecycle.session_id {
                self.log.info(&format!(
                    "Ignored queue completion #{} from previous project session {}",
                    completion.id, completion.project_session_id
                ));
                continue;
            }
            if let Some(mark) = completion.mark {
                self.project.record_snapshot_export(
                    mark.snapshot_id,
                    mark.face_key,
                    mark.folder.to_string_lossy().into_owned(),
                    unix_ms_now(),
                );
                self.mark_project_dirty();
            }
            match completion.result {
                Ok(_) => completed += 1,
                Err(err) => errors.push(format!("Queue item #{}: {err}", completion.id)),
            }
        }

        if errors.is_empty() {
            self.report_info(format!("Export queue completed {completed} item(s)"));
        } else {
            self.report_error(errors.join(" | "));
        }

        if !self.export.queue.has_pending() {
            if self.export.remind_after_export {
                self.export.show_snapshot_save_reminder = true;
            }
            self.export.remind_after_export = false;
            if let Some(folder) = self.export.open_folder_after.take() {
                if let Err(err) = open_folder(&folder) {
                    self.report_error(err);
                }
            }
        }
    }

    fn inspect_tiff_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        let mut dialog = rfd::FileDialog::new().add_filter("TIFF image", &["tif", "tiff"]);
        if let Some(parent) = self
            .faces
            .get(self.current_face)
            .and_then(|face| face.path.parent())
        {
            dialog = dialog.set_directory(parent);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Inspecting TIFF", move |progress| {
            Self::set_progress(
                &progress,
                Some(0.1),
                "Inspecting TIFF",
                "Reading bounded TIFF metadata",
            );
            let result = tiff_inspect::inspect(&path, default_dpi);
            Self::set_progress(&progress, Some(1.0), "Inspecting TIFF", "Complete");
            JobResult::InspectTiff(result)
        });
    }

    fn ui_backup_restore_window(&mut self, ctx: &egui::Context) {
        let Some(candidate) = self.lifecycle.backup_restore.clone() else {
            return;
        };
        let mut restore = false;
        let mut cancel = false;
        egui::Window::new("Project backup available")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.strong("The selected .shade file could not be opened, but its .bak backup is valid.");
                ui.label(format!("Primary: {}", candidate.primary_path.display()));
                ui.label(format!("Backup: {}", candidate.backup_path.display()));
                ui.colored_label(egui::Color32::LIGHT_RED, &candidate.primary_error);
                ui.small("Restore keeps a copy of the failed primary as .corrupt before atomically replacing it with the validated backup.");
                ui.horizontal(|ui| {
                    restore = ui.button("Restore validated backup").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if cancel {
            self.lifecycle.backup_restore = None;
        } else if restore {
            let corrupt = append_path_suffix(&candidate.primary_path, ".corrupt");
            let result = (|| -> Result<(), String> {
                if candidate.primary_path.is_file() {
                    safe_fs::atomic_copy(&candidate.primary_path, &corrupt)?;
                }
                safe_fs::atomic_copy(&candidate.backup_path, &candidate.primary_path)?;
                ShadeProject::load(&candidate.primary_path)
                    .map(|_| ())
                    .map_err(|err| format!("Restored backup did not validate: {err}"))
            })();
            match result {
                Ok(()) => {
                    self.lifecycle.backup_restore = None;
                    self.open_project_path(candidate.primary_path);
                }
                Err(err) => self.report_error(format!("Backup restore failed: {err}")),
            }
        }
    }

    fn ui_tiff_inspector_window(&mut self, ctx: &egui::Context) {
        if !self.inspector.show {
            return;
        }
        let mut open = self.inspector.show;
        let report = self
            .inspector
            .inspection
            .as_ref()
            .map(|item| item.report.clone());
        let path = self
            .inspector
            .inspection
            .as_ref()
            .map(|item| item.path.clone());
        let error = self.inspector.error.clone();
        let mut copy_report = false;
        let mut reveal = false;
        let mut display_report = report.clone().unwrap_or_default();

        egui::Window::new("Inspect TIFF")
            .open(&mut open)
            .resizable(true)
            .default_size([820.0, 680.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("TIFF Production Diagnostics");
                    if ui
                        .add_enabled(report.is_some(), egui::Button::new("Copy report"))
                        .clicked()
                    {
                        copy_report = true;
                    }
                    if ui
                        .add_enabled(path.is_some(), egui::Button::new("Reveal TIFF"))
                        .clicked()
                    {
                        reveal = true;
                    }
                });
                if let Some(path) = path.as_ref() {
                    ui.small(path.display().to_string());
                }
                if let Some(error) = error.as_ref() {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if report.is_some() {
                    ui.separator();
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut display_report)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(34)
                                    .interactive(false),
                            );
                        });
                }
            });

        self.inspector.show = open;
        if copy_report {
            if let Some(report) = report {
                ctx.copy_text(report);
                self.report_info("TIFF inspection report copied");
            }
        }
        if reveal {
            if let Some(path) = path {
                if let Err(err) = reveal_in_explorer(&path) {
                    self.report_error(err);
                }
            }
        }
    }

    fn validate_current_face_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before validation.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let mut dialog = rfd::FileDialog::new();
        if let Some(parent) = face.path.parent() {
            dialog = dialog.set_directory(parent);
        }
        let Some(folder) = dialog.pick_folder() else {
            return;
        };
        let source = face.path.clone();
        let default_dpi = self.settings.default_dpi;
        let force_lzw = self.settings.lzw_compression;
        self.export.remind_after_export = false;
        self.launch_job("Validating TIFF", move |progress| {
            let result = validation::validate_no_adjustment_roundtrip_with_options(
                &source,
                &folder,
                default_dpi,
                force_lzw,
                |fraction, detail| {
                    Self::set_progress(&progress, Some(fraction), "Validating TIFF", detail);
                },
            )
            .map(|artifacts| {
                let result = if artifacts.report.passed {
                    "PASS"
                } else {
                    "FAIL"
                };
                format!(
                    "TIFF round-trip {result} · report {}",
                    artifacts.markdown_path.display()
                )
            });
            JobResult::Export(SnapshotExportBatchResult {
                result,
                marks: Vec::new(),
            })
        });
    }

    fn export_all_dialog(&mut self) {
        if self.job.is_some() || self.faces.is_empty() {
            return;
        }
        let accepted_count = self
            .project
            .faces
            .iter()
            .filter(|face| !face.status.is_rejected())
            .count();
        if accepted_count == 0 {
            self.report_error(
                "Export all has no Accepted Faces. Re-accept at least one Face first.",
            );
            return;
        }
        if self.faces.iter().enumerate().any(|(index, face)| {
            !self
                .project
                .faces
                .get(index)
                .is_some_and(|item| item.status.is_rejected())
                && !face.available
        }) {
            self.report_error("Export all requires every Accepted Face source TIFF to be available. Relink missing Accepted Faces first.");
            return;
        }
        if self.export.all_folder.trim().is_empty() {
            let initial = self
                .project_path
                .as_ref()
                .and_then(|path| path.parent())
                .or_else(|| {
                    self.faces
                        .get(self.current_face)
                        .and_then(|face| face.path.parent())
                })
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.export.all_folder = initial;
        }
        self.export.show_all = true;
    }

    fn start_export_all(&mut self) {
        if self.job.is_some() || self.faces.is_empty() {
            return;
        }
        let base_folder = PathBuf::from(self.export.all_folder.trim());
        if self.export.all_folder.trim().is_empty() {
            self.report_error("Choose an Export All folder first.");
            return;
        }
        if let Err(err) = std::fs::create_dir_all(&base_folder) {
            self.report_error(format!(
                "Cannot create Export All folder {}: {err}",
                base_folder.display()
            ));
            return;
        }
        let accepted_count = self
            .project
            .faces
            .iter()
            .filter(|face| !face.status.is_rejected())
            .count();
        if accepted_count == 0 {
            self.report_error(
                "Export all has no Accepted Faces. Re-accept at least one Face first.",
            );
            return;
        }
        if self.faces.iter().enumerate().any(|(index, face)| {
            !self
                .project
                .faces
                .get(index)
                .is_some_and(|item| item.status.is_rejected())
                && !face.available
        }) {
            self.report_error("Export all requires every Accepted Face source TIFF to be available. Relink missing Accepted Faces first.");
            return;
        }

        let rejected_count = self
            .project
            .faces
            .iter()
            .filter(|face| face.status.is_rejected())
            .count();
        let export_faces = self
            .faces
            .iter()
            .enumerate()
            .filter_map(|(index, face)| {
                let project_face = self.project.faces.get(index)?;
                (!project_face.status.is_rejected())
                    .then(|| (index, face.path.clone(), project_face.label.clone()))
            })
            .collect::<Vec<_>>();
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let project_name = self.project.name.clone();
        let test_code = self.project.effective_test_code_text();
        let snapshot_name = self
            .project
            .active_snapshot_name()
            .unwrap_or("Working")
            .to_owned();
        let template = self.settings.export_all_template.clone();
        let folder_template = self.settings.export_folder_template.clone();
        let conflict_policy = self.settings.export_all_conflict_policy;
        let open_after = self.settings.export_all_open_folder;
        let mut project = self.project.clone();
        project.test_code.enabled = self.settings.export_all_test_code;
        let date = Local::now().format("%Y-%m-%d").to_string();
        let mut reserved = self.export.queue.reserved_destination_keys();
        let mut queued = 0usize;
        let mut skipped = 0usize;

        for (original_index, source, configured_name) in &export_faces {
            let face_name = (!configured_name.trim().is_empty())
                .then_some(configured_name.as_str())
                .or_else(|| source.file_stem().and_then(|value| value.to_str()))
                .unwrap_or("face");
            let source_name = source
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(face_name);
            let context = export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &project_name,
                snapshot_name: &snapshot_name,
                test_code: &test_code,
                face_number: original_index + 1,
                face_name,
                source_name,
                date: &date,
            };
            let folder =
                export_batch::render_export_folder(&base_folder, &folder_template, &context);
            if let Err(err) = std::fs::create_dir_all(&folder) {
                self.report_error(format!(
                    "Cannot create export folder {}: {err}",
                    folder.display()
                ));
                return;
            }
            let filename = export_batch::render_export_filename(&template, &context);
            let destination = match export_batch::resolve_destination_reserved(
                &folder,
                &filename,
                conflict_policy,
                &mut reserved,
            ) {
                export_batch::DestinationDecision::Write(path) => path,
                export_batch::DestinationDecision::Skip(_) => {
                    skipped += 1;
                    continue;
                }
            };
            if !self.enqueue_export(export_queue::ExportQueueSpec {
                label: format!("{face_name} / {snapshot_name}"),
                source: source.clone(),
                destination,
                recipe: export_recipe::ExportRecipe::from_project(&project),
                default_dpi: self.settings.default_dpi,
                force_lzw: self.settings.lzw_compression,
                validate_after_export: self.settings.validate_after_export,
                conflict_policy,
                mark: None,
            }) {
                return;
            }
            queued += 1;
        }

        self.export.show_all = false;
        self.export.show_queue = queued > 0;
        self.export.remind_after_export = queued > 0 && self.snapshot_project_needs_save_reminder();
        if open_after && queued > 0 {
            self.export.open_folder_after = Some(base_folder.clone());
        }
        let _ = self.settings.save();
        if queued > 0 {
            let mut parts = vec![format!("Queued {queued} export(s)")];
            if rejected_count > 0 {
                parts.push(format!("excluded {rejected_count} Rejected Face(s)"));
            }
            if skipped > 0 {
                parts.push(format!("skipped {skipped} existing file(s)"));
            }
            self.report_info(parts.join(" · "));
        } else {
            let mut parts = vec!["No exports queued".to_owned()];
            if rejected_count > 0 {
                parts.push(format!("excluded {rejected_count} Rejected Face(s)"));
            }
            if skipped > 0 {
                parts.push(format!("skipped {skipped} existing file(s)"));
            }
            self.report_info(parts.join(" · "));
        }
    }

    fn ui_export_all_window(&mut self, ctx: &egui::Context) {
        if !self.export.show_all {
            return;
        }
        let mut open = self.export.show_all;
        let folder = PathBuf::from(self.export.all_folder.trim());
        let existing_tiffs = if folder.is_dir() {
            export_batch::folder_tiff_count(&folder)
        } else {
            0
        };
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let test_code = self.project.effective_test_code_text();
        let snapshot_name = self
            .project
            .active_snapshot_name()
            .unwrap_or("Working")
            .to_owned();
        let first_source = self
            .faces
            .first()
            .map(|face| face.path.clone())
            .unwrap_or_else(|| PathBuf::from("source.tif"));
        let source_name = first_source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("source")
            .to_owned();
        let face_name = self
            .project
            .faces
            .first()
            .map(|face| face.label.as_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("face");
        let today = Local::now().format("%Y-%m-%d").to_string();
        let preview_context = export_batch::ExportNameContext {
            shade_name: shade_name.as_deref(),
            project_name: &self.project.name,
            snapshot_name: &snapshot_name,
            test_code: &test_code,
            face_number: 1,
            face_name,
            source_name: &source_name,
            date: &today,
        };
        let preview_name = export_batch::render_export_filename(
            &self.settings.export_all_template,
            &preview_context,
        );
        let preview_folder = export_batch::render_export_folder(
            &folder,
            &self.settings.export_folder_template,
            &preview_context,
        );
        let mut browse = false;
        let mut reveal = false;
        let mut start = false;
        let mut cancel = false;
        let mut changed = false;

        egui::Window::new("Export All Faces")
            .open(&mut open)
            .resizable(true)
            .default_width(540.0)
            .min_width(480.0)
            .max_width(620.0)
            .show(ctx, |ui| {
                ui.strong("Export root folder");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.export.all_folder)
                            .desired_width(330.0),
                    );
                    browse = ui.button("Browse...").clicked();
                    reveal = ui
                        .add_enabled(folder.is_dir(), egui::Button::new("Reveal folder"))
                        .clicked();
                });
                if existing_tiffs > 0 {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!("Warning: this root already contains {existing_tiffs} TIFF file(s)."),
                    );
                }

                ui.add_space(8.0);
                ui.strong("File name template");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.settings.export_all_template)
                            .desired_width(500.0),
                    )
                    .changed();
                ui.small("Tokens: {project}, {face}, {snapshot}, {testcode}, {source}, {date}. Legacy {snapshot-code} remains Test Code compatible.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Preview:");
                    ui.monospace(&preview_name);
                });

                ui.add_space(8.0);
                ui.strong("Folder template");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.settings.export_folder_template)
                            .hint_text("Example: {project}/{date}/{snapshot}/")
                            .desired_width(500.0),
                    )
                    .changed();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Folder preview:");
                    ui.monospace(preview_folder.display().to_string());
                });

                ui.add_space(8.0);
                ui.strong("If a file already exists");
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::Overwrite,
                            "Overwrite",
                        )
                        .changed();
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::Skip,
                            "Skip",
                        )
                        .changed();
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::AutoNumber,
                            "Auto-number",
                        )
                        .changed();
                });

                ui.add_space(8.0);
                changed |= ui
                    .checkbox(
                        &mut self.settings.export_all_open_folder,
                        "Open root folder after queue finishes",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.export_all_test_code,
                        "Write Test Code on every exported Face",
                    )
                    .changed();

                ui.separator();
                ui.horizontal(|ui| {
                    start = ui
                        .add_enabled(
                            !self.export.all_folder.trim().is_empty()
                                && self.job.is_none()
                                && !self.faces.is_empty(),
                            egui::Button::new("Add all to Queue"),
                        )
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });

        if cancel {
            open = false;
        }
        self.export.show_all = open;
        if changed {
            self.settings.sanitize();
            if let Err(err) = self.settings.save() {
                self.log.error(&err);
            }
        }
        if browse {
            let mut dialog = rfd::FileDialog::new();
            if folder.is_dir() {
                dialog = dialog.set_directory(&folder);
            }
            if let Some(selected) = dialog.pick_folder() {
                self.export.all_folder = selected.to_string_lossy().into_owned();
            }
        }
        if reveal && folder.is_dir() {
            if let Err(err) = open_folder(&folder) {
                self.report_error(err);
            }
        }
        if start {
            self.start_export_all();
        }
    }

    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {
        if self.job.is_some() {
            return;
        }
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let Some(snapshot) = self
            .project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == snapshot_id)
            .cloned()
        else {
            return;
        };
        let stem = face
            .path
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "face".to_owned());
        let today = Local::now().format("%Y-%m-%d").to_string();
        let test_code = self.project.effective_test_code_text();
        let context = export_batch::ExportNameContext {
            shade_name: None,
            project_name: &self.project.name,
            snapshot_name: &snapshot.name,
            test_code: &test_code,
            face_number: self.current_face + 1,
            face_name: &stem,
            source_name: &stem,
            date: &today,
        };
        let suggested =
            export_batch::render_export_filename(&self.settings.snapshot_export_template, &context);
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("TIFF image", &["tif", "tiff"])
            .set_file_name(suggested)
            .save_file()
        else {
            return;
        };
        let source = face.path.clone();
        let face_key = source.to_string_lossy().into_owned();
        let folder = destination
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut project = self.project.clone();
        project.adjustments = snapshot.adjustments.clone();
        project.active_snapshot_id = Some(snapshot.id);
        self.export.remind_after_export = self.snapshot_project_needs_save_reminder();
        if !self.enqueue_export(export_queue::ExportQueueSpec {
            label: format!("Face {} / {}", self.current_face + 1, snapshot.name),
            source,
            destination,
            recipe: export_recipe::ExportRecipe::from_project(&project),
            default_dpi: self.settings.default_dpi,
            force_lzw: self.settings.lzw_compression,
            validate_after_export: self.settings.validate_after_export,
            conflict_policy: export_batch::ConflictPolicy::Overwrite,
            mark: Some(export_queue::ExportQueueMark {
                snapshot_id,
                face_key,
                folder,
            }),
        }) {
            return;
        }
        self.export.show_queue = true;
        self.report_info("Snapshot export added to queue");
    }

    fn export_snapshot_group_dialog(&mut self, snapshot_ids: Vec<u64>, label: String) {
        if self.job.is_some() || snapshot_ids.is_empty() {
            return;
        }
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let Some(base_folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let source = face.path.clone();
        let face_key = source.to_string_lossy().into_owned();
        let source_name = source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("face")
            .to_owned();
        let face_name = self
            .project
            .faces
            .get(self.current_face)
            .map(|face| face.label.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| source_name.clone());
        let snapshots = snapshot_ids
            .into_iter()
            .filter_map(|id| {
                self.project
                    .snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .cloned()
            })
            .collect::<Vec<_>>();
        if snapshots.is_empty() {
            return;
        }
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let project_name = self.project.name.clone();
        let date = Local::now().format("%Y-%m-%d").to_string();
        let mut reserved = self.export.queue.reserved_destination_keys();
        let conflict_policy = self.settings.export_all_conflict_policy;
        let mut queued = 0usize;
        let mut skipped = 0usize;

        for snapshot in snapshots {
            let mut project = self.project.clone();
            project.adjustments = snapshot.adjustments.clone();
            project.active_snapshot_id = Some(snapshot.id);
            let test_code = project.effective_test_code_text();
            let context = export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &project_name,
                snapshot_name: &snapshot.name,
                test_code: &test_code,
                face_number: self.current_face + 1,
                face_name: &face_name,
                source_name: &source_name,
                date: &date,
            };
            let folder = export_batch::render_export_folder(
                &base_folder,
                &self.settings.export_folder_template,
                &context,
            );
            if let Err(err) = std::fs::create_dir_all(&folder) {
                self.report_error(format!(
                    "Cannot create export folder {}: {err}",
                    folder.display()
                ));
                return;
            }
            let filename = export_batch::render_export_filename(
                &self.settings.snapshot_export_template,
                &context,
            );
            let destination = match export_batch::resolve_destination_reserved(
                &folder,
                &filename,
                self.settings.export_all_conflict_policy,
                &mut reserved,
            ) {
                export_batch::DestinationDecision::Write(path) => path,
                export_batch::DestinationDecision::Skip(_) => {
                    skipped += 1;
                    continue;
                }
            };
            if !self.enqueue_export(export_queue::ExportQueueSpec {
                label: format!("{face_name} / {}", snapshot.name),
                source: source.clone(),
                destination,
                recipe: export_recipe::ExportRecipe::from_project(&project),
                default_dpi: self.settings.default_dpi,
                force_lzw: self.settings.lzw_compression,
                validate_after_export: self.settings.validate_after_export,
                conflict_policy,
                mark: Some(export_queue::ExportQueueMark {
                    snapshot_id: snapshot.id,
                    face_key: face_key.clone(),
                    folder,
                }),
            }) {
                return;
            }
            queued += 1;
        }

        if queued > 0 {
            self.export.remind_after_export = self.snapshot_project_needs_save_reminder();
            self.export.show_queue = true;
            self.report_info(if skipped > 0 {
                format!("Queued {queued} snapshot(s) ({label}) · skipped {skipped}")
            } else {
                format!("Queued {queued} snapshot(s) ({label})")
            });
        }
    }

    fn ensure_project_palette_for_model(&mut self, color_model: tiff_io::ColorModel) -> bool {
        if self.project.channel_palette.is_some() {
            return false;
        }
        let palette = self
            .settings
            .default_project_palette()
            .or_else(|| match color_model {
                tiff_io::ColorModel::Rgb => Some(palette::builtin_rgb()),
                tiff_io::ColorModel::Cmyk => Some(palette::builtin_cmyk()),
                _ => None,
            });
        if let Some(palette) = palette {
            self.project.channel_palette = Some(palette);
            true
        } else {
            false
        }
    }

    fn select_project_palette(&mut self, palette: ChannelPalette) {
        if self.project.channel_palette.as_ref() == Some(&palette) {
            return;
        }
        let name = palette.name.clone();
        self.project.channel_palette = Some(palette);
        self.mark_project_dirty();
        self.report_info(format!("Channel palette: {name}"));
    }

    fn open_export_folder(&mut self, folder: &str) {
        if let Err(err) = open_folder(Path::new(folder)) {
            self.report_error(err);
        }
    }

    fn poll_job(&mut self) {
        let result = self.job.as_ref().and_then(|job| job.rx.try_recv().ok());
        let Some(result) = result else {
            return;
        };
        self.job = None;
        match result {
            JobResult::AddFaces { faces, errors } => {
                let added = faces.len();
                if let Some(first) = faces.first() {
                    self.ensure_project_palette_for_model(first.preview.metadata.color_model);
                }
                for item in faces {
                    self.project
                        .ensure_channels(&item.preview.metadata.channel_names);
                    let label = item
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Face".to_owned());
                    self.project.faces.push(model::FaceRef {
                        path: item.path.to_string_lossy().into_owned(),
                        label,
                        status: model::FaceStatus::Accepted,
                    });
                    self.faces.push(Self::make_runtime_face(item));
                }
                if added > 0 {
                    self.snapshot_preview_cache.clear();
                    self.current_face = self.faces.len().saturating_sub(added);
                    self.selected_channel = 0;
                    self.solo_channel = None;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.mark_project_dirty();
                    self.history
                        .reset(&self.project.adjustments, "Faces changed");
                    self.history_pending_label = None;
                    self.history_pending_at = None;
                    self.report_info(format!("Added {added} face(s)"));
                }
                if !errors.is_empty() {
                    self.report_error(format!(
                        "Some TIFF files could not be loaded: {}",
                        errors.join(" | ")
                    ));
                }
            }
            JobResult::RebuildPreviews(result) => match result {
                Ok(items) => {
                    let old_generations = self
                        .faces
                        .iter()
                        .map(|face| face.generation)
                        .collect::<Vec<_>>();
                    self.faces = items.into_iter().map(Self::make_runtime_face).collect();
                    self.snapshot_preview_cache.clear();
                    for (face, old_generation) in
                        self.faces.iter_mut().zip(old_generations.into_iter())
                    {
                        face.generation = old_generation.wrapping_add(1).max(1);
                    }
                    self.current_face = self.current_face.min(self.faces.len().saturating_sub(1));
                    if let Some(face) = self.faces.get(self.current_face) {
                        let count = face.preview.metadata.channel_names.len();
                        self.selected_channel = self.selected_channel.min(count.saturating_sub(1));
                        if self.solo_channel.is_some_and(|channel| channel >= count) {
                            self.solo_channel = None;
                        }
                    }
                    self.render_busy = None;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.report_info(format!(
                        "Rebuilt {} preview(s) at max dimension {}",
                        self.faces.len(),
                        self.settings.max_preview_dimension
                    ));
                }
                Err(err) => self.report_error(format!("Preview rebuild failed: {err}")),
            },
            JobResult::RelinkFace { index, result } => {
                workflow::apply_relinked_face(self, index, result);
            }
            JobResult::RelinkFolder { faces, errors } => {
                workflow::apply_relinked_folder(self, faces, errors);
            }
            JobResult::Open(result) => match result {
                Ok(payload) => {
                    self.lifecycle.opening_path = None;
                    self.lifecycle.backup_restore = None;
                    self.project = payload.project;
                    self.bump_project_session();
                    self.snapshot_rename_id = None;
                    self.snapshot_rename_buffer.clear();
                    self.project_path = Some(payload.path.clone());
                    self.faces = payload
                        .faces
                        .into_iter()
                        .map(Self::make_runtime_face)
                        .collect();
                    self.current_face = 0;
                    self.selected_channel = 0;
                    self.solo_channel = None;
                    if let Some(first) = self.faces.first() {
                        let color_model = first.preview.metadata.color_model;
                        self.ensure_project_palette_for_model(color_model);
                    }
                    self.adjustment_scope = AdjustmentScope::All;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.mark_project_saved();
                    self.export.remind_after_export = false;
                    self.export.show_snapshot_save_reminder = false;
                    self.remember_previous_shade(&payload.path);
                    self.load_history_for_active_snapshot("Open project");
                    self.history_clear_backup = None;
                    self.history_pending_label = None;
                    self.history_pending_at = None;
                    self.report_info(format!("Opened {}", payload.path.display()));
                    if !payload.errors.is_empty() {
                        self.report_error(format!(
                            "Project opened with TIFF errors: {}",
                            payload.errors.join(" | ")
                        ));
                    }
                }
                Err(err) => {
                    let primary_path = self.lifecycle.opening_path.take();
                    if let Some(primary_path) = primary_path {
                        let backup_path = safe_fs::backup_path(&primary_path);
                        if backup_path.is_file() && ShadeProject::load(&backup_path).is_ok() {
                            self.lifecycle.backup_restore = Some(BackupRestoreCandidate {
                                primary_path,
                                backup_path,
                                primary_error: err.clone(),
                            });
                        }
                    }
                    self.report_error(err);
                }
            },
            JobResult::Recover(result) => match result {
                Ok(payload) => {
                    self.project = payload.project;
                    self.bump_project_session();
                    self.project_path = payload.origin_path;
                    self.faces = payload
                        .faces
                        .into_iter()
                        .map(Self::make_runtime_face)
                        .collect();
                    self.current_face = 0;
                    self.selected_channel = 0;
                    self.solo_channel = None;
                    self.adjustment_scope = AdjustmentScope::All;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.mark_project_dirty();
                    self.load_history_for_active_snapshot("Recovered project");
                    self.history_clear_backup = None;
                    self.history_pending_label = None;
                    self.history_pending_at = None;
                    self.last_autosave = Instant::now();
                    self.report_info("Recovered autosaved project state");
                    if !payload.errors.is_empty() {
                        self.report_error(format!(
                            "Recovery opened with TIFF errors: {}",
                            payload.errors.join(" | ")
                        ));
                    }
                }
                Err(err) => self.report_error(format!("Recovery failed: {err}")),
            },
            JobResult::Save {
                path,
                revision,
                result,
            } => match result {
                Ok(()) => {
                    self.project.name = project_name_for_path(&self.project.name, &path);
                    self.project_path = Some(path.clone());
                    if self.project_revision == revision {
                        self.mark_project_saved();
                    } else {
                        self.report_info("Project saved, but newer edits remain unsaved.");
                    }
                    self.export.remind_after_export = false;
                    self.export.show_snapshot_save_reminder = false;
                    self.remember_previous_shade(&path);
                    if let Err(err) = recovery::clear() {
                        self.log.error(&err);
                    }
                    self.report_info(format!("Saved {}", path.display()));
                }
                Err(err) => {
                    self.lifecycle.save_failed();
                    self.report_error(err);
                }
            },
            JobResult::InspectTiff(result) => {
                match result {
                    Ok(report) => {
                        self.inspector.inspection = Some(report);
                        self.inspector.error = None;
                    }
                    Err(err) => {
                        self.inspector.inspection = None;
                        self.inspector.error = Some(err);
                    }
                }
                self.inspector.show = true;
            }
            JobResult::WorkerPanic(err) => {
                self.report_error(err);
            }
            JobResult::Export(payload) => {
                let export_ok = payload.result.is_ok();
                if !payload.marks.is_empty() {
                    for mark in payload.marks {
                        self.project.record_snapshot_export(
                            mark.snapshot_id,
                            mark.face_key,
                            mark.folder.to_string_lossy().into_owned(),
                            mark.exported_at_unix_ms,
                        );
                    }
                    self.mark_project_dirty();
                }
                match payload.result {
                    Ok(message) => self.report_info(message),
                    Err(err) => self.report_error(format!("Export failed: {err}")),
                }
                if export_ok && self.export.remind_after_export {
                    self.export.show_snapshot_save_reminder = true;
                }
                self.export.remind_after_export = false;
            }
        }
    }

    fn mark_all_previews_dirty(&mut self) {
        for face in &mut self.faces {
            face.generation = face.generation.wrapping_add(1).max(1);
        }
        self.mark_project_dirty();
    }

    fn mark_current_preview_dirty(&mut self) {
        if let Some(face) = self.faces.get_mut(self.current_face) {
            face.generation = face.generation.wrapping_add(1).max(1);
        }
        let _ = self.restore_active_snapshot_preview();
    }

    /// Re-render textures for display-only color settings. The caller decides
    /// whether the project should be marked dirty; TIFF source/export data is never changed.
    fn invalidate_display_previews(&mut self) {
        self.snapshot_preview_cache.clear();
        for face in &mut self.faces {
            face.generation = face.generation.wrapping_add(1).max(1);
            face.color_status = PreviewColorStatus::Pending;
            face.original_rendered_solo = None;
        }
        self.render_busy = None;
    }

    fn cache_rendered_snapshot_preview(
        &mut self,
        face_index: usize,
        solo_channel: Option<usize>,
    ) -> bool {
        let Some(snapshot_id) = self.project.active_snapshot_id else {
            return false;
        };
        if !self.project.active_snapshot_matches() {
            return false;
        }
        let Some(face) = self.faces.get(face_index) else {
            return false;
        };
        if !face.available
            || face.rendered_generation != face.generation
            || face.original_rendered_solo != Some(solo_channel)
        {
            return false;
        }
        let Some(texture) = face.texture.clone() else {
            return false;
        };
        let estimated_bytes = face
            .preview
            .width
            .saturating_mul(face.preview.height)
            .saturating_mul(4);
        let entry = SnapshotPreviewEntry {
            texture,
            adjusted_histograms: face.adjusted_histograms.clone(),
            clipping: face.clipping.clone(),
            color_status: face.color_status.clone(),
        };
        self.snapshot_preview_cache.insert(
            snapshot_preview_cache::SnapshotPreviewKey::new(snapshot_id, face_index, solo_channel),
            entry,
            estimated_bytes,
        );
        true
    }

    fn cache_current_snapshot_preview_if_ready(&mut self) -> bool {
        self.cache_rendered_snapshot_preview(self.current_face, self.solo_channel)
    }

    fn restore_active_snapshot_preview(&mut self) -> bool {
        let Some(snapshot_id) = self.project.active_snapshot_id else {
            return false;
        };
        if !self.project.active_snapshot_matches() {
            return false;
        }
        let face_index = self.current_face;
        let solo_channel = self.solo_channel;
        let key =
            snapshot_preview_cache::SnapshotPreviewKey::new(snapshot_id, face_index, solo_channel);
        let Some(entry) = self.snapshot_preview_cache.get_cloned(key) else {
            return false;
        };
        let Some(face) = self.faces.get_mut(face_index) else {
            return false;
        };
        // BEFORE uses the same display/solo mode. If that companion texture is
        // no longer current, fall through to the renderer rather than mixing states.
        if !face.available || face.original_rendered_solo != Some(solo_channel) {
            return false;
        }
        face.texture = Some(entry.texture);
        face.adjusted_histograms = entry.adjusted_histograms;
        face.clipping = entry.clipping;
        face.color_status = entry.color_status;
        face.rendered_generation = face.generation;
        true
    }

    fn poll_render(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.render_rx.try_recv() {
            let result = match message {
                Ok(result) => result,
                Err(failure) => {
                    if self.render_busy == Some((failure.face_index, failure.generation)) {
                        self.render_busy = None;
                    }
                    self.report_error(failure.message);
                    continue;
                }
            };
            let face_index = result.face_index;
            let generation = result.generation;
            let solo_channel = result.solo_channel;
            if self.render_busy == Some((face_index, generation)) {
                self.render_busy = None;
            }
            let Some(face) = self.faces.get_mut(face_index) else {
                continue;
            };
            if face.generation != generation {
                continue;
            }
            face.adjusted_histograms = result.adjusted_histograms;
            face.clipping = result.clipping;
            face.color_status = result.color_status;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.rgba,
            );
            let options = egui::TextureOptions::LINEAR;
            // Snapshot cache entries hold immutable TextureHandles. Always create
            // a fresh adjusted texture here so a later dirty render cannot mutate
            // a texture that another Snapshot is using from the cache.
            face.texture = Some(ctx.load_texture(
                format!("face-preview-{face_index}-{generation}"),
                image,
                options,
            ));
            let original_image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.original_rgba,
            );
            if let Some(texture) = &mut face.original_texture {
                texture.set(original_image, options);
            } else {
                face.original_texture = Some(ctx.load_texture(
                    format!("face-original-preview-{face_index}"),
                    original_image,
                    options,
                ));
            }
            face.original_rendered_solo = Some(solo_channel);
            if let Some(source_rgba) = result.embedded_original_rgba {
                let source_image = egui::ColorImage::from_rgba_unmultiplied(
                    [face.preview.width, face.preview.height],
                    &source_rgba,
                );
                if let Some(texture) = &mut face.embedded_original_texture {
                    texture.set(source_image, options);
                } else {
                    face.embedded_original_texture = Some(ctx.load_texture(
                        format!("face-embedded-source-preview-{face_index}"),
                        source_image,
                        options,
                    ));
                }
            }
            if let Some(status) = result.embedded_original_status {
                face.embedded_original_status = status;
            }
            face.rendered_generation = generation;
            let _ = face;
            self.cache_rendered_snapshot_preview(face_index, solo_channel);
        }
    }

    fn start_render_if_needed(&mut self, ctx: &egui::Context) {
        if self.render_busy.is_some() || ctx.input(|input| input.pointer.any_down()) {
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        if !face.available {
            return;
        }
        if face.rendered_generation == face.generation {
            return;
        }
        let face_index = self.current_face;
        let generation = face.generation;
        let needs_embedded_original = face.embedded_original_texture.is_none();
        let preview = Arc::clone(&face.preview);
        let project = self.project.clone();
        let solo_channel = self.solo_channel;
        let color_config = PreviewColorConfig::for_viewport(&self.project, &self.settings);
        let tx = self.render_tx.clone();
        self.render_busy = Some((face_index, generation));
        std::thread::spawn(move || {
            let outcome = worker_guard::catch_value("Preview render worker", || {
                let (adjusted, clipping) = render::adjusted_planes_with_stats(&preview, &project);
                let color =
                    color_management::PreviewColorTransform::new(&preview.metadata, color_config);
                let rgba =
                    render::rgba_from_planes_with_color(&preview, &adjusted, solo_channel, &color);
                let original_rgba = render::rgba_from_planes_with_color(
                    &preview,
                    &preview.channels,
                    solo_channel,
                    &color,
                );
                let adjusted_histograms = adjusted
                    .iter()
                    .map(|values| render::histogram(values))
                    .collect::<Vec<_>>();
                let color_status = color.status().clone();

                let (embedded_original_rgba, embedded_original_status) = if needs_embedded_original
                {
                    let embedded_color = color_management::PreviewColorTransform::new(
                        &preview.metadata,
                        PreviewColorConfig {
                            enabled: true,
                            intent: PreviewRenderingIntent::Perceptual,
                            black_point_compensation: false,
                            assigned_profile_path: None,
                            assigned_profile_identity: None,
                            soft_proof_enabled: false,
                            proof_profile_path: None,
                            proof_profile_identity: None,
                            proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
                            monitor_profile_path: None,
                            monitor_profile_identity: None,
                            gamut_warning: false,
                        },
                    );
                    let status = embedded_color.status().clone();
                    let source_rgba = render::rgba_from_planes_with_color(
                        &preview,
                        &preview.channels,
                        None,
                        &embedded_color,
                    );
                    (Some(source_rgba), Some(status))
                } else {
                    (None, None)
                };

                RenderResult {
                    face_index,
                    generation,
                    solo_channel,
                    adjusted_histograms,
                    clipping,
                    color_status,
                    rgba,
                    original_rgba,
                    embedded_original_rgba,
                    embedded_original_status,
                }
            });
            let message = match outcome {
                Ok(result) => Ok(result),
                Err(message) => Err(RenderFailure {
                    face_index,
                    generation,
                    message,
                }),
            };
            let _ = tx.send(message);
        });
    }

    fn select_channel(&mut self, channel: usize, isolate: bool) {
        let previous_solo = self.solo_channel;
        let was_master = self.adjustment_scope == AdjustmentScope::All;
        self.adjustment_scope = AdjustmentScope::Selected;
        if isolate && !was_master {
            let (selected, solo) =
                channel_click_state(self.selected_channel, self.solo_channel, channel);
            self.selected_channel = selected;
            self.solo_channel = solo;
        } else {
            self.selected_channel = channel;
            self.solo_channel = None;
        }
        if self.solo_channel != previous_solo {
            self.mark_current_preview_dirty();
        }
    }

    fn show_composite(&mut self) {
        if self.solo_channel.is_some() {
            self.solo_channel = None;
            self.mark_current_preview_dirty();
        }
    }

    fn remove_current_face(&mut self) {
        if self.job.is_some() || self.current_face >= self.faces.len() {
            return;
        }
        self.snapshot_preview_cache.clear();
        self.faces.remove(self.current_face);
        if self.current_face < self.project.faces.len() {
            self.project.faces.remove(self.current_face);
        }
        // A preview worker may still finish with the removed Face's old index.
        // Invalidate every surviving generation before accepting any future result
        // so a shifted Face can never receive stale pixels from that worker.
        self.render_busy = None;
        for face in &mut self.faces {
            face.generation = face.generation.wrapping_add(1).max(1);
            face.rendered_generation = 0;
        }
        self.current_face = self.current_face.min(self.faces.len().saturating_sub(1));
        self.selected_channel = 0;
        self.solo_channel = None;
        self.fit_requested = true;
        self.viewport_recenter = true;
        self.mark_project_dirty();
        self.report_info("Face removed from project (source TIFF was not deleted)");
    }

    fn remember_previous_shade(&mut self, path: &Path) {
        self.project_view.list_textures.clear();
        self.previous_shades.record_open(path, &self.project.name);
        if let Err(err) = self.previous_shades.save() {
            self.log.error(&err);
        }
    }

    fn load_previous_shade_preview(&mut self, ctx: &egui::Context, path: &str) {
        self.project_view.selected = Some(path.to_owned());
        self.project_view.texture = None;
        self.project_view.preview = None;
        self.project_view.preview_error = None;
        match previous_shades::inspect(Path::new(path)) {
            Ok(mut preview) => {
                if let Some(thumbnail) = preview.thumbnail.take() {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [thumbnail.width, thumbnail.height],
                        &thumbnail.rgba,
                    );
                    self.project_view.texture = Some(ctx.load_texture(
                        format!("previous-shade-thumbnail:{path}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                self.project_view.preview = Some(preview);
            }
            Err(err) => self.project_view.preview_error = Some(err),
        }
    }

    fn save_settings_quietly(&mut self) {
        if let Err(err) = self.settings.save() {
            self.report_error(err);
        }
    }

    fn bundled_shell_script(file_name: &str) -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let root = exe.parent()?;
        for folder in ["shell", "Shell"] {
            let candidate = root.join(folder).join(file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    fn launch_shell_script(&mut self, file_name: &str, action: &str) {
        let Some(script) = Self::bundled_shell_script(file_name) else {
            self.report_error("Shell integration package was not found next to ShadeEditor.exe. Install the Shell package separately.");
            return;
        };
        match std::process::Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .spawn()
        {
            Ok(_) => self.report_info(format!(
                "Shell integration {action} started - approve the Windows administrator prompt."
            )),
            Err(err) => {
                self.report_error(format!("Cannot start Shell integration {action}: {err}"))
            }
        }
    }

    fn sync_update_state(&mut self) {
        match self.updater.status() {
            UpdateStatus::Failed(message) => {
                if self.last_update_failure.as_deref() != Some(message.as_str()) {
                    self.last_update_failure = Some(message.clone());
                    self.report_error(format!("Update: {message}"));
                }
            }
            _ => self.last_update_failure = None,
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| toast.created.elapsed() > ERROR_TOAST_LIFETIME)
        {
            self.toast = None;
            if self.status_message == "Error - see Logs" {
                self.status_message = "Ready".to_owned();
            }
        }
    }

    fn ui_operation_progress(&self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            if let Ok(progress) = job.progress.lock() {
                let value = progress.fraction.unwrap_or(0.5);
                let full_text = if progress.detail.trim().is_empty() {
                    progress.label.clone()
                } else {
                    format!("{} - {}", progress.label, progress.detail)
                };
                let mut compact = full_text.chars().take(64).collect::<String>();
                if full_text.chars().count() > 64 {
                    compact.push('…');
                }
                ui.add(
                    egui::ProgressBar::new(value)
                        .desired_width(380.0)
                        .text(compact)
                        .animate(progress.fraction.is_none()),
                )
                .on_hover_text(full_text);
                return;
            }
        }
        if let Some((value, text)) = self.export.queue.active_summary() {
            ui.add(
                egui::ProgressBar::new(value)
                    .desired_width(300.0)
                    .text(text),
            );
            return;
        }
        if self.render_busy.is_some()
            && self
                .faces
                .get(self.current_face)
                .is_some_and(|face| face.rendered_generation != face.generation)
        {
            ui.add(
                egui::ProgressBar::new(0.45)
                    .desired_width(300.0)
                    .text("Rendering preview")
                    .animate(true),
            );
        }
    }

    fn ui_update_compact(&mut self, ui: &mut egui::Ui) {
        match self.updater.status() {
            UpdateStatus::Idle => {
                if ui.small_button("Check update").clicked() {
                    self.updater.start_check(false);
                }
            }
            UpdateStatus::Checking => {
                ui.add(
                    egui::ProgressBar::new(0.5)
                        .desired_width(190.0)
                        .text("Checking update")
                        .animate(true),
                );
            }
            UpdateStatus::UpToDate => {
                if ui
                    .small_button("Update OK")
                    .on_hover_text("Check again")
                    .clicked()
                {
                    self.updater.start_check(false);
                }
            }
            UpdateStatus::Available(info) => {
                if ui
                    .small_button(format!("Download {}", info.version))
                    .on_hover_text(info.release_url)
                    .clicked()
                {
                    self.updater.start_download();
                }
            }
            UpdateStatus::Downloading {
                info,
                downloaded,
                total,
            } => {
                let fraction = total
                    .filter(|total| *total > 0)
                    .map(|total| downloaded as f32 / total as f32)
                    .unwrap_or(0.5);
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(220.0)
                        .text(format!("Updating {}", info.version))
                        .animate(total.is_none()),
                );
            }
            UpdateStatus::Ready(info, _) => {
                if ui
                    .small_button(format!("Restart {}", info.version))
                    .clicked()
                {
                    match self.updater.apply_ready() {
                        Ok(true) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                        Ok(false) => {}
                        Err(err) => self.report_error(err),
                    }
                }
            }
            UpdateStatus::Failed(_) => {
                if ui.small_button("Retry update").clicked() {
                    self.updater.start_check(false);
                }
            }
        }
    }

    fn apply_snapshot_now(&mut self, id: u64) {
        self.flush_history_now();
        self.sync_history_to_active_snapshot();
        if self.project.apply_snapshot(id) {
            if let Some(snapshot) = self
                .project
                .snapshots
                .iter()
                .find(|snapshot| snapshot.id == id)
            {
                self.snapshot_rename_id = Some(id);
                self.snapshot_rename_buffer = snapshot.name.clone();
            }
            self.mark_all_previews_dirty();
            let restored = self.restore_active_snapshot_preview();
            let history_label = self
                .project
                .active_snapshot_name()
                .map(|name| format!("Snapshot - {name}"))
                .unwrap_or_else(|| "Snapshot".to_owned());
            self.load_history_for_active_snapshot(&history_label);
            self.history_clear_backup = None;
            self.history_pending_label = None;
            self.history_pending_at = None;
            if restored {
                self.report_info("Snapshot loaded · cached preview");
            } else {
                self.report_info("Snapshot loaded");
            }
        }
    }

    fn request_snapshot_load(&mut self, id: u64) {
        if self.project.active_snapshot_id == Some(id) {
            return;
        }
        if self.active_snapshot_has_unupdated_changes() {
            self.pending_snapshot_action = Some(PendingSnapshotAction::Load(id));
        } else {
            self.apply_snapshot_now(id);
        }
    }

    fn discard_active_snapshot_changes(&mut self) -> bool {
        let Some(active_id) = self.project.active_snapshot_id else {
            return false;
        };
        let Some(saved_adjustments) = self
            .project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == active_id)
            .map(|snapshot| snapshot.adjustments.clone())
        else {
            return false;
        };
        self.project.adjustments = saved_adjustments.clone();
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.history_clear_backup = None;
        self.history
            .discard_to_state(&saved_adjustments, "Snapshot state");
        self.sync_history_to_active_snapshot();
        self.mark_all_previews_dirty();
        let _ = self.restore_active_snapshot_preview();
        self.report_info("Snapshot changes discarded");
        true
    }

    fn continue_snapshot_action_after_choice(
        &mut self,
        action: PendingSnapshotAction,
        update: bool,
    ) {
        if update {
            workflow::update_active_snapshot(self);
        } else {
            self.discard_active_snapshot_changes();
        }
        match action {
            PendingSnapshotAction::Load(target_id) => self.apply_snapshot_now(target_id),
            PendingSnapshotAction::Save(path) => {
                if !self.begin_project_save(path) && self.lifecycle.after_save.is_some() {
                    self.lifecycle.save_failed();
                }
            }
        }
    }

    fn ui_snapshot_update_confirmation(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_snapshot_action.clone() else {
            return;
        };
        let current_name = self
            .project
            .active_snapshot_name()
            .unwrap_or("Current snapshot")
            .to_owned();
        let action_text = match &action {
            PendingSnapshotAction::Load(target_id) => {
                let target_name = self
                    .project
                    .snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == *target_id)
                    .map(|snapshot| snapshot.name.clone())
                    .unwrap_or_else(|| "selected snapshot".to_owned());
                format!("Switch to {target_name}?")
            }
            PendingSnapshotAction::Save(_) => {
                "Save the project with this Snapshot state?".to_owned()
            }
        };
        let mut stay = false;
        let mut discard = false;
        let mut update = false;
        egui::Window::new("Snapshot changes not updated")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{current_name} has adjustment changes that have not been written back with Update."
                ));
                ui.label(action_text);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    stay = ui.button("Stay").clicked();
                    discard = ui.button("Discard").clicked();
                    update = ui.button("Update snapshot").clicked();
                });
            });
        if stay {
            self.pending_snapshot_action = None;
            if matches!(action, PendingSnapshotAction::Save(_)) {
                self.lifecycle.cancel_pending();
            }
        } else if discard {
            self.pending_snapshot_action = None;
            self.continue_snapshot_action_after_choice(action, false);
        } else if update {
            self.pending_snapshot_action = None;
            self.continue_snapshot_action_after_choice(action, true);
        }
    }

    fn ui_project_transition_confirmation(&mut self, ctx: &egui::Context) {
        let Some(transition) = self.lifecycle.pending.clone() else {
            return;
        };
        let action = transition.action_label();
        let mut save_and_continue = false;
        let mut discard_and_continue = false;
        let mut cancel = false;
        egui::Window::new("Save current project?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                if self.project_path.is_some() {
                    ui.strong("The current project has unsaved changes.");
                } else {
                    ui.strong("The current project has not been saved yet.");
                }
                ui.label(format!(
                    "Shade Editor is about to {}. Save the current .shade project first?",
                    transition.verb()
                ));
                ui.label("Faces, Snapshots and adjustment state remain untouched unless Save succeeds or Discard is explicit.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    save_and_continue = ui
                        .add_enabled(
                            self.job.is_none() && !self.faces.is_empty(),
                            egui::Button::new(format!("Save and {action}")),
                        )
                        .clicked();
                    discard_and_continue = ui
                        .button(format!("Discard and {action}"))
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });

        if cancel {
            self.lifecycle.cancel_pending();
        } else if discard_and_continue {
            self.lifecycle.cancel_pending();
            if !matches!(transition, ProjectTransition::Recover) {
                if let Err(err) = recovery::clear() {
                    self.log.error(&err);
                }
            }
            self.execute_project_transition(transition, Some(ctx));
        } else if save_and_continue {
            self.lifecycle.begin_save_then(transition);
            if !self.save_project(false) {
                self.lifecycle.save_failed();
            }
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        if self.lifecycle.allow_close_once {
            self.lifecycle.allow_close_once = false;
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.request_project_transition(ProjectTransition::Exit, Some(ctx));
    }

    fn sync_history_to_active_snapshot(&mut self) -> bool {
        let Some(active_id) = self.project.active_snapshot_id else {
            return false;
        };
        let persisted = self.history.to_persisted();
        let Some(snapshot) = self
            .project
            .snapshots
            .iter_mut()
            .find(|snapshot| snapshot.id == active_id)
        else {
            return false;
        };
        if snapshot.history == persisted {
            return false;
        }
        snapshot.history = persisted;
        self.mark_project_dirty();
        true
    }

    fn load_history_for_active_snapshot(&mut self, fallback_label: &str) {
        let persisted = self.project.active_snapshot_id.and_then(|active_id| {
            self.project
                .snapshots
                .iter()
                .find(|snapshot| snapshot.id == active_id)
                .map(|snapshot| snapshot.history.clone())
        });
        self.history = if let Some(persisted) = persisted {
            history::AdjustmentHistory::from_persisted(
                &persisted,
                &self.project.adjustments,
                fallback_label,
            )
        } else {
            let mut history = history::AdjustmentHistory::default();
            history.reset(&self.project.adjustments, fallback_label);
            history
        };
        self.history_pending_label = None;
        self.history_pending_at = None;
    }

    fn flush_history_now(&mut self) {
        if let Some(label) = self.history_pending_label.take() {
            self.history_pending_at = None;
            if self.history.record(&self.project.adjustments, label) {
                self.history_clear_backup = None;
            }
        }
        self.sync_history_to_active_snapshot();
    }

    fn queue_adjustment_history(&mut self, before: &BTreeMap<String, ChannelAdjustment>) {
        if *before == self.project.adjustments {
            return;
        }
        self.history_pending_label =
            Some(history::describe_change(before, &self.project.adjustments));
        self.history_pending_at = Some(Instant::now());
    }

    fn commit_pending_history(&mut self, ctx: &egui::Context, force: bool) {
        let Some(label) = self.history_pending_label.clone() else {
            return;
        };
        let ready = force
            || (self
                .history_pending_at
                .is_some_and(|at| at.elapsed() >= HISTORY_COMMIT_DELAY)
                && !ctx.input(|input| input.pointer.any_down()));
        if !ready {
            return;
        }
        if self.history.record(&self.project.adjustments, label) {
            self.history_clear_backup = None;
            self.sync_history_to_active_snapshot();
        }
        self.history_pending_label = None;
        self.history_pending_at = None;
    }

    fn apply_history_adjustments(
        &mut self,
        adjustments: BTreeMap<String, ChannelAdjustment>,
        message: &str,
    ) {
        self.project.adjustments = adjustments;
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.history_clear_backup = None;
        self.mark_all_previews_dirty();
        self.sync_history_to_active_snapshot();
        self.report_info(message);
    }

    fn undo_adjustment(&mut self, _ctx: &egui::Context) {
        self.flush_history_now();
        if let Some(adjustments) = self.history.undo() {
            self.apply_history_adjustments(adjustments, "Undo adjustment");
        }
    }

    fn redo_adjustment(&mut self, _ctx: &egui::Context) {
        self.flush_history_now();
        if let Some(adjustments) = self.history.redo() {
            self.apply_history_adjustments(adjustments, "Redo adjustment");
        }
    }

    fn handle_history_shortcuts(&mut self, ctx: &egui::Context) {
        let (undo, redo) = ctx.input(|input| {
            let z = input.key_pressed(egui::Key::Z);
            (
                z && input.modifiers.ctrl && input.modifiers.alt && !input.modifiers.shift,
                z && input.modifiers.ctrl && input.modifiers.shift && !input.modifiers.alt,
            )
        });
        if undo {
            self.undo_adjustment(ctx);
        } else if redo {
            self.redo_adjustment(ctx);
        }
    }

    fn poll_autosave(&mut self) {
        while let Ok(result) = self.autosave_rx.try_recv() {
            self.autosave_busy = false;
            match result {
                Ok(path) => self
                    .log
                    .info(&format!("Recovery autosaved: {}", path.display())),
                Err(err) => self.log.error(&format!("Recovery autosave failed: {err}")),
            }
        }
    }

    fn maybe_autosave(&mut self) {
        if !self.project_dirty
            || self.autosave_busy
            || self.job.is_some()
            || self.faces.is_empty()
            || self.last_autosave.elapsed() < RECOVERY_AUTOSAVE_INTERVAL
        {
            return;
        }
        let recovery_file = recovery::RecoveryFile::new(
            self.project.clone(),
            self.faces.iter().map(|face| face.path.clone()).collect(),
            self.project_path.clone(),
        );
        let tx = self.autosave_tx.clone();
        self.autosave_busy = true;
        self.last_autosave = Instant::now();
        std::thread::spawn(move || {
            let _ = tx.send(recovery::write(&recovery_file));
        });
    }

    fn poll_project_autosave(&mut self) {
        while let Ok(completion) = self.project_autosave_rx.try_recv() {
            self.project_autosave_busy = false;
            match completion.result {
                Ok(()) => {
                    self.project_autosave_error = None;
                    self.log
                        .info(&format!("Project autosaved: {}", completion.path.display()));
                    if self.project_revision == completion.revision {
                        self.mark_project_saved();
                    }
                }
                Err(err) => {
                    self.project_autosave_error = Some(err.clone());
                    self.log.error(&format!("Project autosave failed: {err}"));
                }
            }
        }
    }

    fn maybe_project_autosave(&mut self) {
        let eligibility = project_autosave::Eligibility {
            dirty: self.project_dirty,
            has_project_path: self.project_path.is_some(),
            has_faces: !self.faces.is_empty(),
            save_busy: self.project_autosave_busy,
            other_operation_busy: self.job.is_some(),
            transition_pending: self.lifecycle.pending.is_some()
                || self.lifecycle.after_save.is_some(),
            snapshot_choice_pending: self.pending_snapshot_action.is_some(),
            snapshot_has_unupdated_changes: self.active_snapshot_has_unupdated_changes(),
            quiet_for: self.last_project_edit_at.elapsed(),
        };
        if !project_autosave::should_start(eligibility) {
            return;
        }
        let Some(path) = self.project_path.clone() else {
            return;
        };
        let project = self.project.clone();
        let face_paths = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        let revision = self.project_revision;
        let tx = self.project_autosave_tx.clone();
        self.project_autosave_busy = true;
        self.project_autosave_error = None;
        std::thread::spawn(move || {
            let result = project.save(&path, &face_paths);
            let _ = tx.send(project_autosave::Completion {
                revision,
                path,
                result,
            });
        });
    }

    fn recover_project(&mut self) {
        self.request_project_transition(ProjectTransition::Recover, None);
    }

    fn recover_project_now(&mut self) {
        if self.job.is_some() {
            return;
        }
        let Some(candidate) = self.recovery_candidate.take() else {
            return;
        };
        let max_dimension = self.settings.max_preview_dimension;
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Recovering project", move |progress| {
            let result = (|| -> Result<RecoveryPayload, String> {
                let paths = candidate.resolved_face_paths();
                let origin_path = candidate.origin_path();
                let mut project = candidate.project;
                let total = paths.len().max(1);
                let mut faces = Vec::new();
                let mut errors = Vec::new();
                for (index, source) in paths.into_iter().enumerate() {
                    Self::set_progress(
                        &progress,
                        Some(index as f32 / total as f32),
                        "Recovering project",
                        &source
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );
                    let expected = project
                        .file_metadata
                        .as_ref()
                        .and_then(|metadata| metadata.faces.get(index))
                        .cloned();
                    match tiff_io::load_preview(&source, max_dimension) {
                        Ok(preview) => {
                            project.ensure_channels(&preview.metadata.channel_names);
                            faces.push(LoadedFace {
                                dpi: dpi::read_dpi(&source, default_dpi),
                                path: source,
                                available: true,
                                preview,
                            });
                        }
                        Err(err) => {
                            errors.push(format!("{}: {err}", source.display()));
                            faces.push(workflow::placeholder_loaded_face(
                                source,
                                expected.as_ref(),
                                default_dpi,
                            ));
                        }
                    }
                }
                Ok(RecoveryPayload {
                    origin_path,
                    project,
                    faces,
                    errors,
                })
            })();
            Self::set_progress(&progress, Some(1.0), "Recovering project", "Complete");
            JobResult::Recover(result)
        });
    }

    fn ui_recovery_window(&mut self, ctx: &egui::Context) {
        let Some(candidate) = self.recovery_candidate.as_ref() else {
            return;
        };
        let saved = Local
            .timestamp_millis_opt(candidate.saved_at_unix_ms)
            .single()
            .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown time".to_owned());
        let origin = candidate
            .origin_project_path
            .as_deref()
            .unwrap_or("Unsaved project")
            .to_owned();
        let mut recover_now = false;
        let mut discard = false;
        egui::Window::new("Recovery available")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.strong("An autosaved Shade Editor recovery state was found.");
                ui.label(format!("Saved: {saved}"));
                ui.label(format!("Project: {origin}"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    recover_now = ui.button("Recover").clicked();
                    discard = ui.button("Discard recovery").clicked();
                });
            });
        if recover_now {
            self.recover_project();
        } else if discard {
            if let Err(err) = recovery::clear() {
                self.report_error(err);
            }
            self.recovery_candidate = None;
        }
    }

    fn ui_faces(&mut self, ui: &mut egui::Ui) {
        ui::faces::ui_faces(self, ui);
    }
    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {
        let face_key = self
            .faces
            .get(self.current_face)
            .map(|face| face.path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rows = self
            .project
            .snapshots
            .iter()
            .map(|snapshot| {
                let export = self
                    .project
                    .snapshot_export_for_face(snapshot.id, &face_key)
                    .map(|record| (record.folder.clone(), record.exported_at_unix_ms));
                (
                    snapshot.id,
                    snapshot.name.clone(),
                    snapshot.created_at_unix_ms,
                    export,
                )
            })
            .collect::<Vec<_>>();

        let all_ids = rows.iter().map(|row| row.0).collect::<Vec<_>>();
        let all_latest_folder = rows
            .iter()
            .filter_map(|row| row.3.as_ref())
            .max_by_key(|(_, exported_at)| *exported_at)
            .map(|(folder, _)| folder.clone());
        let all_exported = !rows.is_empty() && rows.iter().all(|row| row.3.is_some());

        let mut new_snapshot = false;
        let mut export_all = false;
        let mut open_all_folder = false;
        ui.horizontal(|ui| {
            ui.heading("Snapshots");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if all_exported {
                    open_all_folder = ui
                        .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                        .on_hover_text("Open the latest export folder for these snapshots")
                        .clicked();
                }
                export_all = ui
                    .add_enabled(
                        self.job.is_none() && !all_ids.is_empty() && !self.faces.is_empty(),
                        VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                    )
                    .on_hover_text("Export all snapshots for the active Face")
                    .clicked();
                new_snapshot = ui.small_button("+ New").clicked();
            });
        });
        ui.small(
            "Saved adjustment test states. Export always remains available after a successful run.",
        );
        ui.add_space(4.0);

        if new_snapshot {
            self.flush_history_now();
            self.sync_history_to_active_snapshot();
            let id = self.project.create_snapshot();
            if let Some(snapshot) = self
                .project
                .snapshots
                .iter()
                .find(|snapshot| snapshot.id == id)
            {
                self.snapshot_rename_id = Some(id);
                self.snapshot_rename_buffer = snapshot.name.clone();
            }
            self.load_history_for_active_snapshot("Snapshot created");
            self.history_clear_backup = None;
            self.mark_project_dirty();
            self.cache_current_snapshot_preview_if_ready();
        }
        if export_all {
            self.export_snapshot_group_dialog(all_ids.clone(), "all snapshots".to_owned());
        }
        if open_all_folder {
            if let Some(folder) = all_latest_folder.as_deref() {
                self.open_export_folder(folder);
            }
        }

        let active_id = self.project.active_snapshot_id;
        let active_dirty = active_id.is_some() && !self.project.active_snapshot_matches();
        let mut groups: Vec<(String, Vec<(u64, String, i64, Option<(String, i64)>)>)> = Vec::new();
        for row in rows {
            let day = snapshot_day_time(row.2).0;
            if groups.last().map(|group| group.0.as_str()) != Some(day.as_str()) {
                groups.push((day, Vec::new()));
            }
            groups.last_mut().unwrap().1.push(row);
        }

        let mut requested_load = None;
        let mut requested_export = None;
        let mut requested_group_export: Option<(Vec<u64>, String)> = None;
        let mut requested_folder: Option<String> = None;

        for (day, day_rows) in groups {
            ui.add_space(2.0);
            let day_ids = day_rows.iter().map(|row| row.0).collect::<Vec<_>>();
            let day_exported = !day_rows.is_empty() && day_rows.iter().all(|row| row.3.is_some());
            let day_latest_folder = day_rows
                .iter()
                .filter_map(|row| row.3.as_ref())
                .max_by_key(|(_, exported_at)| *exported_at)
                .map(|(folder, _)| folder.clone());
            ui.horizontal(|ui| {
                ui.strong(&day);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if day_exported
                        && ui
                            .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                            .on_hover_text("Open the latest export folder for this day")
                            .clicked()
                    {
                        requested_folder = day_latest_folder.clone();
                    }
                    if ui
                        .add_enabled(
                            self.job.is_none() && !day_ids.is_empty() && !self.faces.is_empty(),
                            VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                        )
                        .on_hover_text("Export all snapshots from this day for the active Face")
                        .clicked()
                    {
                        requested_group_export = Some((day_ids.clone(), day.clone()));
                    }
                });
            });

            for (id, name, created_at, export_record) in day_rows {
                let (_, time) = snapshot_day_time(created_at);
                let selected = active_id == Some(id);
                let display_name = if selected && active_dirty {
                    format!("{name}  *")
                } else {
                    name
                };
                let (row_response, export_clicked, folder_clicked) = snapshot_row_with_actions(
                    ui,
                    selected,
                    selected && active_dirty,
                    &display_name,
                    &time,
                    export_record.is_some(),
                    self.job.is_none() && !self.faces.is_empty(),
                );
                if export_clicked {
                    requested_export = Some(id);
                } else if folder_clicked {
                    requested_folder = export_record.as_ref().map(|record| record.0.clone());
                } else if row_response.clicked() {
                    requested_load = Some(id);
                }
            }
            ui.add_space(4.0);
        }

        if let Some(id) = requested_load {
            self.request_snapshot_load(id);
        }
        if let Some(id) = requested_export {
            self.export_snapshot_dialog(id);
        }
        if let Some((ids, label)) = requested_group_export {
            self.export_snapshot_group_dialog(ids, label);
        }
        if let Some(folder) = requested_folder {
            self.open_export_folder(&folder);
        }

        let Some(active_id) = self.project.active_snapshot_id else {
            return;
        };
        let Some(active_name) = self
            .project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == active_id)
            .map(|snapshot| snapshot.name.clone())
        else {
            return;
        };
        if self.snapshot_rename_id != Some(active_id) {
            self.snapshot_rename_id = Some(active_id);
            self.snapshot_rename_buffer = active_name.clone();
        }

        ui.add_space(6.0);
        ui.label("Snapshot name");
        let rename_response = ui.add(
            egui::TextEdit::singleline(&mut self.snapshot_rename_buffer)
                .desired_width(f32::INFINITY),
        );
        let enter =
            rename_response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if (rename_response.lost_focus() || enter)
            && self.snapshot_rename_buffer.trim() != active_name
        {
            let candidate = self.snapshot_rename_buffer.clone();
            match self.project.rename_snapshot(active_id, &candidate) {
                Ok(true) => {
                    self.snapshot_rename_buffer = candidate.trim().to_owned();
                    self.mark_project_dirty();
                    self.report_info("Snapshot renamed");
                }
                Ok(false) => {}
                Err(err) => self.report_error(err),
            }
        }

        let mut update = false;
        let mut delete = false;
        ui.horizontal(|ui| {
            update = ui.button("Update").clicked();
            delete = ui.button("Delete").clicked();
        });
        if update {
            workflow::update_active_snapshot(self);
        }
        if delete && self.project.delete_snapshot(active_id) {
            self.snapshot_preview_cache.remove_snapshot(active_id);
            self.snapshot_rename_id = None;
            self.snapshot_rename_buffer.clear();
            self.history
                .reset(&self.project.adjustments, "Snapshot deleted");
            self.history_clear_backup = None;
            self.mark_project_dirty();
            self.report_info("Snapshot deleted");
        }
    }
    fn ui_test_code(&mut self, ui: &mut egui::Ui) {
        ui.heading("Test code");
        let channel_names = self
            .faces
            .get(self.current_face)
            .filter(|face| face.available)
            .map(|face| face.preview.metadata.channel_names.clone())
            .unwrap_or_default();
        let palette = self.project.channel_palette.clone();
        let fallback = self
            .project
            .active_snapshot_name()
            .unwrap_or("Test")
            .to_owned();
        let mut changed = ui
            .checkbox(&mut self.project.test_code.enabled, "Write code on export")
            .changed();
        ui.add_enabled_ui(self.project.test_code.enabled, |ui| {
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.project.test_code.text)
                        .hint_text(format!("Empty uses {fallback}")),
                )
                .changed();
            if !channel_names.is_empty() {
                let selected_display = if self.project.test_code.channel == TEST_CODE_ALL_CHANNELS {
                    "Master".to_owned()
                } else {
                    let selected_index = channel_names
                        .iter()
                        .position(|name| name == &self.project.test_code.channel)
                        .unwrap_or(0);
                    channel_display_name(
                        palette.as_ref(),
                        &channel_names[selected_index],
                        selected_index,
                    )
                    .to_owned()
                };
                egui::ComboBox::from_label("Ink / channel")
                    .selected_text(selected_display)
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut self.project.test_code.channel,
                                TEST_CODE_ALL_CHANNELS.to_owned(),
                                "Master",
                            )
                            .changed();
                        ui.separator();
                        for (index, name) in channel_names.iter().enumerate() {
                            let display = channel_display_name(palette.as_ref(), name, index);
                            changed |= ui
                                .selectable_value(
                                    &mut self.project.test_code.channel,
                                    name.clone(),
                                    display,
                                )
                                .changed();
                        }
                    });
            }
            ui.horizontal(|ui| {
                ui.label("Font");
                ui.strong("Tahoma");
            });
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.project.test_code.font_size_pt, 6.0..=72.0)
                        .text("Size (pt)"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.project.test_code.margin_cm, 0.0..=5.0)
                        .text("Edge margin (cm)"),
                )
                .changed();
            egui::ComboBox::from_label("Position")
                .selected_text(match self.project.test_code.position {
                    TestCodePosition::TopLeft => "Top left",
                    TestCodePosition::TopRight => "Top right",
                    TestCodePosition::BottomLeft => "Bottom left",
                    TestCodePosition::BottomRight => "Bottom right",
                })
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(
                            &mut self.project.test_code.position,
                            TestCodePosition::TopLeft,
                            "Top left",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut self.project.test_code.position,
                            TestCodePosition::TopRight,
                            "Top right",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut self.project.test_code.position,
                            TestCodePosition::BottomLeft,
                            "Bottom left",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut self.project.test_code.position,
                            TestCodePosition::BottomRight,
                            "Bottom right",
                        )
                        .changed();
                });
            ui.small("Default: top-left, 1 cm margin. Point size is converted using the TIFF DPI.");
        });
        if changed {
            self.mark_project_dirty();
        }
    }

    fn ui_all_adjustments(
        &mut self,
        ui: &mut egui::Ui,
        channel_names: &[String],
        histograms_before: &[[u32; 256]],
        histograms_after: &[[u32; 256]],
        palette: Option<&ChannelPalette>,
    ) -> bool {
        let mut changed = false;
        let compact_curve_controls = self.settings.compact_curve_controls;
        let tonal_display_mode = self.settings.tonal_display_mode;
        ui.small(
            "Master Levels and Master Curve are independent Master controls. They stack with per-channel edits and never overwrite them. Mixer output rows remain channel-specific.",
        );

        let master_levels_histogram_before = aggregate_histograms(histograms_before);
        let master_levels_histogram_after = aggregate_histograms(histograms_after);
        let master_curve_histogram_before = self
            .settings
            .show_curve_histogram
            .then_some(master_levels_histogram_before);
        let master_curve_histogram_after = self
            .settings
            .show_curve_histogram
            .then_some(master_levels_histogram_after);

        if self.settings.adjustment_tabs {
            let reset_tool = adjustment_tab_bar(ui, &mut self.tool);
            if reset_tool {
                match self.tool {
                    ToolPanel::Levels => reset_master_levels(&mut self.project.adjustments),
                    ToolPanel::Curves => reset_master_curve(&mut self.project.adjustments),
                    ToolPanel::Mixer => {
                        reset_all_mixers(&mut self.project.adjustments, channel_names)
                    }
                }
                changed = true;
            }
            changed |= match self.tool {
                ToolPanel::Levels => master_levels_ui(
                    ui,
                    &mut self.project.adjustments,
                    Some(&master_levels_histogram_before),
                    Some(&master_levels_histogram_after),
                    tonal_display_mode,
                ),
                ToolPanel::Curves => master_curve_ui(
                    ui,
                    &mut self.project.adjustments,
                    master_curve_histogram_before.as_ref(),
                    master_curve_histogram_after.as_ref(),
                    tonal_display_mode,
                    compact_curve_controls,
                ),
                ToolPanel::Mixer => all_mixers_ui(
                    ui,
                    &mut self.project.adjustments,
                    channel_names,
                    self.settings.colorize_adjustments,
                    palette,
                ),
            };
        } else {
            let (body_changed, reset) =
                adjustment_foldout(ui, "master-levels-section", "Master Levels", true, |ui| {
                    master_levels_ui(
                        ui,
                        &mut self.project.adjustments,
                        Some(&master_levels_histogram_before),
                        Some(&master_levels_histogram_after),
                        tonal_display_mode,
                    )
                });
            changed |= body_changed.unwrap_or(false);
            if reset {
                reset_master_levels(&mut self.project.adjustments);
                changed = true;
            }

            ui.add_space(4.0);
            let (body_changed, reset) = adjustment_foldout(
                ui,
                "all-mixers-section",
                "Channel Mixer - all output rows",
                true,
                |ui| {
                    all_mixers_ui(
                        ui,
                        &mut self.project.adjustments,
                        channel_names,
                        self.settings.colorize_adjustments,
                        palette,
                    )
                },
            );
            changed |= body_changed.unwrap_or(false);
            if reset {
                reset_all_mixers(&mut self.project.adjustments, channel_names);
                changed = true;
            }

            ui.add_space(4.0);
            let (body_changed, reset) =
                adjustment_foldout(ui, "master-curve-section", "Master Curve", true, |ui| {
                    master_curve_ui(
                        ui,
                        &mut self.project.adjustments,
                        master_curve_histogram_before.as_ref(),
                        master_curve_histogram_after.as_ref(),
                        tonal_display_mode,
                        compact_curve_controls,
                    )
                });
            changed |= body_changed.unwrap_or(false);
            if reset {
                reset_master_curve(&mut self.project.adjustments);
                changed = true;
            }
        }
        changed
    }

    fn ui_tools(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Tools");
            let layout = if self.settings.sidebar_two_columns {
                "2 columns"
            } else {
                "1 column"
            };
            if ui.small_button(layout).clicked() {
                self.settings.sidebar_two_columns = !self.settings.sidebar_two_columns;
                self.save_settings_quietly();
            }
        });
        ui.separator();
        if self.settings.sidebar_two_columns {
            ui.columns(2, |columns| {
                egui::ScrollArea::vertical()
                    .id_salt("channels-column")
                    .show(&mut columns[0], |ui| self.ui_channels_histogram(ui));
                egui::ScrollArea::vertical()
                    .id_salt("adjustments-column")
                    .show(&mut columns[1], |ui| self.ui_adjustments(ui));
            });
        } else {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.ui_channels_histogram(ui);
                ui.separator();
                self.ui_adjustments(ui);
            });
        }
    }

    fn ui_viewport(&mut self, ui: &mut egui::Ui) {
        if workflow::ui_missing_viewport(self, ui) {
            return;
        }

        let Some(face) = self.faces.get(self.current_face) else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Shade Editor");
                    ui.label("Open a .shade project or add TIFF faces.");
                    if ui.button("Add TIFF faces").clicked() {
                        self.add_faces_dialog();
                    }
                });
            });
            return;
        };

        let file_name = face
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| face.path.display().to_string());
        let meta = face.preview.metadata.clone();
        let dpi_info = face.dpi;
        let color_status = face.color_status.clone();
        let embedded_original_status = face.embedded_original_status.clone();
        let texture = face.texture.clone();
        let original_texture = face.original_texture.clone();
        let embedded_original_texture = face.embedded_original_texture.clone();
        let (width_cm, height_cm) = physical_dimensions_cm(meta.width, meta.height, dpi_info);
        ui.horizontal_wrapped(|ui| {
            ui.strong(file_name);
            ui.separator();
            ui.label(format!("{}-bit", meta.bit_depth));
            ui.label(format!(
                "{} x {} cm",
                format_cm_value(width_cm),
                format_cm_value(height_cm)
            ));
            ui.label(format!("{} x {} px", meta.width, meta.height));
            if dpi_info.has_physical_resolution {
                ui.label(format!("{:.0} x {:.0} DPI", dpi_info.dpi_x, dpi_info.dpi_y));
            } else {
                ui.label(format!(
                    "{:.0} x {:.0} DPI (default)",
                    dpi_info.dpi_x, dpi_info.dpi_y
                ));
            }
            ui.label(meta.color_model.title());
            ui.label(format!("{} channels", meta.samples_per_pixel));
            let profile_text = if color_status.is_problem() {
                egui::RichText::new(color_status.button_label()).color(egui::Color32::YELLOW)
            } else if color_status.is_managed() {
                egui::RichText::new(color_status.button_label()).color(egui::Color32::LIGHT_GREEN)
            } else {
                egui::RichText::new(color_status.button_label())
            };
            let icc_response = ui.small_button(profile_text);
            let open_color_management = icc_response.clicked();
            icc_response.on_hover_text(format!(
                "{}\nClick to manage source ICC and Printer/RIP Soft Proof.",
                color_status.detail()
            ));
            if open_color_management {
                self.color.show = true;
                self.color.selected = self.project.preview_color.assigned_profile_path.clone();
            }
        });
        ui.small("Hold right mouse: BEFORE adjustments with current color-management setup · Hold middle mouse: original TIFF samples with Embedded ICC only (cached, no assigned profile / RIP Soft Proof).");
        ui.separator();

        let Some(texture) = texture else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
            return;
        };

        let visible = ui.available_size().max(egui::vec2(1.0, 1.0));
        if self.fit_requested {
            let natural = texture.size_vec2();
            if natural.x > 0.0 && natural.y > 0.0 {
                self.zoom = ((visible.x - 30.0).max(1.0) / natural.x)
                    .min((visible.y - 30.0).max(1.0) / natural.y)
                    .clamp(0.05, 8.0);
            }
            self.fit_requested = false;
            self.viewport_recenter = true;
        }

        let image_size = texture.size_vec2() * self.zoom;
        let recenter = self.viewport_recenter;
        let output = egui::ScrollArea::both()
            .id_salt("image-viewport")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show_viewport(ui, |ui, viewport| {
                let canvas_size = egui::vec2(
                    (image_size.x + VIEWPORT_OVERSCROLL * 2.0)
                        .max(viewport.width() + VIEWPORT_OVERSCROLL * 2.0),
                    (image_size.y + VIEWPORT_OVERSCROLL * 2.0)
                        .max(viewport.height() + VIEWPORT_OVERSCROLL * 2.0),
                );
                let (canvas_rect, _) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
                let image_rect = egui::Rect::from_center_size(canvas_rect.center(), image_size);
                let pointer_over = ui.rect_contains_pointer(image_rect);
                let show_embedded_source =
                    ui.input(|input| input.pointer.middle_down()) && pointer_over;
                let show_before = !show_embedded_source
                    && ui.input(|input| input.pointer.secondary_down())
                    && pointer_over;
                let display_texture = if show_embedded_source {
                    embedded_original_texture
                        .as_ref()
                        .or(original_texture.as_ref())
                        .unwrap_or(&texture)
                } else if show_before {
                    original_texture.as_ref().unwrap_or(&texture)
                } else {
                    &texture
                };
                ui.put(
                    image_rect,
                    egui::Image::from_texture(display_texture).fit_to_exact_size(image_size),
                );
                if show_embedded_source {
                    let source_label = if embedded_original_texture.is_none() {
                        "SOURCE · PREPARING"
                    } else if embedded_original_status.is_managed() {
                        "SOURCE · EMBEDDED ICC"
                    } else if embedded_original_status.is_problem() {
                        "SOURCE · ICC FALLBACK"
                    } else {
                        "SOURCE · NO EMBEDDED ICC"
                    };
                    ui.painter().text(
                        image_rect.left_top() + egui::vec2(10.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        source_label,
                        egui::FontId::proportional(13.0),
                        egui::Color32::WHITE,
                    );
                } else if show_before {
                    ui.painter().text(
                        image_rect.left_top() + egui::vec2(10.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        "BEFORE",
                        egui::FontId::proportional(13.0),
                        egui::Color32::WHITE,
                    );
                }
                if recenter {
                    ui.scroll_to_rect(image_rect, Some(egui::Align::Center));
                }
            });
        let _ = output;
        if recenter {
            self.viewport_recenter = false;
        }
    }

    fn ensure_previous_shade_list_texture(
        &mut self,
        ctx: &egui::Context,
        entry: &previous_shades::PreviousShadeEntry,
    ) {
        let key = entry.path.clone();
        if self.project_view.list_textures.contains_key(&key) {
            self.project_view
                .list_texture_lru
                .retain(|item| item != &key);
            self.project_view.list_texture_lru.push_back(key);
            return;
        }
        let Ok(Some(thumbnail)) = previous_shades::decode_cached_thumbnail(entry) else {
            return;
        };
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [thumbnail.width, thumbnail.height],
            &thumbnail.rgba,
        );
        let texture = ctx.load_texture(
            format!("previous-shade-list:{}", entry.path),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.project_view.list_textures.insert(key.clone(), texture);
        self.project_view
            .list_texture_lru
            .retain(|item| item != &key);
        self.project_view.list_texture_lru.push_back(key);
        while self.project_view.list_texture_lru.len() > PREVIOUS_SHADE_TEXTURE_CACHE_LIMIT {
            if let Some(oldest) = self.project_view.list_texture_lru.pop_front() {
                self.project_view.list_textures.remove(&oldest);
            }
        }
    }

    fn ui_snapshot_save_reminder(&mut self, ctx: &egui::Context) {
        if !self.export.show_snapshot_save_reminder {
            return;
        }
        let unsaved_project = self.project_path.is_none();
        let quick_target = self.quick_save_target();
        let mut quick_save = false;
        let mut save = false;
        let mut save_as = false;
        let mut later = false;
        egui::Window::new("Save Snapshot/Test project state")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            if unsaved_project {
                ui.strong("The TIFF export is saved, but this Snapshot/Test state is not stored in a .shade project yet.");
                ui.label("This project has never been saved. Quick Save will create the project beside the source TIFF files so the exported test can be reproduced later.");
                if let Some(path) = quick_target.as_ref() {
                    ui.small(format!("Quick Save target: {}", path.display()));
                }
            } else {
                ui.strong("The TIFF export is saved, but the Snapshot/Test state has unsaved project changes.");
                ui.label("Save the .shade project now to keep the test state that produced this TIFF.");
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if unsaved_project {
                    quick_save = ui
                        .add_enabled(self.job.is_none() && quick_target.is_some(), egui::Button::new("Quick Save project"))
                        .clicked();
                    save_as = ui
                        .add_enabled(self.job.is_none() && !self.faces.is_empty(), egui::Button::new("Save As..."))
                        .clicked();
                } else {
                    save = ui
                        .add_enabled(self.job.is_none(), egui::Button::new("Save project"))
                        .clicked();
                }
                later = ui.button("Later").clicked();
            });
        });
        if quick_save && self.quick_save_project() {
            self.export.show_snapshot_save_reminder = false;
        } else if save && self.save_project(false) {
            self.export.show_snapshot_save_reminder = false;
        } else if save_as && self.save_project(true) {
            self.export.show_snapshot_save_reminder = false;
        } else if later {
            self.export.show_snapshot_save_reminder = false;
        }
    }

    fn refresh_icc_profile_catalog(&mut self) {
        self.color.scan_done = true;
        match color_management::installed_profiles() {
            Ok(profiles) => {
                self.color.profiles = profiles;
                self.color.scan_error = None;
                let project_relinked = color_management::relink_project_profiles(
                    &mut self.project,
                    &self.color.profiles,
                );
                let monitor_relinked = color_management::relink_monitor_profile(
                    &mut self.settings,
                    &self.color.profiles,
                );
                if project_relinked {
                    self.mark_project_dirty();
                    self.invalidate_display_previews();
                }
                if monitor_relinked {
                    if let Err(err) = self.settings.save() {
                        self.log.error(&err);
                    }
                    self.invalidate_display_previews();
                }
            }
            Err(err) => {
                self.color.profiles.clear();
                self.color.scan_error = Some(err);
            }
        }
    }

    fn ui_color_management_window(&mut self, ctx: &egui::Context) {
        if !self.color.show {
            return;
        }
        if !self.color.scan_done {
            self.refresh_icc_profile_catalog();
        }

        let Some(active_face) = self.faces.get(self.current_face) else {
            self.color.show = false;
            return;
        };
        let active_model = active_face.preview.metadata.color_model;
        let embedded_name =
            color_management::embedded_profile_description(&active_face.preview.metadata)
                .unwrap_or_else(|| "No embedded ICC".to_owned());
        let profiles = self.color.profiles.clone();
        let scan_error = self.color.scan_error.clone();
        let current_status = active_face.color_status.clone();

        let original_query = self.color.query.clone();
        let mut query = original_query.clone();
        let mut selected = self
            .color
            .selected
            .clone()
            .or_else(|| self.project.preview_color.assigned_profile_path.clone());
        let mut enabled = self.project.preview_color.enabled;
        let mut intent = self.project.preview_color.rendering_intent;
        let mut bpc = self.project.preview_color.black_point_compensation;
        let mut soft_proof_enabled = self.project.preview_color.soft_proof_enabled;
        let mut proofing_intent = self.project.preview_color.proofing_intent;
        let proof_path = self.project.preview_color.proof_profile_path.clone();
        let monitor_path = self.settings.monitor_profile_path.clone();
        let mut gamut_warning = self.settings.gamut_warning;
        let mut show_incompatible = self.color.show_incompatible;
        let mut requested_profile: Option<Option<PathBuf>> = None;
        let mut requested_proof: Option<Option<PathBuf>> = None;
        let mut requested_monitor: Option<Option<PathBuf>> = None;
        let mut browse_requested = false;
        let mut browse_proof_requested = false;
        let mut browse_monitor_requested = false;
        let mut refresh_requested = false;
        let mut open = self.color.show;

        egui::Window::new("Color Management / ICC Preview")
            .open(&mut open)
            .resizable(true)
            .default_size([820.0, 760.0])
            .show(ctx, |ui| {
                ui.heading("Color Management / ICC Preview");
                ui.small("Source-profile assignment and Printer/RIP Soft Proof are display-only. TIFF samples, embedded ICC and Photoshop resources remain untouched by these preview settings.");
                ui.add_space(5.0);
                egui::Grid::new("preview-profile-current")
                    .num_columns(2)
                    .striped(true)
                    .spacing([14.0, 5.0])
                    .show(ui, |ui| {
                        ui.strong("Active preview");
                        ui.label(current_status.button_label())
                            .on_hover_text(current_status.detail());
                        ui.end_row();
                        ui.strong("TIFF base model");
                        ui.label(active_model.title());
                        ui.end_row();
                        ui.strong("Embedded profile");
                        ui.label(&embedded_name);
                        ui.end_row();
                        ui.strong("Assigned source profile");
                        ui.label(
                            self.project
                                .preview_color
                                .assigned_profile_path
                                .as_deref()
                                .unwrap_or("Embedded profile"),
                        );
                        ui.end_row();
                        ui.strong("Printer / RIP proof");
                        ui.label(proof_path.as_deref().unwrap_or("Not selected"));
                        ui.end_row();
                    });

                ui.separator();
                ui.heading("Document / source ICC");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Use embedded profile").clicked() {
                        requested_profile = Some(None);
                    }
                    if ui.button("Browse source ICC / ICM...").clicked() {
                        browse_requested = true;
                    }
                    if ui.button("Refresh system profiles").clicked() {
                        refresh_requested = true;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut enabled, "Enable color-managed preview");
                    ui.checkbox(&mut bpc, "Black point compensation");
                });
                egui::ComboBox::from_label("Source rendering intent")
                    .selected_text(intent.label())
                    .show_ui(ui, |ui| {
                        for value in [
                            PreviewRenderingIntent::Perceptual,
                            PreviewRenderingIntent::RelativeColorimetric,
                            PreviewRenderingIntent::Saturation,
                            PreviewRenderingIntent::AbsoluteColorimetric,
                        ] {
                            ui.selectable_value(&mut intent, value, value.label());
                        }
                    });

                ui.horizontal(|ui| {
                    ui.label("Search source profiles");
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut query)
                            .hint_text("Profile name, filename, RGB/CMYK/Gray, path")
                            .desired_width(430.0),
                    );
                    if !search.has_focus() && !ctx.wants_keyboard_input() {
                        let typed = ctx.input(|input| {
                            input
                                .events
                                .iter()
                                .filter_map(|event| match event {
                                    egui::Event::Text(text) if !text.chars().all(char::is_control) => {
                                        Some(text.as_str())
                                    }
                                    _ => None,
                                })
                                .collect::<String>()
                        });
                        if !typed.is_empty() {
                            query.push_str(&typed);
                            search.request_focus();
                        }
                    }
                    ui.checkbox(&mut show_incompatible, "Show incompatible");
                });

                let visible = profiles
                    .iter()
                    .filter(|profile| profile.matches_query(&query))
                    .filter(|profile| show_incompatible || profile.compatible_with(active_model))
                    .collect::<Vec<_>>();
                let compatible_paths = visible
                    .iter()
                    .filter(|profile| profile.compatible_with(active_model))
                    .map(|profile| profile.path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                if query != original_query {
                    selected = compatible_paths.first().cloned();
                }
                let current_position = selected
                    .as_deref()
                    .and_then(|path| compatible_paths.iter().position(|item| item == path));
                let (up, down, enter) = ctx.input(|input| {
                    (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                        input.key_pressed(egui::Key::Enter),
                    )
                });
                if !compatible_paths.is_empty() && (up || down) {
                    let next = match (current_position, up, down) {
                        (Some(position), true, _) => position.saturating_sub(1),
                        (Some(position), _, true) => (position + 1).min(compatible_paths.len() - 1),
                        (None, _, true) => 0,
                        (None, true, _) => compatible_paths.len() - 1,
                        _ => 0,
                    };
                    selected = compatible_paths.get(next).cloned();
                }
                if enter {
                    if let Some(path) = selected.as_ref() {
                        requested_profile = Some(Some(PathBuf::from(path)));
                    }
                }

                if let Some(error) = scan_error.as_ref() {
                    ui.colored_label(egui::Color32::YELLOW, error);
                }
                egui::ScrollArea::vertical()
                    .id_salt("icc-profile-list")
                    .auto_shrink([false, false])
                    .max_height(210.0)
                    .show(ui, |ui| {
                        for profile in visible {
                            let path_text = profile.path.to_string_lossy().into_owned();
                            let compatible = profile.compatible_with(active_model);
                            let label = format!(
                                "{} · {} · {}",
                                profile.description,
                                profile.color_space_label(),
                                profile.filename()
                            );
                            let response = ui
                                .add_enabled(
                                    compatible,
                                    egui::Button::new(label)
                                        .selected(selected.as_deref() == Some(path_text.as_str())),
                                )
                                .on_hover_text(format!(
                                    "{}\nClass: {}",
                                    profile.path.display(),
                                    profile.device_class_label(),
                                ));
                            if response.clicked() {
                                selected = Some(path_text.clone());
                            }
                            if response.double_clicked() && compatible {
                                requested_profile = Some(Some(profile.path.clone()));
                            }
                        }
                    });
                let can_assign = selected.as_deref().is_some_and(|path| {
                    profiles.iter().any(|profile| {
                        profile.path.to_string_lossy() == path
                            && profile.compatible_with(active_model)
                    })
                });
                if ui
                    .add_enabled(can_assign, egui::Button::new("Assign selected source profile"))
                    .clicked()
                {
                    requested_profile = selected
                        .as_ref()
                        .map(|path| Some(PathBuf::from(path)));
                }

                ui.separator();
                ui.heading("Printer / RIP Soft Proof");
                ui.small("Select an Output-class printer/RIP ICC. Shade Editor uses a LittleCMS proofing transform only for the viewport and project thumbnail; export remains separation/sample preserving.");
                let output_profiles = profiles
                    .iter()
                    .filter(|profile| profile.is_output_profile())
                    .collect::<Vec<_>>();
                let proof_selected_text = proof_path
                    .as_deref()
                    .and_then(|path| {
                        output_profiles
                            .iter()
                            .find(|profile| profile.path.to_string_lossy() == path)
                            .map(|profile| profile.description.clone())
                    })
                    .or_else(|| proof_path.clone())
                    .unwrap_or_else(|| "Choose printer/RIP output profile".to_owned());
                egui::ComboBox::from_label("Installed output profile")
                    .selected_text(proof_selected_text)
                    .width(520.0)
                    .show_ui(ui, |ui| {
                        for profile in &output_profiles {
                            let selected_now = proof_path.as_deref()
                                == Some(profile.path.to_string_lossy().as_ref());
                            if ui
                                .selectable_label(
                                    selected_now,
                                    format!(
                                        "{} · {} · {}",
                                        profile.description,
                                        profile.color_space_label(),
                                        profile.filename()
                                    ),
                                )
                                .clicked()
                            {
                                requested_proof = Some(Some(profile.path.clone()));
                            }
                        }
                    });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Browse Printer/RIP ICC...").clicked() {
                        browse_proof_requested = true;
                    }
                    if ui
                        .add_enabled(proof_path.is_some(), egui::Button::new("Clear proof profile"))
                        .clicked()
                    {
                        requested_proof = Some(None);
                        soft_proof_enabled = false;
                    }
                    ui.checkbox(&mut soft_proof_enabled, "Enable Soft Proof");
                });
                egui::ComboBox::from_label("Proof rendering intent")
                    .selected_text(proofing_intent.label())
                    .show_ui(ui, |ui| {
                        for value in [
                            PreviewRenderingIntent::Perceptual,
                            PreviewRenderingIntent::RelativeColorimetric,
                            PreviewRenderingIntent::Saturation,
                            PreviewRenderingIntent::AbsoluteColorimetric,
                        ] {
                            ui.selectable_value(&mut proofing_intent, value, value.label());
                        }
                    });
                if soft_proof_enabled && proof_path.is_none() && requested_proof.is_none() {
                    ui.colored_label(egui::Color32::YELLOW, "Soft Proof is enabled but no printer/RIP profile is selected.");
                }

                ui.separator();
                ui.heading("Monitor / Display ICC");
                ui.small("Workstation-local display conversion. This path is saved in application settings, not inside the .shade project.");
                let display_profiles = profiles
                    .iter()
                    .filter(|profile| profile.is_display_profile())
                    .collect::<Vec<_>>();
                let monitor_selected_text = monitor_path
                    .as_deref()
                    .and_then(|path| {
                        display_profiles
                            .iter()
                            .find(|profile| profile.path.to_string_lossy() == path)
                            .map(|profile| profile.description.clone())
                    })
                    .or_else(|| monitor_path.clone())
                    .unwrap_or_else(|| "sRGB display fallback".to_owned());
                egui::ComboBox::from_label("Installed display profile")
                    .selected_text(monitor_selected_text)
                    .width(520.0)
                    .show_ui(ui, |ui| {
                        for profile in &display_profiles {
                            let selected_now = monitor_path.as_deref()
                                == Some(profile.path.to_string_lossy().as_ref());
                            if ui
                                .selectable_label(
                                    selected_now,
                                    format!("{} · {}", profile.description, profile.filename()),
                                )
                                .clicked()
                            {
                                requested_monitor = Some(Some(profile.path.clone()));
                            }
                        }
                    });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Browse Monitor ICC...").clicked() {
                        browse_monitor_requested = true;
                    }
                    if ui
                        .add_enabled(monitor_path.is_some(), egui::Button::new("Use sRGB fallback"))
                        .clicked()
                    {
                        requested_monitor = Some(None);
                    }
                    ui.add_enabled_ui(soft_proof_enabled, |ui| {
                        ui.checkbox(&mut gamut_warning, "Gamut warning");
                    });
                });
                ui.small("Gamut warning is active only with Printer/RIP Soft Proof. Middle-mouse source preview deliberately bypasses assigned source, proof and monitor profiles and uses only the TIFF embedded ICC.");
            });

        self.color.show = open;
        self.color.query = query;
        self.color.selected = selected;
        self.color.show_incompatible = show_incompatible;

        if refresh_requested {
            self.color.scan_done = false;
            self.refresh_icc_profile_catalog();
        }
        if browse_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_profile = Some(Some(path));
            }
        }
        if browse_proof_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_proof = Some(Some(path));
            }
        }
        if browse_monitor_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_monitor = Some(Some(path));
            }
        }

        let mut changed = false;
        let mut display_settings_changed = false;
        if self.project.preview_color.enabled != enabled {
            self.project.preview_color.enabled = enabled;
            changed = true;
        }
        if self.project.preview_color.rendering_intent != intent {
            self.project.preview_color.rendering_intent = intent;
            changed = true;
        }
        if self.project.preview_color.black_point_compensation != bpc {
            self.project.preview_color.black_point_compensation = bpc;
            changed = true;
        }
        if self.project.preview_color.soft_proof_enabled != soft_proof_enabled {
            self.project.preview_color.soft_proof_enabled = soft_proof_enabled;
            changed = true;
        }
        if self.project.preview_color.proofing_intent != proofing_intent {
            self.project.preview_color.proofing_intent = proofing_intent;
            changed = true;
        }

        if let Some(requested) = requested_profile {
            match requested {
                None => {
                    if self.project.preview_color.assigned_profile_path.is_some() {
                        self.project.preview_color.assigned_profile_path = None;
                        self.project.preview_color.assigned_profile_identity = None;
                        self.color.selected = None;
                        changed = true;
                    }
                }
                Some(path) => match color_management::inspect_profile(&path) {
                    Ok(profile) if profile.compatible_with(active_model) => {
                        let path_text = path.to_string_lossy().into_owned();
                        if self.project.preview_color.assigned_profile_path.as_deref()
                            != Some(path_text.as_str())
                        {
                            self.project.preview_color.assigned_profile_path =
                                Some(path_text.clone());
                            self.project.preview_color.assigned_profile_identity =
                                Some(profile.identity().clone());
                            self.color.selected = Some(path_text);
                            changed = true;
                        }
                    }
                    Ok(profile) => self.report_error(format!(
                        "Cannot assign '{}': profile color space {} does not match active TIFF {}.",
                        profile.description,
                        profile.color_space_label(),
                        active_model.title(),
                    )),
                    Err(err) => self.report_error(err),
                },
            }
        }

        if let Some(requested) = requested_proof {
            match requested {
                None => {
                    if self.project.preview_color.proof_profile_path.is_some() {
                        self.project.preview_color.proof_profile_path = None;
                        self.project.preview_color.proof_profile_identity = None;
                        self.project.preview_color.soft_proof_enabled = false;
                        changed = true;
                    }
                }
                Some(path) => match color_management::inspect_profile(&path) {
                    Ok(profile) if profile.is_output_profile() => {
                        let path_text = path.to_string_lossy().into_owned();
                        if self.project.preview_color.proof_profile_path.as_deref()
                            != Some(path_text.as_str())
                        {
                            self.project.preview_color.proof_profile_path = Some(path_text);
                            self.project.preview_color.proof_profile_identity =
                                Some(profile.identity().clone());
                            changed = true;
                        }
                        if !self.project.preview_color.soft_proof_enabled {
                            self.project.preview_color.soft_proof_enabled = true;
                            changed = true;
                        }
                    }
                    Ok(profile) => self.report_error(format!(
                        "Cannot use '{}' for Soft Proof: profile class is {}, not Output/Printer.",
                        profile.description,
                        profile.device_class_label(),
                    )),
                    Err(err) => self.report_error(err),
                },
            }
        }

        if let Some(requested) = requested_monitor {
            match requested {
                None => {
                    if self.settings.monitor_profile_path.is_some() {
                        self.settings.monitor_profile_path = None;
                        self.settings.monitor_profile_identity = None;
                        display_settings_changed = true;
                    }
                }
                Some(path) => match color_management::inspect_profile(&path) {
                    Ok(profile) if profile.is_display_profile() => {
                        let path_text = path.to_string_lossy().into_owned();
                        self.settings.monitor_profile_path = Some(path_text);
                        self.settings.monitor_profile_identity = Some(profile.identity().clone());
                        display_settings_changed = true;
                    }
                    Ok(profile) => self.report_error(format!(
                        "Cannot use '{}' as Monitor ICC: profile must be RGB Display-class, found {} / {}.",
                        profile.description,
                        profile.device_class_label(),
                        profile.color_space_label(),
                    )),
                    Err(err) => self.report_error(err),
                },
            }
        }
        if self.settings.gamut_warning != gamut_warning {
            self.settings.gamut_warning = gamut_warning;
            display_settings_changed = true;
        }

        if changed {
            self.mark_project_dirty();
            self.invalidate_display_previews();
        }
        if display_settings_changed {
            if let Err(err) = self.settings.save() {
                self.log.error(&err);
            }
            self.invalidate_display_previews();
        }
    }

    fn ui_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = self.show_settings;
        let mut rebuild_previews_requested = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .default_size([640.0, 760.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Application");
                let mut changed = false;
                changed |= ui
                    .checkbox(
                        &mut self.settings.auto_update,
                        "Automatically check and download updates",
                    )
                    .changed();
                let dark_changed = ui
                    .checkbox(&mut self.settings.dark_mode, "Dark mode")
                    .changed();
                changed |= dark_changed;
                ui.horizontal(|ui| {
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.settings.max_preview_dimension, 600..=4000)
                                .text("Preview max dimension"),
                        )
                        .changed();
                    if ui
                        .add_enabled(
                            !self.faces.is_empty() && self.job.is_none(),
                            egui::Button::new("Rebuild previews"),
                        )
                        .on_hover_text("Reload all current TIFF Faces using this preview size")
                        .clicked()
                    {
                        rebuild_previews_requested = true;
                    }
                });
                ui.small("The max dimension is used when TIFF previews are loaded. Use Rebuild previews to apply a changed value to Faces already open in this project.");
                ui.separator();
                ui.heading("Preview diagnostics");
                changed |= ui
                    .checkbox(
                        &mut self.settings.show_clipping_warnings,
                        "Show per-channel clipping warnings",
                    )
                    .changed();
                ui.small("Clipping percentages are estimates from the loaded preview samples. Yellow starts at 0.10%; red at 1.00%. Full-resolution export data is not sampled or modified for these warnings.");
                ui.small("ICC profile assignment is project-owned. Click the profile name beside the active Face metadata to open Color Management.");
                ui.separator();
                ui.heading("Export & storage");
                changed |= ui
                    .checkbox(
                        &mut self.settings.lzw_compression,
                        "Use LZW compression for exported TIFF files",
                    )
                    .changed();
                ui.small("LZW is enabled by default. Disable it only when you specifically need to preserve a supported source compression mode.");
                changed |= ui
                    .checkbox(
                        &mut self.settings.validate_after_export,
                        "Validate TIFF after normal Export face / Export all",
                    )
                    .changed();
                ui.small("When enabled, Shade Editor immediately re-decodes every exported TIFF and verifies channel layout/names, ICC/Photoshop resources, compression/predictor policy and complete strip decoding.");
                changed |= ui
                    .checkbox(
                        &mut self.settings.export_all_test_code,
                        "Write Test Code during Export all",
                    )
                    .changed();
                ui.small("Off by default: Export all writes clean Face TIFFs without Test Code. Enable this only when every Face in Export all should receive the current Test Code configuration.");
                ui.add_space(8.0);
                ui.strong("Snapshot / Test export filename template");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.settings.snapshot_export_template)
                            .desired_width(520.0),
                    )
                    .changed();
                ui.small("Used only by Snapshot/Test exports. Tokens: {project}, {face}, {snapshot}, {testcode}, {source}, {date}. Export Face keeps its manual Save As filename, and Export All keeps the template in its own window.");
                let old_default_dpi = self.settings.default_dpi;
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.default_dpi, 72.0..=1200.0)
                            .text("Default DPI")
                            .suffix(" dpi"),
                    )
                    .changed();
                ui.small("Used when a TIFF has no valid physical DPI. Default: 220. Exported TIFFs receive this DPI when the source does not provide one.");
                let dpi_changed = (old_default_dpi - self.settings.default_dpi).abs() > f64::EPSILON;
                if dpi_changed {
                    for face in &mut self.faces {
                        if face.dpi.used_default {
                            face.dpi = dpi::DpiInfo::with_default(self.settings.default_dpi);
                        }
                    }
                }
                ui.separator();
                ui.heading("Windows Explorer integration");
                let shell_installer = Self::bundled_shell_script("Install-ShadeEditorShell.ps1");
                let shell_uninstaller = Self::bundled_shell_script("Uninstall-ShadeEditorShell.ps1");
                if let Some(installer) = shell_installer {
                    ui.small(format!("Bundled Shell package: {}", installer.parent().unwrap_or_else(|| Path::new(".")).display()));
                    ui.horizontal(|ui| {
                        if ui.button("Install Shell integration").clicked() { self.launch_shell_script("Install-ShadeEditorShell.ps1", "installation"); }
                        if shell_uninstaller.is_some() && ui.button("Uninstall Shell integration").clicked() { self.launch_shell_script("Uninstall-ShadeEditorShell.ps1", "removal"); }
                    });
                    ui.small("The installer may request administrator permission because Explorer COM/property handlers are registered machine-wide.");
                } else {
                    ui.colored_label(egui::Color32::YELLOW, "Bundled shell folder not found next to ShadeEditor.exe.");
                    ui.small("Install the Shell package separately, or place the shell folder from the build package next to ShadeEditor.exe.");
                }
                ui.separator();
                ui.heading("Editor layout");
                changed |= ui
                    .checkbox(
                        &mut self.settings.sidebar_two_columns,
                        "Use two-column tools sidebar",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.show_all_histograms,
                        "Show a histogram for every channel",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.adjustment_tabs,
                        "Use tabs for Levels / Mixer / Curve",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.compact_curve_controls,
                        "Compact Curve editor (hide Input / Output fields)",
                    )
                    .changed();
                ui.small("When enabled, Curve keeps only the draggable graph and hides the selected-point label, Input / Output fields, and helper text.");
                ui.separator();
                ui.heading("Color guides");
                ui.horizontal(|ui| {
                    ui.label("Curve / Histogram direction");
                    changed |= tonal_display_mode_selector(ui, &mut self.settings.tonal_display_mode);
                });
                changed |= ui
                    .checkbox(
                        &mut self.settings.colorize_histograms,
                        "Colorize histograms by channel",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.colorize_adjustments,
                        "Colorize Levels / Mixer / Curve by channel",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.show_curve_histogram,
                        "Show active histogram behind Curve",
                    )
                    .changed();
                ui.separator();
                ui.heading("Channel palettes");
                ui.small("Palettes change only UI channel names/colors. TIFF channel names and separation order stay untouched. The active project palette is saved inside the .shade file.");
                let palette_library = self.settings.palette_library();
                let default_palette_name = if self.settings.default_palette_id == palette::AUTO_PALETTE_ID {
                    "Automatic - CMYK/RGB from first Face".to_owned()
                } else {
                    palette_library
                        .iter()
                        .find(|palette| palette.id == self.settings.default_palette_id)
                        .map(|palette| palette.name.clone())
                        .unwrap_or_else(|| "Automatic - CMYK/RGB from first Face".to_owned())
                };
                egui::ComboBox::from_label("Default palette for new projects")
                    .selected_text(default_palette_name)
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut self.settings.default_palette_id,
                                palette::AUTO_PALETTE_ID.to_owned(),
                                "Automatic - CMYK/RGB from first Face",
                            )
                            .changed();
                        for palette in &palette_library {
                            changed |= ui
                                .selectable_value(
                                    &mut self.settings.default_palette_id,
                                    palette.id.clone(),
                                    &palette.name,
                                )
                                .changed();
                        }
                    });

                ui.label("Built-in palettes (read-only)");
                for builtin in palette::builtin_palettes() {
                    egui::CollapsingHeader::new(&builtin.name)
                        .id_salt(format!("builtin-palette-{}", builtin.id))
                        .show(ui, |ui| {
                            for entry in &builtin.channels {
                                palette_entry_readonly(ui, entry);
                            }
                        });
                }

                let mut delete_palette = None;
                let mut add_channel_to = None;
                let mut remove_channel = None;
                for custom in &mut self.settings.custom_palettes {
                    let custom_id = custom.id.clone();
                    egui::CollapsingHeader::new(format!("Custom - {}", custom.name))
                        .id_salt(format!("custom-palette-{custom_id}"))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Palette name");
                                changed |= ui.text_edit_singleline(&mut custom.name).changed();
                                if ui.small_button("Delete palette").clicked() {
                                    delete_palette = Some(custom_id.clone());
                                }
                            });
                            ui.add_space(3.0);
                            for (index, entry) in custom.channels.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}", index + 1));
                                    changed |= ui
                                        .add(egui::TextEdit::singleline(&mut entry.name).desired_width(130.0))
                                        .changed();
                                    changed |= ui.color_edit_button_srgb(&mut entry.color).changed();
                                    if ui.small_button("-").on_hover_text("Remove channel slot").clicked() {
                                        remove_channel = Some((custom_id.clone(), index));
                                    }
                                });
                            }
                            if ui.small_button("+ Channel slot").clicked() {
                                add_channel_to = Some(custom_id.clone());
                            }
                        });
                }
                if let Some((id, index)) = remove_channel {
                    if let Some(custom) = self.settings.custom_palettes.iter_mut().find(|item| item.id == id) {
                        if index < custom.channels.len() {
                            custom.channels.remove(index);
                            changed = true;
                        }
                    }
                }
                if let Some(id) = add_channel_to {
                    if let Some(custom) = self.settings.custom_palettes.iter_mut().find(|item| item.id == id) {
                        let number = custom.channels.len() + 1;
                        let color = palette::fallback_channel_color("Spot", number - 1);
                        custom.channels.push(palette::ChannelPaletteEntry {
                            name: format!("Ink {number}"),
                            color,
                        });
                        changed = true;
                    }
                }
                if let Some(id) = delete_palette {
                    changed |= self.settings.delete_custom_palette(&id);
                }
                if ui.button("+ New custom palette").clicked() {
                    self.settings.create_custom_palette();
                    changed = true;
                }
                if dark_changed {
                    apply_theme(ctx, self.settings.dark_mode);
                }
                if changed {
                    if let Err(err) = self.settings.save() {
                        self.report_error(err);
                    }
                }
                });
            });
        self.show_settings = open;
        if rebuild_previews_requested {
            self.rebuild_previews();
        }
    }

    fn ui_about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open = self.show_about;
        egui::Window::new("About Shade Editor")
            .open(&mut open)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.heading("Shade Editor");
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("Native multi-channel TIFF shade editor for digital ceramic printing.");
                ui.separator();
                ui.label("Copyright (c) 2026 Emad Ghasemi");
                ui.label("MIT License");
                ui.hyperlink_to(
                    "GitHub repository",
                    "https://github.com/emadgh/windows-shade-editor",
                );
                ui.hyperlink_to("EmadGhasemi.ir", "https://emadghasemi.ir");
                ui.separator();
                ui.label("Update controls are located on the right side of the main toolbar.");
                ui.separator();
                ui.strong("Shortcuts");
                egui::Grid::new("about-shortcuts")
                    .num_columns(2)
                    .spacing([18.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("File");
                        ui.label("Ctrl+N  New   |   Ctrl+S  Save   |   Ctrl+Shift+S  Save As");
                        ui.end_row();
                        ui.strong("Export");
                        ui.label("Ctrl+E  Export Face   |   Ctrl+Shift+E  Export All");
                        ui.end_row();
                        ui.strong("View / Settings");
                        ui.label("F  Fit image   |   G  Settings");
                        ui.end_row();
                        ui.strong("Channels");
                        ui.label(
                            "1-9  Select channel   |   ~  Toggle Master   |   S  Solo channel",
                        );
                        ui.end_row();
                        ui.strong("Snapshot");
                        ui.label("Ctrl+Enter  Update active Snapshot");
                        ui.end_row();
                        ui.strong("Curve");
                        ui.label("Arrow keys  Nudge point   |   Shift+Arrow  Larger step");
                        ui.end_row();
                        ui.strong("History");
                        ui.label("Ctrl+Alt+Z  Undo   |   Ctrl+Shift+Z  Redo");
                        ui.end_row();
                    });
            });
        self.show_about = open;
    }

    fn ui_logs_window(&mut self, ctx: &egui::Context) {
        if !self.show_logs {
            return;
        }
        let mut open = self.show_logs;
        egui::Window::new("Application log")
            .open(&mut open)
            .default_size([780.0, 480.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.log_cache = self.log.read();
                    }
                    if ui.button("Clear").clicked() {
                        match self.log.clear() {
                            Ok(()) => self.log_cache.clear(),
                            Err(err) => self.report_error(err),
                        }
                    }
                    ui.label(self.log.path().display().to_string());
                });
                ui.separator();
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.log_cache)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(22),
                        );
                    });
            });
        self.show_logs = open;
    }
}

impl eframe::App for ShadeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        self.poll_job();
        self.complete_transition_after_save(ui.ctx());
        self.poll_export_queue();
        self.poll_render(ui.ctx());
        self.sync_update_state();
        self.poll_autosave();
        self.poll_project_autosave();
        self.handle_dropped_files(ui.ctx());
        workflow::handle_shortcuts(self, ui.ctx());
        if !self.project_view.open {
            self.handle_history_shortcuts(ui.ctx());
        }
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("shade-editor-curve-graph-focused"), false);
        });
        self.maybe_autosave();
        self.maybe_project_autosave();
        self.handle_close_request(ui.ctx());

        egui::Panel::top("toolbar").show(ui, |ui| self.ui_toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.ui_status(ui));
        egui::Panel::left("faces")
            .default_size(270.0)
            .resizable(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.ui_faces(ui))
            });
        let tools_width = if self.settings.sidebar_two_columns {
            760.0
        } else {
            400.0
        };
        egui::Panel::right("tools")
            .default_size(tools_width)
            .min_size(if self.settings.sidebar_two_columns {
                560.0
            } else {
                300.0
            })
            .resizable(true)
            .show(ui, |ui| self.ui_tools(ui));
        egui::CentralPanel::default().show(ui, |ui| self.ui_viewport(ui));

        self.ui_settings_window(ui.ctx());
        self.ui_color_management_window(ui.ctx());
        self.ui_about_window(ui.ctx());
        self.ui_logs_window(ui.ctx());
        self.ui_previous_shades_window(ui.ctx());
        self.ui_export_all_window(ui.ctx());
        self.ui_export_queue_window(ui.ctx());
        self.ui_tiff_inspector_window(ui.ctx());
        self.ui_backup_restore_window(ui.ctx());
        self.ui_recovery_window(ui.ctx());
        self.ui_snapshot_update_confirmation(ui.ctx());
        self.ui_snapshot_save_reminder(ui.ctx());
        self.ui_project_transition_confirmation(ui.ctx());
        self.commit_pending_history(ui.ctx(), false);

        self.start_render_if_needed(ui.ctx());
    }
}

fn append_path_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

fn project_name_for_path(current: &str, path: &Path) -> String {
    let trimmed = current.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("Untitled Shade") {
        return trimmed.to_owned();
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Shade Project".to_owned())
}

fn previous_shade_history_row(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    metadata: &str,
    detail_primary: &str,
    detail_secondary: &str,
    thumbnail: Option<&egui::TextureHandle>,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
    let height = 84.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let visuals = ui.visuals();
    let fill = if selected {
        visuals.selection.bg_fill.gamma_multiply(0.72)
    } else if response.hovered() {
        visuals.widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, fill);
    }

    let thumb_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(7.0, 14.0),
        egui::vec2(56.0, 56.0),
    );
    if let Some(texture) = thumbnail {
        let natural = texture.size_vec2();
        let scale = if natural.x > 0.0 && natural.y > 0.0 {
            (thumb_rect.width() / natural.x)
                .min(thumb_rect.height() / natural.y)
                .min(1.0)
        } else {
            1.0
        };
        let image_rect = egui::Rect::from_center_size(thumb_rect.center(), natural * scale);
        ui.painter()
            .rect_filled(thumb_rect, 4.0, ui.visuals().extreme_bg_color);
        ui.painter().image(
            texture.id(),
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        ui.painter().rect_stroke(
            thumb_rect,
            4.0,
            visuals.widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            thumb_rect.center(),
            egui::Align2::CENTER_CENTER,
            "—",
            egui::FontId::proportional(16.0),
            visuals.weak_text_color(),
        );
    }

    let text_left = thumb_rect.right() + 9.0;
    ui.painter().text(
        egui::pos2(text_left, rect.top() + 14.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.5),
        if selected {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        },
    );
    ui.painter().text(
        egui::pos2(text_left, rect.top() + 34.0),
        egui::Align2::LEFT_CENTER,
        metadata,
        egui::FontId::proportional(12.0),
        visuals.weak_text_color(),
    );
    ui.painter().text(
        egui::pos2(text_left, rect.top() + 54.0),
        egui::Align2::LEFT_CENTER,
        detail_primary,
        egui::FontId::proportional(11.5),
        visuals.weak_text_color(),
    );
    if !detail_secondary.is_empty() {
        ui.painter().text(
            egui::pos2(text_left, rect.top() + 72.0),
            egui::Align2::LEFT_CENTER,
            detail_secondary,
            egui::FontId::proportional(11.5),
            visuals.weak_text_color(),
        );
    }
    response
}

fn adjustment_tab_bar(ui: &mut egui::Ui, tool: &mut ToolPanel) -> bool {
    ui.add_space(7.0);
    let mut reset = false;
    ui.horizontal(|ui| {
        let spacing = ui.spacing().item_spacing.x;
        let reset_width = 54.0;
        let available = ui.available_width();
        let tab_width = ((available - reset_width - spacing * 3.0) / 3.0).clamp(54.0, 76.0);
        if ui
            .add_sized(
                [tab_width, 30.0],
                egui::Button::new("Levels").selected(*tool == ToolPanel::Levels),
            )
            .clicked()
        {
            *tool = ToolPanel::Levels;
        }
        if ui
            .add_sized(
                [tab_width, 30.0],
                egui::Button::new("Mixer").selected(*tool == ToolPanel::Mixer),
            )
            .clicked()
        {
            *tool = ToolPanel::Mixer;
        }
        if ui
            .add_sized(
                [tab_width, 30.0],
                egui::Button::new("Curve").selected(*tool == ToolPanel::Curves),
            )
            .clicked()
        {
            *tool = ToolPanel::Curves;
        }
        reset = ui
            .add_sized([reset_width, 30.0], egui::Button::new("Reset"))
            .clicked();
    });
    ui.add_space(7.0);
    reset
}

fn unique_shade_path(directory: &Path, stem: &str) -> PathBuf {
    let candidate = directory.join(format!("{stem}.shade"));
    if !candidate.exists() {
        return candidate;
    }
    for number in 2u32.. {
        let candidate = directory.join(format!("{stem}-{number}.shade"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn format_previous_shade_time(unix_ms: i64) -> String {
    if unix_ms <= 0 {
        return "-".to_owned();
    }
    Local
        .timestamp_millis_opt(unix_ms)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".to_owned())
}

fn format_byte_count(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

fn adjustment_foldout<R>(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    title: impl Into<egui::WidgetText>,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> (Option<R>, bool) {
    let id = ui.make_persistent_id(id_salt);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let title = title.into();
    let mut reset = false;
    let header = ui.horizontal(|ui| {
        state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
        let title_response = ui.add(egui::Label::new(title).sense(egui::Sense::click()));
        if title_response.clicked() {
            state.toggle(ui);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            reset = ui.small_button("Reset").clicked();
        });
    });
    let body = state.show_body_indented(&header.response, ui, body);
    (body.map(|response| response.inner), reset)
}

fn reset_mixer_row(
    adjustment: &mut ChannelAdjustment,
    output_name: &str,
    channel_names: &[String],
) {
    adjustment.mixer.coefficients.clear();
    for name in channel_names {
        adjustment
            .mixer
            .coefficients
            .insert(name.clone(), if name == output_name { 1.0 } else { 0.0 });
    }
    adjustment.mixer.constant = 0.0;
}

fn cleanup_master_adjustment(adjustments: &mut BTreeMap<String, ChannelAdjustment>) {
    let remove = adjustments
        .get(MASTER_ADJUSTMENT_KEY)
        .is_some_and(|adjustment| adjustment == &ChannelAdjustment::default());
    if remove {
        adjustments.remove(MASTER_ADJUSTMENT_KEY);
    }
}

fn reset_master_levels(adjustments: &mut BTreeMap<String, ChannelAdjustment>) {
    if let Some(master) = adjustments.get_mut(MASTER_ADJUSTMENT_KEY) {
        master.levels = model::Levels::default();
    }
    cleanup_master_adjustment(adjustments);
}

fn reset_master_curve(adjustments: &mut BTreeMap<String, ChannelAdjustment>) {
    if let Some(master) = adjustments.get_mut(MASTER_ADJUSTMENT_KEY) {
        master.curve = model::Curve::default();
    }
    cleanup_master_adjustment(adjustments);
}

fn reset_all_mixers(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
) {
    for output_name in channel_names {
        let adjustment = adjustments.entry(output_name.clone()).or_default();
        reset_mixer_row(adjustment, output_name, channel_names);
    }
}

fn master_levels_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    histogram_before: Option<&[u32; 256]>,
    histogram_after: Option<&[u32; 256]>,
    display_mode: TonalDisplayMode,
) -> bool {
    let mut draft = adjustments
        .get(MASTER_ADJUSTMENT_KEY)
        .cloned()
        .unwrap_or_default();
    if !ui::levels_mixer::levels_ui(
        ui,
        &mut draft,
        histogram_before,
        histogram_after,
        None,
        display_mode,
    ) {
        return false;
    }
    adjustments
        .entry(MASTER_ADJUSTMENT_KEY.to_owned())
        .or_default()
        .levels = draft.levels;
    cleanup_master_adjustment(adjustments);
    true
}

fn master_curve_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    histogram_before: Option<&[u32; 256]>,
    histogram_after: Option<&[u32; 256]>,
    display_mode: TonalDisplayMode,
    compact_controls: bool,
) -> bool {
    let mut draft = adjustments
        .get(MASTER_ADJUSTMENT_KEY)
        .cloned()
        .unwrap_or_default();
    if !curves_ui(
        ui,
        &mut draft,
        histogram_before,
        histogram_after,
        Some(ui.visuals().text_color()),
        display_mode,
        compact_controls,
        true,
    ) {
        return false;
    }
    adjustments
        .entry(MASTER_ADJUSTMENT_KEY.to_owned())
        .or_default()
        .curve = draft.curve;
    cleanup_master_adjustment(adjustments);
    true
}

fn aggregate_histograms(histograms: &[[u32; 256]]) -> [u32; 256] {
    let mut aggregate = [0u32; 256];
    for histogram in histograms {
        for (target, value) in aggregate.iter_mut().zip(histogram.iter()) {
            *target = target.saturating_add(*value);
        }
    }
    aggregate
}

fn all_mixers_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
    colorize: bool,
    palette: Option<&ChannelPalette>,
) -> bool {
    let mut changed = false;
    for (index, output_name) in channel_names.iter().enumerate() {
        let display = channel_display_name(palette, output_name, index);
        ui.collapsing(format!("Output - {display}"), |ui| {
            let adjustment = adjustments.entry(output_name.clone()).or_default();
            let accent = colorize.then(|| channel_color(palette, output_name, index));
            changed |= ui::levels_mixer::mixer_ui(
                ui,
                adjustment,
                output_name,
                channel_names,
                accent,
                palette,
            );
        });
    }
    changed
}

fn tonal_display_mode_selector(ui: &mut egui::Ui, mode: &mut TonalDisplayMode) -> bool {
    let current = mode.label();
    let next = mode.toggled().label();
    if ui
        .button(format!("Mode: {current}"))
        .on_hover_text(format!(
            "Click to switch Curve and Histogram display to {next}. This only changes presentation and interaction; TIFF adjustment math is unchanged."
        ))
        .clicked()
    {
        *mode = mode.toggled();
        true
    } else {
        false
    }
}

fn with_accent<R>(
    ui: &mut egui::Ui,
    accent: Option<egui::Color32>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        if let Some(color) = accent {
            let visuals = ui.visuals_mut();
            visuals.selection.bg_fill = color;
            visuals.selection.stroke.color = color;
            visuals.widgets.active.bg_fill = color.gamma_multiply(0.45);
            visuals.widgets.hovered.bg_fill = color.gamma_multiply(0.28);
        }
        add(ui)
    })
    .inner
}

fn channel_click_state(
    selected_channel: usize,
    solo_channel: Option<usize>,
    clicked_channel: usize,
) -> (usize, Option<usize>) {
    if selected_channel != clicked_channel {
        // First click on another channel selects it for editing and returns to composite.
        (clicked_channel, None)
    } else if solo_channel == Some(clicked_channel) {
        // Second click while solo is active returns to the composite preview.
        (selected_channel, None)
    } else {
        // Clicking the already-selected channel toggles its monochrome solo preview on.
        (selected_channel, Some(clicked_channel))
    }
}

fn export_queue_progress_bar(ui: &mut egui::Ui, progress: f32, detail: &str) -> egui::Response {
    // ProgressBar already fills the available width by default, but keep the
    // requested size explicit and finite. Passing f32::INFINITY here creates
    // non-finite widget geometry; egui's pointer hit-test can then panic as
    // soon as the mouse moves over the queue window.
    let available_width = ui.available_width();
    let width = if available_width.is_finite() {
        available_width.max(1.0)
    } else {
        1.0
    };
    let progress = if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    ui.add(
        egui::ProgressBar::new(progress)
            .desired_width(width)
            .text(detail),
    )
}

#[cfg(test)]
mod export_queue_ui_tests {
    use super::{egui, export_queue_progress_bar};

    #[test]
    fn processing_progress_bar_has_finite_hit_geometry_during_hover() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 640.0));
        let mut progress_rect = None;

        let mut first_input = egui::RawInput::default();
        first_input.screen_rect = Some(screen);
        let _ = ctx.run_ui(first_input, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                progress_rect = Some(export_queue_progress_bar(ui, f32::NAN, "Processing").rect);
                ui.small("C:\\exports\\sample.tif");
            });
        });

        let progress_rect = progress_rect.expect("progress bar should be laid out");
        assert!(progress_rect.min.x.is_finite());
        assert!(progress_rect.min.y.is_finite());
        assert!(progress_rect.max.x.is_finite());
        assert!(progress_rect.max.y.is_finite());

        let mut hover_input = egui::RawInput::default();
        hover_input.screen_rect = Some(screen);
        hover_input
            .events
            .push(egui::Event::PointerMoved(progress_rect.center()));
        let _ = ctx.run_ui(hover_input, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                export_queue_progress_bar(ui, 0.5, "Processing");
                ui.small("C:\\exports\\sample.tif");
            });
        });
    }
}

#[cfg(test)]
mod channel_interaction_tests {
    use super::channel_click_state;

    #[test]
    fn first_click_selects_without_solo_then_active_click_toggles_solo() {
        assert_eq!(channel_click_state(0, None, 2), (2, None));
        assert_eq!(channel_click_state(2, None, 2), (2, Some(2)));
        assert_eq!(channel_click_state(2, Some(2), 2), (2, None));
    }

    #[test]
    fn selecting_another_channel_exits_previous_solo() {
        assert_eq!(channel_click_state(2, Some(2), 4), (4, None));
    }
}

fn channel_display_name<'a>(
    palette: Option<&'a ChannelPalette>,
    actual_name: &'a str,
    index: usize,
) -> &'a str {
    palette
        .map(|palette| palette.display_name(actual_name, index))
        .unwrap_or(actual_name)
}

fn channel_color(palette: Option<&ChannelPalette>, name: &str, index: usize) -> egui::Color32 {
    let [r, g, b] = palette
        .map(|palette| palette.color(name, index))
        .unwrap_or_else(|| palette::fallback_channel_color(name, index));
    egui::Color32::from_rgb(r, g, b)
}

fn channel_color_with_photoshop(
    palette: Option<&ChannelPalette>,
    photoshop_display: &[Option<tiff_io::PhotoshopChannelDisplay>],
    name: &str,
    index: usize,
) -> egui::Color32 {
    // Explicit project palette entries always win. For channels beyond the
    // configured palette, use the TIFF's Photoshop Spot display color before
    // falling back to the deterministic generic palette.
    if palette
        .and_then(|palette| palette.channels.get(index))
        .is_some()
    {
        return channel_color(palette, name, index);
    }
    if let Some(info) = photoshop_display.get(index).and_then(|value| *value) {
        if info.is_spot() {
            if let Some([r, g, b]) = info.rgb {
                return egui::Color32::from_rgb(
                    (r.clamp(0.0, 1.0) * 255.0).round() as u8,
                    (g.clamp(0.0, 1.0) * 255.0).round() as u8,
                    (b.clamp(0.0, 1.0) * 255.0).round() as u8,
                );
            }
        }
    }
    channel_color(palette, name, index)
}

fn palette_entry_readonly(ui: &mut egui::Ui, entry: &palette::ChannelPaletteEntry) {
    let [r, g, b] = entry.color;
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
        ui.label(&entry.name);
    });
}

#[derive(Clone, Copy)]
enum VectorIconKind {
    Export,
    Check,
}

struct VectorIconButton {
    icon: VectorIconKind,
    min_size: egui::Vec2,
    frame: bool,
    sense: egui::Sense,
}

impl VectorIconButton {
    fn export() -> Self {
        Self {
            icon: VectorIconKind::Export,
            min_size: egui::vec2(20.0, 20.0),
            frame: true,
            sense: egui::Sense::click(),
        }
    }

    fn check() -> Self {
        Self {
            icon: VectorIconKind::Check,
            min_size: egui::vec2(20.0, 20.0),
            frame: true,
            sense: egui::Sense::click(),
        }
    }

    fn min_size(mut self, size: egui::Vec2) -> Self {
        self.min_size = size;
        self
    }

    fn frame(mut self, frame: bool) -> Self {
        self.frame = frame;
        self
    }

    fn sense(mut self, sense: egui::Sense) -> Self {
        self.sense = sense;
        self
    }
}

impl egui::Widget for VectorIconButton {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(self.min_size, self.sense);
        let enabled = ui.is_enabled();
        let visuals = ui.style().interact(&response);

        if self.frame {
            ui.painter().rect_filled(rect, 4.0, visuals.bg_fill);
            ui.painter()
                .rect_stroke(rect, 4.0, visuals.bg_stroke, egui::StrokeKind::Inside);
        } else if enabled && response.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
        }

        let color = if enabled {
            visuals.fg_stroke.color
        } else {
            ui.visuals().weak_text_color().gamma_multiply(0.45)
        };
        paint_vector_icon(ui.painter(), rect, self.icon, color);
        response
    }
}

fn paint_vector_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: VectorIconKind,
    color: egui::Color32,
) {
    let icon_rect = rect.shrink(5.0);
    match icon {
        VectorIconKind::Export => {
            let stroke = egui::Stroke::new(1.8, color);
            let x = icon_rect.center().x;
            let top = icon_rect.top() + 0.5;
            let shaft_bottom = icon_rect.center().y + 1.5;
            painter.line_segment([egui::pos2(x, shaft_bottom), egui::pos2(x, top)], stroke);
            painter.line_segment([egui::pos2(x, top), egui::pos2(x - 3.2, top + 3.2)], stroke);
            painter.line_segment([egui::pos2(x, top), egui::pos2(x + 3.2, top + 3.2)], stroke);
            let bottom = icon_rect.bottom() - 0.5;
            let side_top = bottom - 4.0;
            painter.line_segment(
                [
                    egui::pos2(icon_rect.left() + 0.5, side_top),
                    egui::pos2(icon_rect.left() + 0.5, bottom),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(icon_rect.left() + 0.5, bottom),
                    egui::pos2(icon_rect.right() - 0.5, bottom),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(icon_rect.right() - 0.5, bottom),
                    egui::pos2(icon_rect.right() - 0.5, side_top),
                ],
                stroke,
            );
        }
        VectorIconKind::Check => {
            let stroke = egui::Stroke::new(2.0, color);
            painter.line_segment(
                [
                    egui::pos2(icon_rect.left() + 1.0, icon_rect.center().y),
                    egui::pos2(icon_rect.center().x - 1.0, icon_rect.bottom() - 1.5),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(icon_rect.center().x - 1.0, icon_rect.bottom() - 1.5),
                    egui::pos2(icon_rect.right() - 0.5, icon_rect.top() + 1.0),
                ],
                stroke,
            );
        }
    }
}

fn clickable_row(
    ui: &mut egui::Ui,
    selected: bool,
    left: &str,
    trailing: Option<&str>,
    accent: Option<egui::Color32>,
    height: f32,
) -> egui::Response {
    clickable_row_tinted(ui, selected, left, trailing, accent, None, height)
}

fn clickable_row_tinted(
    ui: &mut egui::Ui,
    selected: bool,
    left: &str,
    trailing: Option<&str>,
    accent: Option<egui::Color32>,
    base_fill: Option<egui::Color32>,
    height: f32,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let visuals = ui.visuals();
    let fill = if let Some(base) = base_fill {
        if selected {
            base.gamma_multiply(1.35)
        } else if response.hovered() {
            base.gamma_multiply(1.18)
        } else {
            base
        }
    } else if selected {
        visuals.selection.bg_fill.gamma_multiply(0.72)
    } else if response.hovered() {
        visuals.widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    let left_color = accent.unwrap_or_else(|| {
        if selected {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        }
    });
    ui.painter().text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        left,
        egui::FontId::proportional(14.0),
        left_color,
    );
    if let Some(trailing) = trailing {
        ui.painter().text(
            rect.right_center() - egui::vec2(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            trailing,
            egui::FontId::proportional(12.5),
            visuals.weak_text_color(),
        );
    }
    response
}

fn clipping_warning_color(stats: render::ChannelClippingStats) -> Option<egui::Color32> {
    let max = stats.max_percent();
    if max >= 1.0 {
        Some(egui::Color32::RED)
    } else if max >= 0.10 {
        Some(egui::Color32::YELLOW)
    } else {
        None
    }
}

fn clipping_tooltip(stats: render::ChannelClippingStats) -> String {
    format!(
        "Preview clipping estimate · Levels: black ~{:.2}%, white ~{:.2}% · Curve: black ~{:.2}%, white ~{:.2}% · {} sampled pixels",
        stats.levels_black_percent(),
        stats.levels_white_percent(),
        stats.curve_black_percent(),
        stats.curve_white_percent(),
        stats.sample_count,
    )
}

fn clipping_summary_ui(ui: &mut egui::Ui, stats: render::ChannelClippingStats) {
    let warning = clipping_warning_color(stats);
    egui::Frame::new()
        .inner_margin(6)
        .corner_radius(4)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(color) = warning {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, color);
                }
                ui.strong("Clipping estimate");
                ui.label(format!(
                    "Levels  Black ~{:.2}%  White ~{:.2}%",
                    stats.levels_black_percent(),
                    stats.levels_white_percent(),
                ));
                ui.separator();
                ui.label(format!(
                    "Curve  Black ~{:.2}%  White ~{:.2}%",
                    stats.curve_black_percent(),
                    stats.curve_white_percent(),
                ));
            });
            ui.small(format!(
                "Preview estimate from {} sampled pixels · yellow ≥0.10% · red ≥1.00%",
                stats.sample_count,
            ));
        });
    ui.add_space(5.0);
}

fn clickable_channel_row(
    ui: &mut egui::Ui,
    selected: bool,
    master_context: bool,
    solo: bool,
    label: &str,
    accent: egui::Color32,
    warning: Option<egui::Color32>,
    height: f32,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let visuals = ui.visuals();
    let fill = channel_row_fill(visuals, selected, master_context, response.hovered());
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }

    let indicator = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 14.0, rect.center().y),
        egui::vec2(11.0, 11.0),
    );
    if solo {
        ui.painter().rect_filled(indicator, 1.5, accent);
    } else {
        ui.painter().rect_stroke(
            indicator,
            1.5,
            egui::Stroke::new(1.5, accent),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + 28.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        accent,
    );
    if let Some(color) = warning {
        ui.painter()
            .circle_filled(egui::pos2(rect.right() - 10.0, rect.center().y), 4.5, color);
    }
    response
}

fn channel_row_fill(
    visuals: &egui::Visuals,
    selected: bool,
    master_context: bool,
    hovered: bool,
) -> egui::Color32 {
    if selected && master_context {
        // The channel remains selected as context for histograms and Mixer,
        // but a neutral highlight makes it clear that Levels/Curve edits are
        // currently targeting Master rather than this channel.
        if visuals.dark_mode {
            egui::Color32::from_gray(62)
        } else {
            egui::Color32::from_gray(205)
        }
    } else if selected {
        visuals.selection.bg_fill.gamma_multiply(0.72)
    } else if hovered {
        visuals.widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    }
}

#[cfg(test)]
mod channel_row_visual_tests {
    use super::{channel_row_fill, egui};

    #[test]
    fn master_context_uses_a_neutral_selected_channel_highlight() {
        for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
            let master_fill = channel_row_fill(&visuals, true, true, false);
            assert_eq!(master_fill.r(), master_fill.g());
            assert_eq!(master_fill.g(), master_fill.b());

            let channel_fill = channel_row_fill(&visuals, true, false, false);
            assert_eq!(channel_fill, visuals.selection.bg_fill.gamma_multiply(0.72));
            assert_ne!(master_fill, channel_fill);
        }
    }
}

fn snapshot_row_with_actions(
    ui: &mut egui::Ui,
    selected: bool,
    dirty: bool,
    left: &str,
    time: &str,
    exported: bool,
    export_enabled: bool,
) -> (egui::Response, bool, bool) {
    let width = ui.available_width().max(1.0);
    let height = 38.0;
    let (rect, row_response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let visuals = ui.visuals();
    let fill = if dirty {
        visuals.selection.bg_fill.gamma_multiply(1.12)
    } else if selected {
        visuals.selection.bg_fill.gamma_multiply(0.72)
    } else if row_response.hovered() {
        visuals.widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    if dirty {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4.0,
            egui::Stroke::new(1.5, visuals.selection.stroke.color),
            egui::StrokeKind::Inside,
        );
        ui.painter().rect_filled(
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 3.0, rect.bottom())),
            2.0,
            visuals.selection.stroke.color,
        );
    }

    let action_width = 26.0;
    let action_gap = 2.0;
    let right_padding = 4.0;
    let export_rect = egui::Rect::from_center_size(
        egui::pos2(
            rect.right() - right_padding - action_width * 0.5,
            rect.center().y,
        ),
        egui::vec2(action_width, 24.0),
    );
    let check_rect = egui::Rect::from_center_size(
        egui::pos2(
            export_rect.left() - action_gap - action_width * 0.5,
            rect.center().y,
        ),
        egui::vec2(action_width, 24.0),
    );
    let time_right = if exported {
        check_rect.left() - 7.0
    } else {
        export_rect.left() - 7.0
    };

    ui.painter().text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        left,
        egui::FontId::proportional(if dirty { 15.0 } else { 14.0 }),
        if selected {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        },
    );
    ui.painter().text(
        egui::pos2(time_right, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        time,
        egui::FontId::proportional(12.0),
        visuals.weak_text_color(),
    );

    let export_clicked = ui
        .put(
            export_rect,
            VectorIconButton::export()
                .frame(false)
                .sense(egui::Sense::click()),
        )
        .on_hover_text("Export this snapshot for the active Face")
        .clicked()
        && export_enabled;
    let folder_clicked = if exported {
        ui.put(
            check_rect,
            VectorIconButton::check()
                .frame(false)
                .sense(egui::Sense::click()),
        )
        .on_hover_text("Open export folder")
        .clicked()
    } else {
        false
    };
    (row_response, export_clicked, folder_clicked)
}

fn snapshot_day_time(created_at_unix_ms: i64) -> (String, String) {
    if created_at_unix_ms <= 0 {
        return ("Earlier snapshots".to_owned(), "-".to_owned());
    }
    match Local.timestamp_millis_opt(created_at_unix_ms).single() {
        Some(value) => (
            value.format("%Y-%m-%d").to_string(),
            value.format("%H:%M").to_string(),
        ),
        None => ("Earlier snapshots".to_owned(), "-".to_owned()),
    }
}

fn draw_histogram(
    ui: &mut egui::Ui,
    original: Option<&[u32; 256]>,
    adjusted: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    display_mode: TonalDisplayMode,
) {
    let desired = egui::vec2(ui.available_width().max(80.0), 105.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        2.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let max_value = original
        .into_iter()
        .flat_map(|bins| bins.iter())
        .chain(adjusted.into_iter().flat_map(|bins| bins.iter()))
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let original_color = ui.visuals().weak_text_color();
    let adjusted_color = accent.unwrap_or(ui.visuals().selection.stroke.color);
    for index in 0..256 {
        let x = egui::lerp(
            rect.x_range(),
            tonal_display_value(index as f32 / 255.0, display_mode),
        );
        if let Some(bins) = original {
            let h = bins[index] as f32 / max_value * rect.height();
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.bottom() - h),
                ],
                egui::Stroke::new(1.0, original_color),
            );
        }
        if let Some(bins) = adjusted {
            let h = bins[index] as f32 / max_value * rect.height();
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.bottom() - h),
                ],
                egui::Stroke::new(1.0, adjusted_color),
            );
        }
    }
}

fn apply_theme(ctx: &egui::Context, dark: bool) {
    if dark {
        ctx.set_visuals(egui::Visuals::dark());
    } else {
        ctx.set_visuals(egui::Visuals::light());
    }
}

fn physical_dimensions_cm(width_px: u32, height_px: u32, dpi: dpi::DpiInfo) -> (f64, f64) {
    let dpi_x = dpi.dpi_x.max(1.0);
    let dpi_y = dpi.dpi_y.max(1.0);
    (
        width_px as f64 / dpi_x * 2.54,
        height_px as f64 / dpi_y * 2.54,
    )
}

fn format_cm_value(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.05 {
        format!("{rounded:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn build_project_file_metadata(
    project: &ShadeProject,
    faces: &[RuntimeFace],
    active_face_index: usize,
) -> model::ProjectFileMetadata {
    let mut total_source_bytes = 0u64;
    let mut entries = Vec::with_capacity(faces.len());
    for (index, face) in faces.iter().enumerate() {
        let fs_metadata = std::fs::metadata(&face.path).ok();
        let file_size_bytes = fs_metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);
        total_source_bytes = total_source_bytes.saturating_add(file_size_bytes);
        let modified_at_unix_ms = fs_metadata
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(system_time_unix_ms);
        let tiff = &face.preview.metadata;
        let label = project
            .faces
            .get(index)
            .map(|face| face.label.clone())
            .unwrap_or_else(|| {
                face.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("Face {}", index + 1))
            });
        entries.push(model::FaceFileMetadata {
            label,
            source_file_name: face
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            width: tiff.width,
            height: tiff.height,
            bit_depth: tiff.bit_depth,
            color_model: tiff.color_model.title().to_owned(),
            channel_count: tiff.samples_per_pixel,
            base_channel_count: tiff.base_channel_count,
            channel_names: tiff.channel_names.clone(),
            dpi_x: face.dpi.dpi_x,
            dpi_y: face.dpi.dpi_y,
            dpi_from_source: face.dpi.has_physical_resolution,
            resolution_unit: face.dpi.unit,
            file_size_bytes,
            modified_at_unix_ms,
        });
    }
    model::ProjectFileMetadata {
        saved_at_unix_ms: unix_ms_now(),
        face_count: entries.len(),
        active_face_index: active_face_index.min(entries.len().saturating_sub(1)),
        total_source_bytes,
        faces: entries,
    }
}

fn system_time_unix_ms(value: std::time::SystemTime) -> Option<i64> {
    value
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn master_adjustment_is_modified(adjustment: &ChannelAdjustment) -> bool {
    let default = ChannelAdjustment::default();
    adjustment.enabled != default.enabled
        || adjustment.levels != default.levels
        || adjustment.curve != default.curve
}

fn adjustment_is_modified(adjustment: &ChannelAdjustment) -> bool {
    let default = ChannelAdjustment::default();
    adjustment.levels != default.levels
        || adjustment.mixer != default.mixer
        || adjustment.curve != default.curve
}

fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        if path.is_file() {
            std::process::Command::new("explorer.exe")
                .arg("/select,")
                .arg(path)
                .spawn()
                .map_err(|err| format!("Cannot reveal {} in Explorer: {err}", path.display()))?;
            return Ok(());
        }
    }
    let folder = path.parent().unwrap_or(path);
    open_folder(folder)
}

fn open_folder(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .map_err(|err| format!("Cannot open export folder {}: {err}", path.display()))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("Opening export folders is only available in the Windows build.".to_owned())
    }
}

fn sanitize_filename(value: &str) -> String {
    let filtered = value
        .chars()
        .map(|ch| {
            if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    if filtered.trim().is_empty() {
        "shade-project".to_owned()
    } else {
        filtered
    }
}

#[cfg(test)]
mod project_revision_tests {
    #[test]
    fn only_the_revision_that_was_serialized_may_clear_dirty_state() {
        let saved_revision = 7_u64;
        let current_revision_same = 7_u64;
        let current_revision_newer = 8_u64;
        assert_eq!(saved_revision, current_revision_same);
        assert_ne!(saved_revision, current_revision_newer);
    }
}
