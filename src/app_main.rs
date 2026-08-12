#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_log;
mod dpi;
#[path = "export_v6.rs"]
mod export;
#[path = "model_v6.rs"]
mod model;
mod palette;
mod render;
#[path = "settings_v6.rs"]
mod settings;
#[path = "tiff_io.rs"]
mod tiff_io;
#[path = "update_v4.rs"]
mod update;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use eframe::egui;
use model::{ChannelAdjustment, ShadeProject, TestCodePosition};
use palette::ChannelPalette;
use settings::AppSettings;
use tiff_io::PreviewFace;
use update::{UpdateManager, UpdateStatus};

const VIEWPORT_OVERSCROLL: f32 = 180.0;
const ERROR_TOAST_LIFETIME: Duration = Duration::from_secs(120);

fn main() -> eframe::Result {
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
        Box::new(|cc| Ok(Box::new(ShadeApp::new(cc)))),
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
    preview: Arc<PreviewFace>,
    dpi: dpi::DpiInfo,
    adjusted: Vec<Vec<u16>>,
    texture: Option<egui::TextureHandle>,
    generation: u64,
    rendered_generation: u64,
}

struct LoadedFace {
    path: PathBuf,
    preview: PreviewFace,
    dpi: dpi::DpiInfo,
}

struct OpenPayload {
    path: PathBuf,
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
    Open(Result<OpenPayload, String>),
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
        let mut project = ShadeProject::default();
        project.channel_palette = settings.default_project_palette();
        let log = app_log::AppLog::default();
        log.info(&format!(
            "Shade Editor {} started",
            env!("CARGO_PKG_VERSION")
        ));
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
        self.report_info("New shade project");
    }

    fn make_runtime_face(item: LoadedFace) -> RuntimeFace {
        RuntimeFace {
            path: item.path,
            preview: Arc::new(item.preview),
            dpi: item.dpi,
            adjusted: Vec::new(),
            texture: None,
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
                        preview,
                    }),
                    Err(err) => errors.push(format!("{}: {err}", path.display())),
                }
            }
            Self::set_progress(&progress, Some(1.0), "Opening TIFF", "Complete");
            JobResult::AddFaces { faces, errors }
        });
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
                    match tiff_io::load_preview(&source, max_dimension) {
                        Ok(preview) => {
                            project.ensure_channels(&preview.metadata.channel_names);
                            faces.push(LoadedFace {
                                dpi: dpi::read_dpi(&source, default_dpi),
                                path: source,
                                preview,
                            });
                        }
                        Err(err) => errors.push(format!("{}: {err}", source.display())),
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

    fn save_project(&mut self, save_as: bool) {
        if self.job.is_some() || self.faces.is_empty() {
            return;
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
            return;
        };
        let project = self.project.clone();
        let face_paths = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        let result_path = path.clone();
        self.launch_job("Saving project", move |progress| {
            Self::set_progress(
                &progress,
                Some(0.25),
                "Saving project",
                "Serializing settings",
            );
            let result = project.save(&path, &face_paths);
            Self::set_progress(&progress, Some(1.0), "Saving project", "Complete");
            JobResult::Save {
                path: result_path,
                result,
            }
        });
    }

    fn export_current_dialog(&mut self) {
        if self.job.is_some() {
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
        self.launch_job("Exporting TIFF", move |progress| {
            let result = export::export_face_with_progress(
                &source,
                &destination,
                &project,
                default_dpi,
                |fraction, detail| {
                    Self::set_progress(&progress, Some(fraction), "Exporting TIFF", detail);
                },
            )
            .map(|_| format!("Exported {}", destination.display()));
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
                            let overall = (index as f32 + inner) / total as f32;
                            Self::set_progress(&progress, Some(overall), "Exporting faces", detail);
                        },
                    )?;
                }
                Ok(format!("Exported {total} face(s) to {}", folder.display()))
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
                    self.report_info(format!("Added {added} face(s)"));
                }
                if !errors.is_empty() {
                    self.report_error(format!(
                        "Some TIFF files could not be loaded: {}",
                        errors.join(" | ")
                    ));
                }
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
            JobResult::Save { path, result } => match result {
                Ok(()) => {
                    self.project_path = Some(path.clone());
                    self.project_dirty = false;
                    self.report_info(format!("Saved {}", path.display()));
                }
                Err(err) => self.report_error(err),
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
            let _ = tx.send(RenderResult {
                face_index,
                generation,
                adjusted,
                rgba,
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
                    .small_button(format!("Restart → {}", info.version))
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

    fn ui_faces(&mut self, ui: &mut egui::Ui) {
        ui.heading("Faces");
        if self.faces.is_empty() {
            ui.label("Add TIFF files to create a shade project.");
        } else {
            let mut requested_face = None;
            for (index, face) in self.faces.iter().enumerate() {
                let label = self
                    .project
                    .faces
                    .get(index)
                    .map(|item| item.label.as_str())
                    .unwrap_or_else(|| {
                        face.path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("Face")
                    });
                if clickable_row(ui, self.current_face == index, label, None, None, 32.0).clicked()
                {
                    requested_face = Some(index);
                }
            }
            if let Some(index) = requested_face {
                self.current_face = index;
                self.selected_channel = 0;
                self.solo_channel = None;
                self.fit_requested = true;
                self.viewport_recenter = true;
                self.mark_current_preview_dirty();
            }
            ui.add_space(4.0);
            if ui.button("Remove active face").clicked() {
                self.remove_current_face();
            }
        }

        ui.separator();
        self.ui_snapshots(ui);
        ui.separator();
        self.ui_test_code(ui);
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
                    .small_button("✓")
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
                        .small_button("✓")
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
                self.report_info("Snapshot loaded");
            }
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
        if update && self.project.update_snapshot(active_id) {
            self.project_dirty = true;
            self.report_info("Snapshot updated");
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
                        .hint_text(format!("Empty → {fallback}")),
                )
                .changed();
            if !channel_names.is_empty() {
                let selected_index = channel_names
                    .iter()
                    .position(|name| name == &self.project.test_code.channel)
                    .unwrap_or(0);
                let selected_display = channel_display_name(
                    palette.as_ref(),
                    &channel_names[selected_index],
                    selected_index,
                );
                egui::ComboBox::from_label("Ink / channel")
                    .selected_text(selected_display)
                    .show_ui(ui, |ui| {
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
        let channel_names = face.preview.metadata.channel_names.clone();
        let original_histograms = face.preview.histograms.clone();
        let adjusted_histograms = face
            .adjusted
            .iter()
            .map(|values| render::histogram(values))
            .collect::<Vec<_>>();
        let base_count = face.preview.metadata.base_channel_count;
        let color_model = face.preview.metadata.color_model;
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
            let suffix = if index >= base_count {
                "  • spot"
            } else {
                ""
            };
            let accent = channel_color(active_palette.as_ref(), name, index);
            let is_solo = self.solo_channel == Some(index);
            let indicator = if is_solo { "■" } else { "□" };
            let display_name = channel_display_name(active_palette.as_ref(), name, index);
            let label = format!("{indicator}  {display_name}{suffix}");
            let response = clickable_row(
                ui,
                self.selected_channel == index,
                &label,
                None,
                Some(accent),
                32.0,
            )
            .on_hover_text("Click to select for editing. Click the active channel again to toggle solo preview.");
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
                let accent = self
                    .settings
                    .colorize_histograms
                    .then(|| channel_color(active_palette.as_ref(), name, index));
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
            let accent = self
                .settings
                .colorize_histograms
                .then(|| channel_color(active_palette.as_ref(), &channel_names[index], index));
            let display =
                channel_display_name(active_palette.as_ref(), &channel_names[index], index);
            ui.strong(format!("Histogram — {display}"));
            draw_histogram(
                ui,
                original_histograms.get(index),
                adjusted_histograms.get(index),
                accent,
            );
        }
    }
    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {
        let Some(face) = self.faces.get(self.current_face) else {
            ui.heading("Adjustments");
            ui.label("No active face");
            return;
        };
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
                    ui.colored_label(color, format!("Editing: {output_display}"));
                }
                if ui.button("Reset all adjustments").clicked() {
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
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                    ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                    ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
                });
                changed |= match self.tool {
                    ToolPanel::Levels => levels_ui(ui, adjustment, accent),
                    ToolPanel::Curves => curves_ui(
                        ui,
                        adjustment,
                        histogram.filter(|_| self.settings.show_curve_histogram),
                        accent,
                    ),
                    ToolPanel::Mixer => {
                        mixer_ui(ui, adjustment, output_name, channel_names, accent, palette)
                    }
                };
            } else {
                egui::CollapsingHeader::new("Levels")
                    .id_salt(format!("selected-levels-{output_name}"))
                    .default_open(true)
                    .show(ui, |ui| {
                        changed |= levels_ui(ui, adjustment, accent);
                    });
                ui.add_space(4.0);
                egui::CollapsingHeader::new("Curve")
                    .id_salt(format!("selected-curve-{output_name}"))
                    .default_open(true)
                    .show(ui, |ui| {
                        changed |= curves_ui(
                            ui,
                            adjustment,
                            histogram.filter(|_| self.settings.show_curve_histogram),
                            accent,
                        );
                    });
                ui.add_space(4.0);
                egui::CollapsingHeader::new("Channel Mixer")
                    .id_salt(format!("selected-mixer-{output_name}"))
                    .default_open(true)
                    .show(ui, |ui| {
                        changed |=
                            mixer_ui(ui, adjustment, output_name, channel_names, accent, palette);
                    });
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
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
            });
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
            egui::CollapsingHeader::new("Levels — all channels")
                .id_salt("all-levels-section")
                .default_open(true)
                .show(ui, |ui| {
                    changed |= broadcast_levels_ui(
                        ui,
                        &mut self.project.adjustments,
                        template_name,
                        channel_names,
                        accent,
                    );
                });
            ui.add_space(4.0);
            egui::CollapsingHeader::new("Curve — broadcast + per channel")
                .id_salt("all-curves-section")
                .default_open(true)
                .show(ui, |ui| {
                    changed |= all_curves_ui(
                        ui,
                        &mut self.project.adjustments,
                        template_name,
                        channel_names,
                        histograms,
                        self.settings.colorize_adjustments,
                        self.settings.show_curve_histogram,
                        palette,
                    );
                });
            ui.add_space(4.0);
            egui::CollapsingHeader::new("Channel Mixer — all output rows")
                .id_salt("all-mixers-section")
                .default_open(true)
                .show(ui, |ui| {
                    changed |= all_mixers_ui(
                        ui,
                        &mut self.project.adjustments,
                        channel_names,
                        self.settings.colorize_adjustments,
                        palette,
                    );
                });
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

        let title = self
            .project
            .faces
            .get(self.current_face)
            .map(|item| item.label.clone())
            .unwrap_or_else(|| face.path.display().to_string());
        let meta = face.preview.metadata.clone();
        let dpi_info = face.dpi;
        let texture = face.texture.clone();
        ui.horizontal_wrapped(|ui| {
            ui.strong(title);
            ui.separator();
            ui.label(format!("{} × {} px", meta.width, meta.height));
            ui.label(format!("{}-bit", meta.bit_depth));
            ui.label(meta.color_model.title());
            ui.label(format!("{} channels", meta.samples_per_pixel));
            if dpi_info.has_physical_resolution {
                ui.label(format!("{:.0} × {:.0} DPI", dpi_info.dpi_x, dpi_info.dpi_y));
            } else {
                ui.label(format!(
                    "{:.0} × {:.0} DPI (default)",
                    dpi_info.dpi_x, dpi_info.dpi_y
                ));
            }
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
                ui.put(
                    image_rect,
                    egui::Image::from_texture(&texture).fit_to_exact_size(image_size),
                );
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
                " • modified"
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
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.max_preview_dimension, 600..=4000)
                            .text("Preview max dimension"),
                    )
                    .changed();
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
                    "Automatic — CMYK/RGB from first Face".to_owned()
                } else {
                    palette_library
                        .iter()
                        .find(|palette| palette.id == self.settings.default_palette_id)
                        .map(|palette| palette.name.clone())
                        .unwrap_or_else(|| "Automatic — CMYK/RGB from first Face".to_owned())
                };
                egui::ComboBox::from_label("Default palette for new projects")
                    .selected_text(default_palette_name)
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut self.settings.default_palette_id,
                                palette::AUTO_PALETTE_ID.to_owned(),
                                "Automatic — CMYK/RGB from first Face",
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
                    egui::CollapsingHeader::new(format!("Custom — {}", custom.name))
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
                                    if ui.small_button("−").on_hover_text("Remove channel slot").clicked() {
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
                ui.label("Copyright © 2026 Emad Ghasemi");
                ui.label("MIT License");
                ui.hyperlink_to(
                    "GitHub repository",
                    "https://github.com/emadgh/windows-shade-editor",
                );
                ui.separator();
                ui.label("Update controls are located on the right side of the main toolbar.");
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

        self.start_render_if_needed(ui.ctx());
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
        if ui.small_button("Reset Levels").clicked() {
            adjustment.levels = model::Levels::default();
            changed = true;
        }
        changed
    })
}

fn curves_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
) -> bool {
    with_accent(ui, accent, |ui| {
        draw_curve(ui, adjustment.curve, histogram, accent);
        let mut changed = false;
        changed |= ui
            .add(
                egui::Slider::new(&mut adjustment.curve.input_black, 0.0..=0.98)
                    .text("Input black"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut adjustment.curve.input_white, 0.02..=1.0)
                    .text("Input white"),
            )
            .changed();
        if adjustment.curve.input_white <= adjustment.curve.input_black {
            adjustment.curve.input_white = (adjustment.curve.input_black + 0.01).min(1.0);
            changed = true;
        }
        ui.add_space(3.0);
        changed |= ui
            .add(egui::Slider::new(&mut adjustment.curve.black, 0.0..=1.0).text("Black output"))
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut adjustment.curve.midpoint, 0.0..=1.0)
                    .text("Midpoint (relative)"),
            )
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut adjustment.curve.white, 0.0..=1.0).text("White output"))
            .changed();
        ui.small(format!(
            "Calculated midpoint output: {:.3}",
            model::curve_mid_output(adjustment.curve)
        ));
        if ui.small_button("Reset Curve").clicked() {
            adjustment.curve = model::Curve::default();
            changed = true;
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
        if ui.small_button("Reset Mixer").clicked() {
            adjustment.mixer.coefficients.clear();
            for name in channel_names {
                adjustment
                    .mixer
                    .coefficients
                    .insert(name.clone(), if name == output_name { 1.0 } else { 0.0 });
            }
            adjustment.mixer.constant = 0.0;
            changed = true;
        }
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
) -> bool {
    let mut draft = adjustments.get(template_name).cloned().unwrap_or_default();
    if !curves_ui(ui, &mut draft, histogram, accent) {
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
                changed |= curves_ui(ui, adjustment, histogram, accent);
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
        ui.collapsing(format!("Output — {display}"), |ui| {
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

fn palette_entry_readonly(ui: &mut egui::Ui, entry: &palette::ChannelPaletteEntry) {
    let [r, g, b] = entry.color;
    ui.horizontal(|ui| {
        ui.colored_label(egui::Color32::from_rgb(r, g, b), "■");
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

fn snapshot_row_with_actions(
    ui: &mut egui::Ui,
    selected: bool,
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
    let fill = if selected {
        visuals.selection.bg_fill.gamma_multiply(0.72)
    } else if row_response.hovered() {
        visuals.widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
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
        egui::FontId::proportional(14.0),
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
        return ("Earlier snapshots".to_owned(), "—".to_owned());
    }
    match Local.timestamp_millis_opt(created_at_unix_ms).single() {
        Some(value) => (
            value.format("%Y-%m-%d").to_string(),
            value.format("%H:%M").to_string(),
        ),
        None => ("Earlier snapshots".to_owned(), "—".to_owned()),
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

fn draw_curve(
    ui: &mut egui::Ui,
    curve: model::Curve,
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
) {
    let desired = egui::vec2(ui.available_width().min(320.0).max(120.0), 170.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
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
            .gamma_multiply(0.35);
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
    for step in 0..=96 {
        let x = step as f32 / 96.0;
        let y = model::apply_curve(x, curve);
        let point = egui::pos2(
            egui::lerp(rect.x_range(), x),
            egui::lerp(rect.bottom()..=rect.top(), y),
        );
        if let Some(previous) = last {
            painter.line_segment([previous, point], egui::Stroke::new(2.0, curve_color));
        }
        last = Some(point);
    }
}

fn apply_theme(ctx: &egui::Context, dark: bool) {
    if dark {
        ctx.set_visuals(egui::Visuals::dark());
    } else {
        ctx.set_visuals(egui::Visuals::light());
    }
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
