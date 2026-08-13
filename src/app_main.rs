#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

trait ContextKeyboardCompat {
    fn wants_keyboard_input(&self) -> bool;
}

impl ContextKeyboardCompat for eframe::egui::Context {
    fn wants_keyboard_input(&self) -> bool {
        self.egui_wants_keyboard_input()
    }
}

#[path = "app_log.rs"]
mod app_log;
#[path = "dpi.rs"]
mod dpi;
#[path = "export_v6.rs"]
mod export;
#[path = "history.rs"]
mod history;
#[path = "model_v6.rs"]
mod model;
#[path = "palette.rs"]
mod palette;
#[path = "recovery.rs"]
mod recovery;
#[path = "render.rs"]
mod render;
#[path = "settings_v6.rs"]
mod settings;
#[path = "thumbnail.rs"]
mod thumbnail;
#[path = "tiff_io.rs"]
mod tiff_io;
#[path = "update_v4.rs"]
mod update;
#[path = "validation.rs"]
mod validation;
#[path = "workflow_v0103.rs"]
mod workflow_v0103;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use eframe::egui;
use model::{ChannelAdjustment, ShadeProject, TEST_CODE_ALL_CHANNELS, TestCodePosition};
use palette::ChannelPalette;
use settings::AppSettings;
use tiff_io::PreviewFace;
use update::{UpdateManager, UpdateStatus};

const VIEWPORT_OVERSCROLL: f32 = 180.0;
const ERROR_TOAST_LIFETIME: Duration = Duration::from_secs(120);
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);
const HISTORY_COMMIT_DELAY: Duration = Duration::from_millis(300);

fn main() -> eframe::Result {
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
            .with_title("Shade Editor")
            .with_inner_size([1550.0, 920.0])
            .with_min_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Shade Editor",
        native_options,
        Box::new(move |cc| {
            let mut app = ShadeApp::new(cc);
            if let Some(path) = startup_project.clone() {
                app.open_project_path(path);
            }
            Ok(Box::new(app))
        }),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolPanel {
    Levels,
    Curves,
    Mixer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdjustmentScope {
    Selected,
    All,
}

struct RuntimeFace {
    path: PathBuf,
    available: bool,
    preview: Arc<PreviewFace>,
    dpi: dpi::DpiInfo,
    adjusted: Vec<Vec<u16>>,
    texture: Option<egui::TextureHandle>,
    original_texture: Option<egui::TextureHandle>,
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
        result: Result<(), String>,
    },
    Export(SnapshotExportBatchResult),
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

struct RenderResult {
    face_index: usize,
    generation: u64,
    adjusted: Vec<Vec<u16>>,
    rgba: Vec<u8>,
    original_rgba: Vec<u8>,
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
    show_about: bool,
    show_logs: bool,
    log: app_log::AppLog,
    log_cache: String,
    last_update_failure: Option<String>,
    toast: Option<ErrorToast>,
    status_message: String,
    project_dirty: bool,
    snapshot_rename_id: Option<u64>,
    snapshot_rename_buffer: String,
    pending_snapshot_load: Option<u64>,
    show_close_confirmation: bool,
    close_after_save: bool,
    allow_close_once: bool,
    history: history::AdjustmentHistory,
    history_pending_label: Option<String>,
    history_pending_at: Option<Instant>,
    recovery_candidate: Option<recovery::RecoveryFile>,
    autosave_tx: mpsc::Sender<Result<PathBuf, String>>,
    autosave_rx: mpsc::Receiver<Result<PathBuf, String>>,
    autosave_busy: bool,
    last_autosave: Instant,
    job: Option<JobHandle>,
    render_tx: mpsc::Sender<RenderResult>,
    render_rx: mpsc::Receiver<RenderResult>,
    render_busy: Option<(usize, u64)>,
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
        let mut project = ShadeProject::default();
        project.channel_palette = settings.default_project_palette();
        let log = app_log::AppLog::default();
        log.info(&format!(
            "Shade Editor {} started",
            env!("CARGO_PKG_VERSION")
        ));
        let recovery_candidate = match recovery::load() {
            Ok(candidate) => candidate,
            Err(err) => {
                log.error(&err);
                None
            }
        };
        let mut history = history::AdjustmentHistory::default();
        history.reset(&project.adjustments, "Start");
        Self {
            project,
            project_path: None,
            faces: Vec::new(),
            current_face: 0,
            selected_channel: 0,
            solo_channel: None,
            tool: ToolPanel::Levels,
            adjustment_scope: AdjustmentScope::Selected,
            zoom: 1.0,
            fit_requested: false,
            viewport_recenter: true,
            settings,
            updater,
            show_settings: false,
            show_about: false,
            show_logs: false,
            log,
            log_cache: String::new(),
            last_update_failure: None,
            toast: None,
            status_message: "Ready".to_owned(),
            project_dirty: false,
            snapshot_rename_id: None,
            snapshot_rename_buffer: String::new(),
            pending_snapshot_load: None,
            show_close_confirmation: false,
            close_after_save: false,
            allow_close_once: false,
            history,
            history_pending_label: None,
            history_pending_at: None,
            recovery_candidate,
            autosave_tx,
            autosave_rx,
            autosave_busy: false,
            last_autosave: Instant::now(),
            job: None,
            render_tx,
            render_rx,
            render_busy: None,
        }
    }

    fn report_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log.error(&message);
        self.status_message = message.clone();
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

    fn new_project(&mut self) {
        if self.job.is_some() {
            return;
        }
        self.project = ShadeProject::default();
        self.project.channel_palette = self.settings.default_project_palette();
        self.project_path = None;
        self.faces.clear();
        self.current_face = 0;
        self.selected_channel = 0;
        self.solo_channel = None;
        self.adjustment_scope = AdjustmentScope::Selected;
        self.viewport_recenter = true;
        self.fit_requested = true;
        self.project_dirty = false;
        self.snapshot_rename_id = None;
        self.snapshot_rename_buffer.clear();
        self.pending_snapshot_load = None;
        self.show_close_confirmation = false;
        self.close_after_save = false;
        self.history.reset(&self.project.adjustments, "New project");
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.report_info("New shade project");
    }

    fn make_runtime_face(item: LoadedFace) -> RuntimeFace {
        RuntimeFace {
            path: item.path,
            available: item.available,
            preview: Arc::new(item.preview),
            dpi: item.dpi,
            adjusted: Vec::new(),
            texture: None,
            original_texture: None,
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
            let result = task(worker_progress);
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

    fn rebuild_previews(&mut self) {
        workflow_v0103::rebuild_previews(self);
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
        self.open_project_path(path);
    }

    fn open_project_path(&mut self, path: PathBuf) {
        if self.job.is_some() {
            return;
        }
        self.recovery_candidate = None;
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
                        &source
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
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
                            faces.push(workflow_v0103::placeholder_loaded_face(
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
        let mut project = self.project.clone();
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
                result,
            }
        });
        true
    }

    fn export_current_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        if !workflow_v0103::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting.",
            );
            return;
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
        let project = self.project.clone();
        let default_dpi = self.settings.default_dpi;
        let validate_after_export = self.settings.validate_after_export;
        self.launch_job("Exporting TIFF", move |progress| {
            let result = export::export_face_with_progress(
                &source,
                &destination,
                &project,
                default_dpi,
                |fraction, detail| {
                    let fraction = if validate_after_export {
                        fraction * 0.88
                    } else {
                        fraction
                    };
                    Self::set_progress(&progress, Some(fraction), "Exporting TIFF", detail);
                },
            )
            .and_then(|_| {
                if validate_after_export {
                    Self::set_progress(
                        &progress,
                        Some(0.92),
                        "Validating exported TIFF",
                        "Decoding strips and checking production metadata",
                    );
                    let verified = validation::validate_export_transport(&source, &destination)?;
                    Ok(format!("Exported {} · {verified}", destination.display()))
                } else {
                    Ok(format!("Exported {}", destination.display()))
                }
            });
            JobResult::Export(SnapshotExportBatchResult {
                result,
                marks: Vec::new(),
            })
        });
    }

    fn validate_current_face_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        if !workflow_v0103::active_face_available(self) {
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
        self.launch_job("Validating TIFF", move |progress| {
            let result = validation::validate_no_adjustment_roundtrip(
                &source,
                &folder,
                default_dpi,
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
        if self.faces.iter().any(|face| !face.available) {
            self.report_error("Export all requires every Face source TIFF to be available. Relink missing Faces first.");
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let sources = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        let project = self.project.clone();
        let default_dpi = self.settings.default_dpi;
        let validate_after_export = self.settings.validate_after_export;
        self.launch_job("Exporting faces", move |progress| {
            let total = sources.len().max(1);
            let result = (|| -> Result<String, String> {
                for (index, source) in sources.iter().enumerate() {
                    let stem = source
                        .file_stem()
                        .map(|value| value.to_string_lossy())
                        .unwrap_or_default();
                    let destination = folder.join(format!("{stem}-shade.tif"));
                    export::export_face_with_progress(
                        source,
                        &destination,
                        &project,
                        default_dpi,
                        |inner, detail| {
                            let phase = if validate_after_export {
                                inner * 0.88
                            } else {
                                inner
                            };
                            let overall = (index as f32 + phase) / total as f32;
                            Self::set_progress(&progress, Some(overall), "Exporting faces", detail);
                        },
                    )?;
                    if validate_after_export {
                        let overall = (index as f32 + 0.92) / total as f32;
                        Self::set_progress(
                            &progress,
                            Some(overall),
                            "Validating exported TIFF",
                            &destination.display().to_string(),
                        );
                        validation::validate_export_transport(source, &destination)?;
                    }
                }
                if validate_after_export {
                    Ok(format!(
                        "Exported and verified {total} face(s) to {}",
                        folder.display()
                    ))
                } else {
                    Ok(format!("Exported {total} face(s) to {}", folder.display()))
                }
            })();
            JobResult::Export(SnapshotExportBatchResult {
                result,
                marks: Vec::new(),
            })
        });
    }

    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {
        if self.job.is_some() {
            return;
        }
        if !workflow_v0103::active_face_available(self) {
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
        let suggested = format!(
            "{}-{}.tif",
            sanitize_filename(&stem),
            sanitize_filename(&snapshot.name)
        );
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
        let default_dpi = self.settings.default_dpi;
        project.adjustments = snapshot.adjustments.clone();
        project.active_snapshot_id = Some(snapshot.id);
        self.launch_job("Exporting snapshot", move |progress| {
            let result = export::export_face_with_progress(
                &source,
                &destination,
                &project,
                default_dpi,
                |fraction, detail| {
                    Self::set_progress(&progress, Some(fraction), "Exporting snapshot", detail);
                },
            )
            .map(|_| format!("Exported {}", destination.display()));
            let marks = if result.is_ok() {
                vec![SnapshotExportMark {
                    snapshot_id,
                    face_key,
                    folder,
                    exported_at_unix_ms: unix_ms_now(),
                }]
            } else {
                Vec::new()
            };
            JobResult::Export(SnapshotExportBatchResult { result, marks })
        });
    }

    fn export_snapshot_group_dialog(&mut self, snapshot_ids: Vec<u64>, label: String) {
        if self.job.is_some() || snapshot_ids.is_empty() {
            return;
        }
        if !workflow_v0103::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let source = face.path.clone();
        let face_key = source.to_string_lossy().into_owned();
        let stem = source
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "face".to_owned());
        let base_project = self.project.clone();
        let default_dpi = self.settings.default_dpi;
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
        self.launch_job("Exporting snapshots", move |progress| {
            let total = snapshots.len().max(1);
            let mut marks = Vec::new();
            let result = (|| -> Result<String, String> {
                for (index, snapshot) in snapshots.iter().enumerate() {
                    let destination = folder.join(format!(
                        "{}-{}.tif",
                        sanitize_filename(&stem),
                        sanitize_filename(&snapshot.name)
                    ));
                    let mut project = base_project.clone();
                    project.adjustments = snapshot.adjustments.clone();
                    project.active_snapshot_id = Some(snapshot.id);
                    export::export_face_with_progress(
                        &source,
                        &destination,
                        &project,
                        default_dpi,
                        |inner, detail| {
                            let overall = (index as f32 + inner) / total as f32;
                            Self::set_progress(
                                &progress,
                                Some(overall),
                                "Exporting snapshots",
                                &format!("{} · {detail}", snapshot.name),
                            );
                        },
                    )?;
                    marks.push(SnapshotExportMark {
                        snapshot_id: snapshot.id,
                        face_key: face_key.clone(),
                        folder: folder.clone(),
                        exported_at_unix_ms: unix_ms_now(),
                    });
                }
                Ok(format!(
                    "Exported {} snapshot(s) ({label}) to {}",
                    snapshots.len(),
                    folder.display()
                ))
            })();
            JobResult::Export(SnapshotExportBatchResult { result, marks })
        });
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
        self.project_dirty = true;
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
                    });
                    self.faces.push(Self::make_runtime_face(item));
                }
                if added > 0 {
                    self.current_face = self.faces.len().saturating_sub(added);
                    self.selected_channel = 0;
                    self.solo_channel = None;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.project_dirty = true;
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
                workflow_v0103::apply_relinked_face(self, index, result);
            }
            JobResult::RelinkFolder { faces, errors } => {
                workflow_v0103::apply_relinked_folder(self, faces, errors);
            }
            JobResult::Open(result) => match result {
                Ok(payload) => {
                    self.project = payload.project;
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
                    self.adjustment_scope = AdjustmentScope::Selected;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.project_dirty = false;
                    self.history
                        .reset(&self.project.adjustments, "Open project");
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
                Err(err) => self.report_error(err),
            },
            JobResult::Recover(result) => match result {
                Ok(payload) => {
                    self.project = payload.project;
                    self.project_path = payload.origin_path;
                    self.faces = payload
                        .faces
                        .into_iter()
                        .map(Self::make_runtime_face)
                        .collect();
                    self.current_face = 0;
                    self.selected_channel = 0;
                    self.solo_channel = None;
                    self.adjustment_scope = AdjustmentScope::Selected;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.project_dirty = true;
                    self.history
                        .reset(&self.project.adjustments, "Recovered project");
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
            JobResult::Save { path, result } => match result {
                Ok(()) => {
                    self.project_path = Some(path.clone());
                    self.project_dirty = false;
                    if let Err(err) = recovery::clear() {
                        self.log.error(&err);
                    }
                    self.report_info(format!("Saved {}", path.display()));
                }
                Err(err) => {
                    self.close_after_save = false;
                    self.report_error(err);
                }
            },
            JobResult::Export(payload) => {
                if !payload.marks.is_empty() {
                    for mark in payload.marks {
                        self.project.record_snapshot_export(
                            mark.snapshot_id,
                            mark.face_key,
                            mark.folder.to_string_lossy().into_owned(),
                            mark.exported_at_unix_ms,
                        );
                    }
                    self.project_dirty = true;
                }
                match payload.result {
                    Ok(message) => self.report_info(message),
                    Err(err) => self.report_error(format!("Export failed: {err}")),
                }
            }
        }
    }

    fn mark_all_previews_dirty(&mut self) {
        for face in &mut self.faces {
            face.generation = face.generation.wrapping_add(1).max(1);
        }
        self.project_dirty = true;
    }

    fn mark_current_preview_dirty(&mut self) {
        if let Some(face) = self.faces.get_mut(self.current_face) {
            face.generation = face.generation.wrapping_add(1).max(1);
        }
    }

    fn poll_render(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.render_rx.try_recv() {
            if self.render_busy == Some((result.face_index, result.generation)) {
                self.render_busy = None;
            }
            let Some(face) = self.faces.get_mut(result.face_index) else {
                continue;
            };
            if face.generation != result.generation {
                continue;
            }
            face.adjusted = result.adjusted;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.rgba,
            );
            let options = egui::TextureOptions::LINEAR;
            if let Some(texture) = &mut face.texture {
                texture.set(image, options);
            } else {
                face.texture = Some(ctx.load_texture(
                    format!("face-preview-{}", result.face_index),
                    image,
                    options,
                ));
            }
            let original_image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.original_rgba,
            );
            if let Some(texture) = &mut face.original_texture {
                texture.set(original_image, options);
            } else {
                face.original_texture = Some(ctx.load_texture(
                    format!("face-original-preview-{}", result.face_index),
                    original_image,
                    options,
                ));
            }
            face.rendered_generation = result.generation;
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
        let preview = Arc::clone(&face.preview);
        let project = self.project.clone();
        let solo_channel = self.solo_channel;
        let tx = self.render_tx.clone();
        self.render_busy = Some((face_index, generation));
        std::thread::spawn(move || {
            let adjusted = render::adjusted_planes(&preview, &project);
            let rgba = render::rgba_from_planes(&preview, &adjusted, solo_channel);
            let original_rgba = render::rgba_from_planes(&preview, &preview.channels, solo_channel);
            let _ = tx.send(RenderResult {
                face_index,
                generation,
                adjusted,
                rgba,
                original_rgba,
            });
        });
    }

    fn select_channel(&mut self, channel: usize, isolate: bool) {
        let previous_solo = self.solo_channel;
        if isolate {
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
        self.faces.remove(self.current_face);
        if self.current_face < self.project.faces.len() {
            self.project.faces.remove(self.current_face);
        }
        self.current_face = self.current_face.min(self.faces.len().saturating_sub(1));
        self.selected_channel = 0;
        self.solo_channel = None;
        self.fit_requested = true;
        self.viewport_recenter = true;
        self.project_dirty = true;
        self.report_info("Face removed from project (source TIFF was not deleted)");
    }

    fn save_settings_quietly(&mut self) {
        if let Err(err) = self.settings.save() {
            self.report_error(err);
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
        }
    }

    fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.horizontal_wrapped(|ui| {
                let enabled = self.job.is_none();
                if ui.add_enabled(enabled, egui::Button::new("New")).clicked() {
                    self.new_project();
                }
                if ui
                    .add_enabled(enabled, egui::Button::new("Open .shade"))
                    .clicked()
                {
                    self.open_project_dialog();
                }
                if ui
                    .add_enabled(enabled, egui::Button::new("Add TIFF faces"))
                    .clicked()
                {
                    self.add_faces_dialog();
                }
                ui.separator();
                if ui
                    .add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save"))
                    .clicked()
                {
                    self.save_project(false);
                }
                if ui
                    .add_enabled(
                        enabled && !self.faces.is_empty(),
                        egui::Button::new("Save As"),
                    )
                    .clicked()
                {
                    self.save_project(true);
                }
                ui.separator();
                if ui
                    .add_enabled(
                        enabled && !self.faces.is_empty(),
                        egui::Button::new("Export face"),
                    )
                    .clicked()
                {
                    self.export_current_dialog();
                }
                if ui
                    .add_enabled(
                        enabled && !self.faces.is_empty(),
                        egui::Button::new("Export all"),
                    )
                    .clicked()
                {
                    self.export_all_dialog();
                }
                if ui
                    .add_enabled(
                        enabled && !self.faces.is_empty(),
                        egui::Button::new("Validate face"),
                    )
                    .on_hover_text("Run a no-adjustment export through the production TIFF backend, re-decode it, and compare pixels plus critical Photoshop/TIFF metadata.")
                    .clicked()
                {
                    self.validate_current_face_dialog();
                }
                ui.separator();
                if ui.button("Settings").clicked() {
                    self.show_settings = true;
                }
                if ui.button("About").clicked() {
                    self.show_about = true;
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Logs").clicked() {
                    self.log_cache = self.log.read();
                    self.show_logs = true;
                }
                self.ui_update_compact(ui);
                self.ui_operation_progress(ui);
                if let Some(toast) = &self.toast {
                    ui.label(
                        egui::RichText::new(&toast.message)
                            .color(egui::Color32::LIGHT_RED)
                            .small(),
                    );
                }
            });
        });
    }

    fn ui_operation_progress(&self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            if let Ok(progress) = job.progress.lock() {
                let value = progress.fraction.unwrap_or(0.5);
                let text = if progress.detail.is_empty() {
                    progress.label.clone()
                } else {
                    format!("{} · {}", progress.label, progress.detail)
                };
                ui.add(
                    egui::ProgressBar::new(value)
                        .desired_width(175.0)
                        .text(text)
                        .animate(progress.fraction.is_none()),
                );
                return;
            }
        }
        if self.render_busy.is_some() {
            ui.add(
                egui::ProgressBar::new(0.45)
                    .desired_width(145.0)
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
                        .desired_width(125.0)
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
                        .desired_width(150.0)
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
            let history_label = self
                .project
                .active_snapshot_name()
                .map(|name| format!("Snapshot - {name}"))
                .unwrap_or_else(|| "Snapshot".to_owned());
            self.history.reset(&self.project.adjustments, history_label);
            self.history_pending_label = None;
            self.history_pending_at = None;
            self.report_info("Snapshot loaded");
        }
    }

    fn request_snapshot_load(&mut self, id: u64) {
        if self.project.active_snapshot_id == Some(id) {
            return;
        }
        let active_snapshot_dirty =
            self.project.active_snapshot_id.is_some() && !self.project.active_snapshot_matches();
        if active_snapshot_dirty {
            self.pending_snapshot_load = Some(id);
        } else {
            self.apply_snapshot_now(id);
        }
    }

    fn ui_snapshot_discard_confirmation(&mut self, ctx: &egui::Context) {
        let Some(target_id) = self.pending_snapshot_load else {
            return;
        };
        let current_name = self
            .project
            .active_snapshot_name()
            .unwrap_or("Current snapshot")
            .to_owned();
        let target_name = self
            .project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == target_id)
            .map(|snapshot| snapshot.name.clone())
            .unwrap_or_else(|| "selected snapshot".to_owned());
        let mut stay = false;
        let mut discard = false;
        egui::Window::new("Snapshot changes not updated")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{current_name} has adjustment changes that have not been written back with Update."
                ));
                ui.label(format!("Switch to {target_name}?"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    stay = ui.button("Stay editing").clicked();
                    discard = ui.button("Discard changes and switch").clicked();
                });
            });
        if stay {
            self.pending_snapshot_load = None;
        } else if discard {
            self.pending_snapshot_load = None;
            self.apply_snapshot_now(target_id);
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        if self.allow_close_once {
            self.allow_close_once = false;
            return;
        }
        if self.project_dirty {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.show_close_confirmation = true;
        }
    }

    fn ui_close_confirmation(&mut self, ctx: &egui::Context) {
        if !self.show_close_confirmation {
            return;
        }
        let mut save_and_exit = false;
        let mut discard_and_exit = false;
        let mut stay = false;
        egui::Window::new("Unsaved project changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("This .shade project has changes that have not been saved.");
                if self.job.is_some() {
                    ui.small("Wait for the current operation to finish before saving.");
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    save_and_exit = ui
                        .add_enabled(
                            self.job.is_none() && !self.faces.is_empty(),
                            egui::Button::new("Save and exit"),
                        )
                        .clicked();
                    discard_and_exit = ui.button("Discard and exit").clicked();
                    stay = ui.button("Stay").clicked();
                });
            });

        if stay {
            self.show_close_confirmation = false;
        } else if discard_and_exit {
            self.show_close_confirmation = false;
            if let Err(err) = recovery::clear() {
                self.log.error(&err);
            }
            self.allow_close_once = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if save_and_exit && self.save_project(false) {
            self.show_close_confirmation = false;
            self.close_after_save = true;
        }
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
        self.history.record(&self.project.adjustments, label);
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
        self.mark_all_previews_dirty();
        self.report_info(message);
    }

    fn undo_adjustment(&mut self, ctx: &egui::Context) {
        self.commit_pending_history(ctx, true);
        if let Some(adjustments) = self.history.undo() {
            self.apply_history_adjustments(adjustments, "Undo adjustment");
        }
    }

    fn redo_adjustment(&mut self, ctx: &egui::Context) {
        self.commit_pending_history(ctx, true);
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

    fn ui_history(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("History");
            if ui
                .add_enabled(self.history.can_undo(), egui::Button::new("Undo").small())
                .on_hover_text("Ctrl+Alt+Z")
                .clicked()
            {
                self.undo_adjustment(ui.ctx());
            }
            if ui
                .add_enabled(self.history.can_redo(), egui::Button::new("Redo").small())
                .on_hover_text("Ctrl+Shift+Z")
                .clicked()
            {
                self.redo_adjustment(ui.ctx());
            }
        });
        ui.small("Adjustment history only. Faces, Snapshots and Palette changes are intentionally excluded.");
        let rows = self
            .history
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| (index, entry.label.clone()))
            .collect::<Vec<_>>();
        let cursor = self.history.cursor();
        let mut requested = None;
        egui::ScrollArea::vertical()
            .id_salt("adjustment-history")
            .max_height(210.0)
            .show(ui, |ui| {
                for (index, label) in rows {
                    if clickable_row(ui, index == cursor, &label, None, None, 28.0).clicked() {
                        requested = Some(index);
                    }
                }
            });
        if let Some(index) = requested {
            self.commit_pending_history(ui.ctx(), true);
            if let Some(adjustments) = self.history.jump(index) {
                self.apply_history_adjustments(adjustments, "History state selected");
            }
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
            || self.last_autosave.elapsed() < AUTOSAVE_INTERVAL
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

    fn recover_project(&mut self) {
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
                            faces.push(workflow_v0103::placeholder_loaded_face(
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
        workflow_v0103::ui_faces(self, ui);
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
            new_snapshot = ui.small_button("+ New").clicked();
            export_all = ui
                .add_enabled(
                    self.job.is_none() && !all_ids.is_empty() && !self.faces.is_empty(),
                    VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                )
                .on_hover_text("Export all snapshots for the active Face")
                .clicked();
            if all_exported {
                open_all_folder = ui
                    .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                    .on_hover_text("Open the latest export folder for these snapshots")
                    .clicked();
            }
        });
        ui.small(
            "Saved adjustment test states. Export always remains available after a successful run.",
        );
        ui.add_space(4.0);

        if new_snapshot {
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
            self.project_dirty = true;
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
                if day_exported
                    && ui
                        .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                        .on_hover_text("Open the latest export folder for this day")
                        .clicked()
                {
                    requested_folder = day_latest_folder.clone();
                }
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
                    self.project_dirty = true;
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
            workflow_v0103::update_active_snapshot(self);
        }
        if delete && self.project.delete_snapshot(active_id) {
            self.snapshot_rename_id = None;
            self.snapshot_rename_buffer.clear();
            self.project_dirty = true;
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
                    "All channels".to_owned()
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
                                "All channels",
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
            self.project_dirty = true;
        }
    }

    fn ui_channels_histogram(&mut self, ui: &mut egui::Ui) {
        let Some(face) = self.faces.get(self.current_face) else {
            ui.heading("Channels");
            ui.label("No active face");
            return;
        };
        if !face.available {
            ui.heading("Channels");
            ui.label("Source TIFF missing. Relink this Face to inspect channels and histograms.");
            return;
        }
        let channel_names = face.preview.metadata.channel_names.clone();
        let original_histograms = face.preview.histograms.clone();
        let adjusted_histograms = face
            .adjusted
            .iter()
            .map(|values| render::histogram(values))
            .collect::<Vec<_>>();
        let base_count = face.preview.metadata.base_channel_count;
        let color_model = face.preview.metadata.color_model;
        let photoshop_display = face.preview.metadata.channel_display_info.clone();
        if channel_names.is_empty() {
            return;
        }
        self.selected_channel = self.selected_channel.min(channel_names.len() - 1);
        let mut active_palette = self.project.channel_palette.clone();
        let palette_library = self.settings.palette_library();

        ui.horizontal(|ui| {
            ui.heading("Channels");
            let selected = active_palette
                .as_ref()
                .map(|palette| palette.name.as_str())
                .unwrap_or("TIFF channel names");
            let mut requested_palette = None;
            egui::ComboBox::from_id_salt("project-channel-palette")
                .selected_text(selected)
                .width(155.0)
                .show_ui(ui, |ui| {
                    for palette in &palette_library {
                        if ui
                            .selectable_label(
                                active_palette
                                    .as_ref()
                                    .is_some_and(|current| current.id == palette.id),
                                &palette.name,
                            )
                            .clicked()
                        {
                            requested_palette = Some(palette.clone());
                        }
                    }
                });
            if let Some(palette) = requested_palette {
                active_palette = Some(palette.clone());
                self.select_project_palette(palette);
            }
        });
        if clickable_row(
            ui,
            self.solo_channel.is_none(),
            "Composite",
            None,
            None,
            32.0,
        )
        .clicked()
        {
            self.show_composite();
        }
        ui.small(format!(
            "{} + {} extra",
            color_model.title(),
            channel_names.len().saturating_sub(base_count)
        ));
        ui.add_space(3.0);
        for (index, name) in channel_names.iter().enumerate() {
            let display_info = photoshop_display.get(index).and_then(|value| *value);
            let suffix = if index >= base_count {
                match display_info {
                    Some(info) if info.is_spot() => "  spot",
                    Some(_) => "  alpha",
                    None => "  extra",
                }
            } else {
                ""
            };
            let accent = channel_color_with_photoshop(
                active_palette.as_ref(),
                &photoshop_display,
                name,
                index,
            );
            let is_solo = self.solo_channel == Some(index);
            let display_name = channel_display_name(active_palette.as_ref(), name, index);
            let label = format!("{display_name}{suffix}");
            let hover = match display_info {
                Some(info) if info.is_spot() => format!(
                    "Photoshop Spot Channel · Solidity {:.0}% · click to select; click again to toggle solo preview.",
                    info.solidity * 100.0
                ),
                Some(_) => "Photoshop Alpha/auxiliary channel · click to select; click again to toggle solo preview.".to_owned(),
                None => "Extra TIFF channel (Spot/Alpha type not declared) · click to select; click again to toggle solo preview.".to_owned(),
            };
            let response = clickable_channel_row(
                ui,
                self.selected_channel == index,
                is_solo,
                &label,
                accent,
                32.0,
            )
            .on_hover_text(hover);
            if response.clicked() {
                self.select_channel(index, true);
            }
        }
        if self.solo_channel.is_some() && ui.small_button("Return to composite").clicked() {
            self.show_composite();
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Histogram");
            let label = if self.settings.show_all_histograms {
                "All channels"
            } else {
                "Selected"
            };
            if ui.small_button(label).clicked() {
                self.settings.show_all_histograms = !self.settings.show_all_histograms;
                self.save_settings_quietly();
            }
        });
        if self.settings.show_all_histograms {
            for (index, name) in channel_names.iter().enumerate() {
                let accent = self.settings.colorize_histograms.then(|| {
                    channel_color_with_photoshop(
                        active_palette.as_ref(),
                        &photoshop_display,
                        name,
                        index,
                    )
                });
                let display = channel_display_name(active_palette.as_ref(), name, index);
                ui.colored_label(accent.unwrap_or(ui.visuals().text_color()), display);
                draw_histogram(
                    ui,
                    original_histograms.get(index),
                    adjusted_histograms.get(index),
                    accent,
                );
            }
        } else {
            let index = self.selected_channel;
            let accent = self.settings.colorize_histograms.then(|| {
                channel_color_with_photoshop(
                    active_palette.as_ref(),
                    &photoshop_display,
                    &channel_names[index],
                    index,
                )
            });
            let display =
                channel_display_name(active_palette.as_ref(), &channel_names[index], index);
            ui.strong(format!("Histogram - {display}"));
            draw_histogram(
                ui,
                original_histograms.get(index),
                adjusted_histograms.get(index),
                accent,
            );
        }
    }
    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {
        let adjustments_before = self.project.adjustments.clone();
        let Some(face) = self.faces.get(self.current_face) else {
            ui.heading("Adjustments");
            ui.label("No active face");
            return;
        };
        if !face.available {
            ui.heading("Adjustments");
            ui.label("Source TIFF missing. Relink this Face before editing its channels.");
            return;
        }
        let channel_names = face.preview.metadata.channel_names.clone();
        if channel_names.is_empty() {
            return;
        }
        self.selected_channel = self.selected_channel.min(channel_names.len() - 1);
        let output_name = channel_names[self.selected_channel].clone();
        let palette = self.project.channel_palette.clone();
        let output_display =
            channel_display_name(palette.as_ref(), &output_name, self.selected_channel);
        let all_adjusted_histograms = face
            .adjusted
            .iter()
            .map(|values| render::histogram(values))
            .collect::<Vec<_>>();
        let active_histogram = all_adjusted_histograms.get(self.selected_channel).copied();
        let control_accent = self
            .settings
            .colorize_adjustments
            .then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));
        let panel_accent = (self.adjustment_scope == AdjustmentScope::Selected)
            .then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));

        ui.horizontal_wrapped(|ui| {
            ui.heading("Adjustments");
            ui.selectable_value(
                &mut self.adjustment_scope,
                AdjustmentScope::Selected,
                output_display,
            );
            ui.selectable_value(
                &mut self.adjustment_scope,
                AdjustmentScope::All,
                "All channels",
            );
            let layout_label = if self.settings.adjustment_tabs {
                "Tabs"
            } else {
                "Stacked"
            };
            if ui.small_button(layout_label).clicked() {
                self.settings.adjustment_tabs = !self.settings.adjustment_tabs;
                self.save_settings_quietly();
            }
        });

        let mut frame = egui::Frame::new().inner_margin(8).corner_radius(6);
        if let Some(color) = panel_accent {
            frame = frame.stroke(egui::Stroke::new(1.5, color.gamma_multiply(0.72)));
        } else {
            frame = frame.stroke(ui.visuals().widgets.noninteractive.bg_stroke);
        }
        let changed = frame
            .show(ui, |ui| {
                if let Some(color) = panel_accent {
                    ui.visuals_mut().widgets.noninteractive.bg_stroke.color =
                        color.gamma_multiply(0.52);
                }
                let reset_all = ui
                    .horizontal(|ui| {
                        if let Some(color) = panel_accent {
                            ui.colored_label(color, format!("Editing: {output_display}"));
                        } else {
                            ui.strong("Editing: All channels");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.small_button("Reset all").clicked()
                        })
                        .inner
                    })
                    .inner;
                if reset_all {
                    self.project.reset_adjustments(&channel_names);
                    self.mark_all_previews_dirty();
                    self.report_info("All adjustments reset to defaults");
                }
                match self.adjustment_scope {
                    AdjustmentScope::Selected => self.ui_selected_adjustment(
                        ui,
                        &output_name,
                        &channel_names,
                        active_histogram.as_ref(),
                        control_accent,
                        palette.as_ref(),
                    ),
                    AdjustmentScope::All => self.ui_all_adjustments(
                        ui,
                        &output_name,
                        &channel_names,
                        &all_adjusted_histograms,
                        control_accent,
                        palette.as_ref(),
                    ),
                }
            })
            .inner;
        if changed {
            self.mark_all_previews_dirty();
        }
        if self.project.adjustments != adjustments_before {
            self.queue_adjustment_history(&adjustments_before);
        }
    }
    fn ui_selected_adjustment(
        &mut self,
        ui: &mut egui::Ui,
        output_name: &str,
        channel_names: &[String],
        histogram: Option<&[u32; 256]>,
        accent: Option<egui::Color32>,
        palette: Option<&ChannelPalette>,
    ) -> bool {
        let mut changed = false;
        let compact_curve_controls = self.settings.compact_curve_controls;
        let adjustment = self
            .project
            .adjustments
            .entry(output_name.to_owned())
            .or_default();
        changed |= ui
            .checkbox(
                &mut adjustment.enabled,
                "Enable adjustment for this channel",
            )
            .changed();
        ui.add_enabled_ui(adjustment.enabled, |ui| {
            if self.settings.adjustment_tabs {
                let mut reset_tool = false;
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                    ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                    ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        reset_tool = ui.small_button("Reset").clicked();
                    });
                });
                if reset_tool {
                    match self.tool {
                        ToolPanel::Levels => adjustment.levels = model::Levels::default(),
                        ToolPanel::Curves => adjustment.curve = model::Curve::default(),
                        ToolPanel::Mixer => reset_mixer_row(adjustment, output_name, channel_names),
                    }
                    changed = true;
                }
                changed |= match self.tool {
                    ToolPanel::Levels => levels_ui(ui, adjustment, accent),
                    ToolPanel::Curves => curves_ui(
                        ui,
                        adjustment,
                        histogram.filter(|_| self.settings.show_curve_histogram),
                        accent,
                        compact_curve_controls,
                    ),
                    ToolPanel::Mixer => {
                        mixer_ui(ui, adjustment, output_name, channel_names, accent, palette)
                    }
                };
            } else {
                let (body_changed, reset) = adjustment_foldout(
                    ui,
                    format!("selected-levels-{output_name}"),
                    "Levels",
                    true,
                    |ui| levels_ui(ui, adjustment, accent),
                );
                changed |= body_changed.unwrap_or(false);
                if reset {
                    adjustment.levels = model::Levels::default();
                    changed = true;
                }

                ui.add_space(4.0);
                let (body_changed, reset) = adjustment_foldout(
                    ui,
                    format!("selected-curve-{output_name}"),
                    "Curve",
                    true,
                    |ui| {
                        curves_ui(
                            ui,
                            adjustment,
                            histogram.filter(|_| self.settings.show_curve_histogram),
                            accent,
                            compact_curve_controls,
                        )
                    },
                );
                changed |= body_changed.unwrap_or(false);
                if reset {
                    adjustment.curve = model::Curve::default();
                    changed = true;
                }

                ui.add_space(4.0);
                let (body_changed, reset) = adjustment_foldout(
                    ui,
                    format!("selected-mixer-{output_name}"),
                    "Channel Mixer",
                    true,
                    |ui| mixer_ui(ui, adjustment, output_name, channel_names, accent, palette),
                );
                changed |= body_changed.unwrap_or(false);
                if reset {
                    reset_mixer_row(adjustment, output_name, channel_names);
                    changed = true;
                }
            }
        });
        changed
    }

    fn ui_all_adjustments(
        &mut self,
        ui: &mut egui::Ui,
        template_name: &str,
        channel_names: &[String],
        histograms: &[[u32; 256]],
        accent: Option<egui::Color32>,
        palette: Option<&ChannelPalette>,
    ) -> bool {
        let mut changed = false;
        let compact_curve_controls = self.settings.compact_curve_controls;
        let enabled_count = channel_names
            .iter()
            .filter(|name| {
                self.project
                    .adjustments
                    .get(*name)
                    .map(|adjustment| adjustment.enabled)
                    .unwrap_or(true)
            })
            .count();
        let mut all_enabled = enabled_count == channel_names.len();
        if ui
            .checkbox(&mut all_enabled, "Enable adjustments on all channels")
            .changed()
        {
            for name in channel_names {
                self.project
                    .adjustments
                    .entry(name.clone())
                    .or_default()
                    .enabled = all_enabled;
            }
            changed = true;
        }
        ui.small(
            "Levels broadcasts to every channel. Curve keeps one Broadcast control plus independent per-channel foldouts. Mixer output rows remain independent.",
        );

        if self.settings.adjustment_tabs {
            let mut reset_tool = false;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_tool = ui.small_button("Reset").clicked();
                });
            });
            if reset_tool {
                match self.tool {
                    ToolPanel::Levels => {
                        reset_all_levels(&mut self.project.adjustments, channel_names)
                    }
                    ToolPanel::Curves => {
                        reset_all_curves(&mut self.project.adjustments, channel_names)
                    }
                    ToolPanel::Mixer => {
                        reset_all_mixers(&mut self.project.adjustments, channel_names)
                    }
                }
                changed = true;
            }
            changed |= match self.tool {
                ToolPanel::Levels => broadcast_levels_ui(
                    ui,
                    &mut self.project.adjustments,
                    template_name,
                    channel_names,
                    accent,
                ),
                ToolPanel::Curves => all_curves_ui(
                    ui,
                    &mut self.project.adjustments,
                    template_name,
                    channel_names,
                    histograms,
                    self.settings.colorize_adjustments,
                    self.settings.show_curve_histogram,
                    compact_curve_controls,
                    palette,
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
            let (body_changed, reset) = adjustment_foldout(
                ui,
                "all-levels-section",
                "Levels - all channels",
                true,
                |ui| {
                    broadcast_levels_ui(
                        ui,
                        &mut self.project.adjustments,
                        template_name,
                        channel_names,
                        accent,
                    )
                },
            );
            changed |= body_changed.unwrap_or(false);
            if reset {
                reset_all_levels(&mut self.project.adjustments, channel_names);
                changed = true;
            }

            ui.add_space(4.0);
            let (body_changed, reset) = adjustment_foldout(
                ui,
                "all-curves-section",
                "Curve - broadcast + per channel",
                true,
                |ui| {
                    all_curves_ui(
                        ui,
                        &mut self.project.adjustments,
                        template_name,
                        channel_names,
                        histograms,
                        self.settings.colorize_adjustments,
                        self.settings.show_curve_histogram,
                        compact_curve_controls,
                        palette,
                    )
                },
            );
            changed |= body_changed.unwrap_or(false);
            if reset {
                reset_all_curves(&mut self.project.adjustments, channel_names);
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
                    .show(&mut columns[0], |ui| {
                        self.ui_channels_histogram(ui);
                        ui.separator();
                        self.ui_history(ui);
                    });
                egui::ScrollArea::vertical()
                    .id_salt("adjustments-column")
                    .show(&mut columns[1], |ui| self.ui_adjustments(ui));
            });
        } else {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.ui_channels_histogram(ui);
                ui.separator();
                self.ui_history(ui);
                ui.separator();
                self.ui_adjustments(ui);
            });
        }
    }

    fn ui_viewport(&mut self, ui: &mut egui::Ui) {
        if workflow_v0103::ui_missing_viewport(self, ui) {
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
        let texture = face.texture.clone();
        let original_texture = face.original_texture.clone();
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
        });
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
                let show_before = ui.input(|input| input.pointer.secondary_down())
                    && ui.rect_contains_pointer(image_rect);
                let display_texture = if show_before {
                    original_texture.as_ref().unwrap_or(&texture)
                } else {
                    &texture
                };
                ui.put(
                    image_rect,
                    egui::Image::from_texture(display_texture).fit_to_exact_size(image_size),
                );
                if show_before {
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

    fn ui_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let dirty = if self.project_dirty {
                " * modified"
            } else {
                ""
            };
            ui.label(format!("{}{}", self.status_message, dirty));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Fit").clicked() {
                    self.fit_requested = true;
                }
                let zoom = ui.add(
                    egui::Slider::new(&mut self.zoom, 0.05..=8.0)
                        .logarithmic(true)
                        .text("Zoom"),
                );
                if zoom.changed() {
                    self.viewport_recenter = true;
                }
                if let Some(path) = &self.project_path {
                    ui.label(path.display().to_string());
                }
            });
        });
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
                changed |= ui
                    .checkbox(
                        &mut self.settings.validate_after_export,
                        "Validate TIFF after normal Export face / Export all",
                    )
                    .changed();
                ui.small("When enabled, Shade Editor immediately re-decodes every exported TIFF and verifies channel layout/names, ICC/Photoshop resources, compression/predictor policy and complete strip decoding.");
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
                        "Use tabs for Levels / Curve / Mixer",
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
                changed |= ui
                    .checkbox(
                        &mut self.settings.colorize_histograms,
                        "Colorize histograms by channel",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.colorize_adjustments,
                        "Colorize Levels / Curve / Mixer by channel",
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
            .resizable(false)
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
                ui.separator();
                ui.label("Update controls are located on the right side of the main toolbar.");
                ui.separator();
                ui.label("Shortcuts: Ctrl+S Save · Ctrl+Shift+S Save As · F Fit · 1-9 channel · S Solo · Ctrl+Enter Update Snapshot · Curve arrows nudge; Shift+Arrow uses larger steps.");
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
        self.poll_render(ui.ctx());
        self.sync_update_state();
        self.poll_autosave();
        workflow_v0103::handle_shortcuts(self, ui.ctx());
        self.handle_history_shortcuts(ui.ctx());
        self.maybe_autosave();
        self.handle_close_request(ui.ctx());
        if self.close_after_save && self.job.is_none() && !self.project_dirty {
            self.close_after_save = false;
            self.allow_close_once = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

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
        self.ui_about_window(ui.ctx());
        self.ui_logs_window(ui.ctx());
        self.ui_recovery_window(ui.ctx());
        self.ui_snapshot_discard_confirmation(ui.ctx());
        self.ui_close_confirmation(ui.ctx());
        self.commit_pending_history(ui.ctx(), false);

        self.start_render_if_needed(ui.ctx());
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

fn reset_all_levels(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
) {
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().levels = model::Levels::default();
    }
}

fn reset_all_curves(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
) {
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().curve = model::Curve::default();
    }
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

fn levels_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    accent: Option<egui::Color32>,
) -> bool {
    with_accent(ui, accent, |ui| {
        let levels = &mut adjustment.levels;
        let mut changed = false;
        changed |= ui
            .add(egui::Slider::new(&mut levels.input_black, 0.0..=0.98).text("Input black"))
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut levels.gamma, 0.1..=4.0)
                    .logarithmic(true)
                    .text("Gamma (relative)"),
            )
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut levels.input_white, 0.02..=1.0).text("Input white"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut levels.output_black, 0.0..=1.0).text("Output black"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut levels.output_white, 0.0..=1.0).text("Output white"))
            .changed();
        if levels.input_white <= levels.input_black {
            levels.input_white = (levels.input_black + 0.01).min(1.0);
            changed = true;
        }
        ui.small(format!(
            "Gamma midpoint output: {:.3}",
            model::levels_gamma_mid_output(*levels)
        ));
        changed
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CurvePointKind {
    Black,
    Midpoint,
    White,
}

impl CurvePointKind {
    fn label(self) -> &'static str {
        match self {
            Self::Black => "Black point",
            Self::Midpoint => "Midpoint",
            Self::White => "White point",
        }
    }
}

fn curve_point_xy(curve: model::Curve, point: CurvePointKind) -> (f32, f32) {
    match point {
        CurvePointKind::Black => (curve.input_black, curve.black),
        CurvePointKind::Midpoint => (curve.midpoint_input, curve.midpoint),
        CurvePointKind::White => (curve.input_white, curve.white),
    }
}

fn set_curve_point(curve: &mut model::Curve, point: CurvePointKind, input: f32, output: f32) {
    let gap = 1.0 / 255.0;
    let output = output.clamp(0.0, 1.0);
    match point {
        CurvePointKind::Black => {
            let max_input = if curve.midpoint_enabled {
                (curve.midpoint_input - gap).max(0.0)
            } else {
                (curve.input_white - gap).max(0.0)
            };
            curve.input_black = input.clamp(0.0, max_input);
            curve.black = output;
        }
        CurvePointKind::Midpoint => {
            curve.midpoint_input = input.clamp(
                (curve.input_black + gap).min(1.0),
                (curve.input_white - gap).max(0.0),
            );
            curve.midpoint = output;
        }
        CurvePointKind::White => {
            let min_input = if curve.midpoint_enabled {
                (curve.midpoint_input + gap).min(1.0)
            } else {
                (curve.input_black + gap).min(1.0)
            };
            curve.input_white = input.clamp(min_input, 1.0);
            curve.white = output;
        }
    }
}

fn curve_point_screen(rect: egui::Rect, input: f32, output: f32) -> egui::Pos2 {
    egui::pos2(
        egui::lerp(rect.x_range(), input.clamp(0.0, 1.0)),
        egui::lerp(rect.bottom()..=rect.top(), output.clamp(0.0, 1.0)),
    )
}

fn curve_editor_graph(
    ui: &mut egui::Ui,
    curve: &mut model::Curve,
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
) -> (bool, CurvePointKind) {
    let desired = egui::vec2(ui.available_width().min(340.0).max(150.0), 210.0);
    let (rect, graph_response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let graph_id = ui.make_persistent_id("three-point-curve-editor");
    let selection_id = graph_id.with("selected-point");
    let mut selected = ui
        .data(|data| data.get_temp::<CurvePointKind>(selection_id))
        .unwrap_or(CurvePointKind::Black);
    if !curve.midpoint_enabled && selected == CurvePointKind::Midpoint {
        selected = CurvePointKind::Black;
    }
    let mut changed = false;
    if graph_response.clicked() {
        graph_response.request_focus();
    }
    let mut midpoint_removed_this_frame = false;
    let points = [
        CurvePointKind::Black,
        CurvePointKind::Midpoint,
        CurvePointKind::White,
    ];

    for point in points {
        if point == CurvePointKind::Midpoint && !curve.midpoint_enabled {
            continue;
        }
        let (input, output) = curve_point_xy(*curve, point);
        let center = curve_point_screen(rect, input, output);
        let hit_rect = egui::Rect::from_center_size(center, egui::vec2(22.0, 22.0));
        let response = ui.interact(
            hit_rect,
            graph_id.with(point),
            egui::Sense::click_and_drag(),
        );
        if point == CurvePointKind::Midpoint && response.double_clicked() {
            curve.midpoint_enabled = false;
            midpoint_removed_this_frame = true;
            selected = CurvePointKind::Black;
            ui.data_mut(|data| data.insert_temp(selection_id, selected));
            changed = true;
            continue;
        }
        if response.clicked() || response.drag_started() {
            selected = point;
            ui.data_mut(|data| data.insert_temp(selection_id, point));
            graph_response.request_focus();
        }
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let input = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let output = ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0);
                set_curve_point(curve, point, input, output);
                selected = point;
                ui.data_mut(|data| data.insert_temp(selection_id, point));
                changed = true;
            }
        }
    }

    if !curve.midpoint_enabled && !midpoint_removed_this_frame && graph_response.double_clicked() {
        if let Some(pointer) = graph_response.interact_pointer_pos() {
            let input = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let gap = 1.0 / 255.0;
            if input > curve.input_black + gap && input < curve.input_white - gap {
                let output = model::curve_linear_output(input, *curve);
                let line_point = curve_point_screen(rect, input, output);
                if pointer.distance(line_point) <= 16.0 {
                    curve.midpoint_enabled = true;
                    curve.midpoint_input = input;
                    curve.midpoint = output;
                    selected = CurvePointKind::Midpoint;
                    ui.data_mut(|data| data.insert_temp(selection_id, selected));
                    changed = true;
                }
            }
        }
    }

    if graph_response.has_focus() {
        let (left, right, up, down, shift) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.modifiers.shift,
            )
        });
        if left || right || up || down {
            let step = if shift { 10.0 / 255.0 } else { 1.0 / 255.0 };
            let (mut input_value, mut output_value) = curve_point_xy(*curve, selected);
            if left {
                input_value -= step;
            }
            if right {
                input_value += step;
            }
            if up {
                output_value += step;
            }
            if down {
                output_value -= step;
            }
            set_curve_point(curve, selected, input_value, output_value);
            changed = true;
        }
    }

    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        2.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    if let Some(bins) = histogram {
        let max_value = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
        let hist_color = accent
            .unwrap_or(ui.visuals().weak_text_color())
            .gamma_multiply(0.30);
        for (index, value) in bins.iter().enumerate() {
            let x = egui::lerp(rect.x_range(), index as f32 / 255.0);
            let h = *value as f32 / max_value * rect.height();
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.bottom() - h),
                ],
                egui::Stroke::new(1.0, hist_color),
            );
        }
    }
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom()),
            egui::pos2(rect.right(), rect.top()),
        ],
        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
    );
    let curve_color = accent.unwrap_or(ui.visuals().selection.stroke.color);
    let mut last = None;
    for step in 0..=128 {
        let x = step as f32 / 128.0;
        let y = model::apply_curve(x, *curve);
        let point = curve_point_screen(rect, x, y);
        if let Some(previous) = last {
            painter.line_segment([previous, point], egui::Stroke::new(2.0, curve_color));
        }
        last = Some(point);
    }
    for point in points {
        if point == CurvePointKind::Midpoint && !curve.midpoint_enabled {
            continue;
        }
        let (input, output) = curve_point_xy(*curve, point);
        let center = curve_point_screen(rect, input, output);
        let is_selected = point == selected;
        let radius = if is_selected { 6.5 } else { 5.0 };
        let fill = if is_selected {
            curve_color
        } else {
            ui.visuals().extreme_bg_color
        };
        painter.circle_filled(center, radius, fill);
        painter.circle_stroke(center, radius, egui::Stroke::new(2.0, curve_color));
    }
    (changed, selected)
}

fn curve_point_fields(
    ui: &mut egui::Ui,
    curve: &mut model::Curve,
    selected: CurvePointKind,
) -> bool {
    let (input, output) = curve_point_xy(*curve, selected);
    let mut input_value = (input * 255.0).round() as i32;
    let mut output_value = (output * 255.0).round() as i32;
    ui.small(selected.label());
    let mut input_changed = false;
    let mut output_changed = false;
    ui.columns(2, |columns| {
        columns[0].label("Input");
        input_changed = columns[0]
            .add(
                egui::DragValue::new(&mut input_value)
                    .range(0..=255)
                    .speed(1),
            )
            .changed();
        columns[1].label("Output");
        output_changed = columns[1]
            .add(
                egui::DragValue::new(&mut output_value)
                    .range(0..=255)
                    .speed(1),
            )
            .changed();
    });
    if input_changed || output_changed {
        set_curve_point(
            curve,
            selected,
            input_value as f32 / 255.0,
            output_value as f32 / 255.0,
        );
        true
    } else {
        false
    }
}

fn curves_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    compact_controls: bool,
) -> bool {
    with_accent(ui, accent, |ui| {
        let (graph_changed, selected) =
            curve_editor_graph(ui, &mut adjustment.curve, histogram, accent);
        let mut changed = graph_changed;
        if !compact_controls {
            ui.add_space(6.0);
            changed |= curve_point_fields(ui, &mut adjustment.curve, selected);
            ui.add_space(4.0);
            ui.small("Double-click the Curve line to add the midpoint; double-click the midpoint to remove it. Drag active points directly. Input / Output use Photoshop-style 0-255 values.");
        }
        changed
    })
}

fn mixer_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    output_name: &str,
    channel_names: &[String],
    accent: Option<egui::Color32>,
    palette: Option<&ChannelPalette>,
) -> bool {
    with_accent(ui, accent, |ui| {
        let output_index = channel_names
            .iter()
            .position(|name| name == output_name)
            .unwrap_or(0);
        let output_display = channel_display_name(palette, output_name, output_index);
        if let Some(color) = accent {
            ui.colored_label(color, format!("Output: {output_display}"));
        } else {
            ui.label(format!("Output: {output_display}"));
        }
        let mut changed = false;
        for (index, name) in channel_names.iter().enumerate() {
            let default = if name == output_name { 1.0 } else { 0.0 };
            let coefficient = adjustment
                .mixer
                .coefficients
                .entry(name.clone())
                .or_insert(default);
            let row_accent = accent.map(|_| channel_color(palette, name, index));
            changed |= with_accent(ui, row_accent, |ui| {
                let mut slider = egui::Slider::new(coefficient, -2.0..=2.0)
                    .text(channel_display_name(palette, name, index))
                    .trailing_fill(true);
                if let Some(color) = row_accent {
                    slider = slider.text_color(color);
                }
                ui.add(slider).changed()
            });
        }
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        let mut constant_slider = egui::Slider::new(&mut adjustment.mixer.constant, -1.0..=1.0)
            .text("Constant")
            .trailing_fill(true);
        if let Some(color) = accent {
            constant_slider = constant_slider.text_color(color);
        }
        changed |= ui.add(constant_slider).changed();
        changed
    })
}
fn broadcast_levels_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    accent: Option<egui::Color32>,
) -> bool {
    let mut draft = adjustments.get(template_name).cloned().unwrap_or_default();
    if !levels_ui(ui, &mut draft, accent) {
        return false;
    }
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().levels = draft.levels;
    }
    true
}

fn broadcast_curves_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    compact_controls: bool,
) -> bool {
    let mut draft = adjustments.get(template_name).cloned().unwrap_or_default();
    if !curves_ui(ui, &mut draft, histogram, accent, compact_controls) {
        return false;
    }
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().curve = draft.curve;
    }
    true
}

fn all_curves_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    histograms: &[[u32; 256]],
    colorize: bool,
    show_histogram: bool,
    compact_controls: bool,
    palette: Option<&ChannelPalette>,
) -> bool {
    let mut changed = false;
    let template_index = channel_names
        .iter()
        .position(|name| name == template_name)
        .unwrap_or(0);
    let broadcast_accent = colorize.then(|| channel_color(palette, template_name, template_index));
    let broadcast_histogram = show_histogram
        .then(|| histograms.get(template_index))
        .flatten();

    egui::Frame::new()
        .inner_margin(6)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(5)
        .show(ui, |ui| {
            ui.strong("Broadcast to all");
            ui.small("Changes here are copied to every channel Curve.");
            changed |= broadcast_curves_ui(
                ui,
                adjustments,
                template_name,
                channel_names,
                broadcast_histogram,
                broadcast_accent,
                compact_controls,
            );
        });

    ui.add_space(7.0);
    ui.strong("Per-channel Curves");
    ui.small("Open any channel to refine it after using Broadcast.");
    for (index, name) in channel_names.iter().enumerate() {
        let accent = colorize.then(|| channel_color(palette, name, index));
        let title = if let Some(color) = accent {
            egui::RichText::new(format!("●  {}", channel_display_name(palette, name, index)))
                .color(color)
        } else {
            egui::RichText::new(channel_display_name(palette, name, index))
        };
        egui::CollapsingHeader::new(title)
            .id_salt(format!("all-channel-curve-{index}-{name}"))
            .default_open(false)
            .show(ui, |ui| {
                let histogram = if show_histogram {
                    histograms.get(index)
                } else {
                    None
                };
                let adjustment = adjustments.entry(name.clone()).or_default();
                changed |= curves_ui(ui, adjustment, histogram, accent, compact_controls);
            });
    }
    changed
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
            changed |= mixer_ui(ui, adjustment, output_name, channel_names, accent, palette);
        });
    }
    changed
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
    let width = ui.available_width().max(1.0);
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

fn clickable_channel_row(
    ui: &mut egui::Ui,
    selected: bool,
    solo: bool,
    label: &str,
    accent: egui::Color32,
    height: f32,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
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
    response
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
        let x = egui::lerp(rect.x_range(), index as f32 / 255.0);
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

fn face_identity_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn duplicate_face_counts(faces: &[RuntimeFace]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for face in faces {
        *counts.entry(face_identity_key(&face.path)).or_insert(0) += 1;
    }
    counts
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
