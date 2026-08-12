#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "export_v4.rs"]
mod export;
#[path = "model_v4.rs"]
mod model;
#[path = "settings_v4.rs"]
mod settings;
#[path = "update_v4.rs"]
mod update;
#[path = "tiff_io.rs"]
mod tiff_io;
mod render;
mod dpi;
mod app_log;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use model::{ChannelAdjustment, ShadeProject, TestCodePosition};
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
    AddFaces { faces: Vec<LoadedFace>, errors: Vec<String> },
    Open(Result<OpenPayload, String>),
    Save { path: PathBuf, result: Result<(), String> },
    Export(Result<String, String>),
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
        let log = app_log::AppLog::default();
        log.info(&format!("Shade Editor {} started", env!("CARGO_PKG_VERSION")));
        Self {
            project: ShadeProject::default(),
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
        self.toast = Some(ErrorToast { message, created: Instant::now() });
    }

    fn report_info(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log.info(&message);
        self.status_message = message;
    }

    fn new_project(&mut self) {
        if self.job.is_some() { return; }
        self.project = ShadeProject::default();
        self.project_path = None;
        self.faces.clear();
        self.current_face = 0;
        self.selected_channel = 0;
        self.solo_channel = None;
        self.adjustment_scope = AdjustmentScope::Selected;
        self.viewport_recenter = true;
        self.fit_requested = true;
        self.project_dirty = false;
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

    fn set_progress(progress: &Arc<Mutex<JobProgress>>, fraction: Option<f32>, label: &str, detail: &str) {
        if let Ok(mut state) = progress.lock() {
            state.fraction = fraction.map(|value| value.clamp(0.0, 1.0));
            state.label = label.to_owned();
            state.detail = detail.to_owned();
        }
    }

    fn add_faces_dialog(&mut self) {
        if self.job.is_some() { return; }
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("TIFF images", &["tif", "tiff"])
            .pick_files()
        else { return; };
        if paths.is_empty() { return; }
        let max_dimension = self.settings.max_preview_dimension;
        self.launch_job("Opening TIFF", move |progress| {
            let total = paths.len().max(1);
            let mut faces = Vec::new();
            let mut errors = Vec::new();
            for (index, path) in paths.into_iter().enumerate() {
                Self::set_progress(
                    &progress,
                    Some(index as f32 / total as f32),
                    "Opening TIFF",
                    &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                );
                match tiff_io::load_preview(&path, max_dimension) {
                    Ok(preview) => faces.push(LoadedFace { dpi: dpi::read_dpi(&path), path, preview }),
                    Err(err) => errors.push(format!("{}: {err}", path.display())),
                }
            }
            Self::set_progress(&progress, Some(1.0), "Opening TIFF", "Complete");
            JobResult::AddFaces { faces, errors }
        });
    }

    fn open_project_dialog(&mut self) {
        if self.job.is_some() { return; }
        let Some(path) = rfd::FileDialog::new().add_filter("Shade project", &["shade"]).pick_file() else { return; };
        let max_dimension = self.settings.max_preview_dimension;
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
                        &source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    );
                    match tiff_io::load_preview(&source, max_dimension) {
                        Ok(preview) => {
                            project.ensure_channels(&preview.metadata.channel_names);
                            faces.push(LoadedFace { dpi: dpi::read_dpi(&source), path: source, preview });
                        }
                        Err(err) => errors.push(format!("{}: {err}", source.display())),
                    }
                }
                Self::set_progress(&progress, Some(1.0), "Opening project", "Complete");
                Ok(OpenPayload { path, project, faces, errors })
            })();
            JobResult::Open(result)
        });
    }

    fn save_project(&mut self, save_as: bool) {
        if self.job.is_some() || self.faces.is_empty() { return; }
        let target = if !save_as { self.project_path.clone() } else { None };
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
        let Some(path) = target else { return; };
        let project = self.project.clone();
        let face_paths = self.faces.iter().map(|face| face.path.clone()).collect::<Vec<_>>();
        let result_path = path.clone();
        self.launch_job("Saving project", move |progress| {
            Self::set_progress(&progress, Some(0.25), "Saving project", "Serializing settings");
            let result = project.save(&path, &face_paths);
            Self::set_progress(&progress, Some(1.0), "Saving project", "Complete");
            JobResult::Save { path: result_path, result }
        });
    }

    fn export_current_dialog(&mut self) {
        if self.job.is_some() { return; }
        let Some(face) = self.faces.get(self.current_face) else { return; };
        let stem = face.path.file_stem().map(|value| value.to_string_lossy()).unwrap_or_default();
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("TIFF image", &["tif", "tiff"])
            .set_file_name(format!("{stem}-shade.tif"))
            .save_file()
        else { return; };
        let source = face.path.clone();
        let project = self.project.clone();
        self.launch_job("Exporting TIFF", move |progress| {
            let result = export::export_face_with_progress(&source, &destination, &project, |fraction, detail| {
                Self::set_progress(&progress, Some(fraction), "Exporting TIFF", detail);
            })
            .map(|_| format!("Exported {}", destination.display()));
            JobResult::Export(result)
        });
    }

    fn export_all_dialog(&mut self) {
        if self.job.is_some() || self.faces.is_empty() { return; }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else { return; };
        let sources = self.faces.iter().map(|face| face.path.clone()).collect::<Vec<_>>();
        let project = self.project.clone();
        self.launch_job("Exporting faces", move |progress| {
            let total = sources.len().max(1);
            let result = (|| -> Result<String, String> {
                for (index, source) in sources.iter().enumerate() {
                    let stem = source.file_stem().map(|value| value.to_string_lossy()).unwrap_or_default();
                    let destination = folder.join(format!("{stem}-shade.tif"));
                    export::export_face_with_progress(source, &destination, &project, |inner, detail| {
                        let overall = (index as f32 + inner) / total as f32;
                        Self::set_progress(&progress, Some(overall), "Exporting faces", detail);
                    })?;
                }
                Ok(format!("Exported {total} face(s) to {}", folder.display()))
            })();
            JobResult::Export(result)
        });
    }

    fn poll_job(&mut self) {
        let result = self.job.as_ref().and_then(|job| job.rx.try_recv().ok());
        let Some(result) = result else { return; };
        self.job = None;
        match result {
            JobResult::AddFaces { faces, errors } => {
                let added = faces.len();
                for item in faces {
                    self.project.ensure_channels(&item.preview.metadata.channel_names);
                    let label = item.path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Face".to_owned());
                    self.project.faces.push(model::FaceRef { path: item.path.to_string_lossy().into_owned(), label });
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
                    self.report_error(format!("Some TIFF files could not be loaded: {}", errors.join(" | ")));
                }
            }
            JobResult::Open(result) => match result {
                Ok(payload) => {
                    self.project = payload.project;
                    self.project_path = Some(payload.path.clone());
                    self.faces = payload.faces.into_iter().map(Self::make_runtime_face).collect();
                    self.current_face = 0;
                    self.selected_channel = 0;
                    self.solo_channel = None;
                    self.adjustment_scope = AdjustmentScope::Selected;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.project_dirty = false;
                    self.report_info(format!("Opened {}", payload.path.display()));
                    if !payload.errors.is_empty() {
                        self.report_error(format!("Project opened with TIFF errors: {}", payload.errors.join(" | ")));
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
            JobResult::Export(result) => match result {
                Ok(message) => self.report_info(message),
                Err(err) => self.report_error(format!("Export failed: {err}")),
            },
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
            let Some(face) = self.faces.get_mut(result.face_index) else { continue; };
            if face.generation != result.generation { continue; }
            face.adjusted = result.adjusted;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.rgba,
            );
            let options = egui::TextureOptions::LINEAR;
            if let Some(texture) = &mut face.texture {
                texture.set(image, options);
            } else {
                face.texture = Some(ctx.load_texture(format!("face-preview-{}", result.face_index), image, options));
            }
            face.rendered_generation = result.generation;
        }
    }

    fn start_render_if_needed(&mut self, ctx: &egui::Context) {
        if self.render_busy.is_some() || ctx.input(|input| input.pointer.any_down()) { return; }
        let Some(face) = self.faces.get(self.current_face) else { return; };
        if face.rendered_generation == face.generation { return; }
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
            let _ = tx.send(RenderResult { face_index, generation, adjusted, rgba });
        });
    }

    fn select_channel(&mut self, channel: usize, isolate: bool) {
        self.selected_channel = channel;
        let next_solo = if isolate { Some(channel) } else { None };
        if self.solo_channel != next_solo {
            self.solo_channel = next_solo;
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
        if self.job.is_some() || self.current_face >= self.faces.len() { return; }
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
        if let Err(err) = self.settings.save() { self.report_error(err); }
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
        if self.toast.as_ref().is_some_and(|toast| toast.created.elapsed() > ERROR_TOAST_LIFETIME) {
            self.toast = None;
        }
    }

    fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.horizontal_wrapped(|ui| {
                let enabled = self.job.is_none();
                if ui.add_enabled(enabled, egui::Button::new("New")).clicked() { self.new_project(); }
                if ui.add_enabled(enabled, egui::Button::new("Open .shade")).clicked() { self.open_project_dialog(); }
                if ui.add_enabled(enabled, egui::Button::new("Add TIFF faces")).clicked() { self.add_faces_dialog(); }
                ui.separator();
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save")).clicked() { self.save_project(false); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save As")).clicked() { self.save_project(true); }
                ui.separator();
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Export face")).clicked() { self.export_current_dialog(); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Export all")).clicked() { self.export_all_dialog(); }
                ui.separator();
                if ui.button("Settings").clicked() { self.show_settings = true; }
                if ui.button("About").clicked() { self.show_about = true; }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Logs").clicked() {
                    self.log_cache = self.log.read();
                    self.show_logs = true;
                }
                self.ui_update_compact(ui);
                self.ui_operation_progress(ui);
                if let Some(toast) = &self.toast {
                    ui.label(egui::RichText::new(&toast.message).color(egui::Color32::LIGHT_RED).small());
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
                ui.add(egui::ProgressBar::new(value).desired_width(175.0).text(text).animate(progress.fraction.is_none()));
                return;
            }
        }
        if self.render_busy.is_some() {
            ui.add(egui::ProgressBar::new(0.45).desired_width(145.0).text("Rendering preview").animate(true));
        }
    }

    fn ui_update_compact(&mut self, ui: &mut egui::Ui) {
        match self.updater.status() {
            UpdateStatus::Idle => {
                if ui.small_button("Check update").clicked() { self.updater.start_check(false); }
            }
            UpdateStatus::Checking => {
                ui.add(egui::ProgressBar::new(0.5).desired_width(125.0).text("Checking update").animate(true));
            }
            UpdateStatus::UpToDate => {
                if ui.small_button("Update ✓").on_hover_text("Check again").clicked() { self.updater.start_check(false); }
            }
            UpdateStatus::Available(info) => {
                if ui.small_button(format!("Download {}", info.version)).on_hover_text(info.release_url).clicked() {
                    self.updater.start_download();
                }
            }
            UpdateStatus::Downloading { info, downloaded, total } => {
                let fraction = total.filter(|total| *total > 0)
                    .map(|total| downloaded as f32 / total as f32)
                    .unwrap_or(0.5);
                ui.add(egui::ProgressBar::new(fraction).desired_width(150.0).text(format!("Updating {}", info.version)).animate(total.is_none()));
            }
            UpdateStatus::Ready(info, _) => {
                if ui.small_button(format!("Restart → {}", info.version)).clicked() {
                    match self.updater.apply_ready() {
                        Ok(true) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                        Ok(false) => {}
                        Err(err) => self.report_error(err),
                    }
                }
            }
            UpdateStatus::Failed(_) => {
                if ui.small_button("Retry update").clicked() { self.updater.start_check(false); }
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
                let label = self.project.faces.get(index)
                    .map(|item| item.label.as_str())
                    .unwrap_or_else(|| face.path.file_name().and_then(|name| name.to_str()).unwrap_or("Face"));
                if ui.selectable_label(self.current_face == index, label).clicked() { requested_face = Some(index); }
            }
            if let Some(index) = requested_face {
                self.current_face = index;
                self.selected_channel = 0;
                self.solo_channel = None;
                self.fit_requested = true;
                self.viewport_recenter = true;
                self.mark_current_preview_dirty();
            }
            if ui.button("Remove active face").clicked() { self.remove_current_face(); }
        }

        ui.separator();
        self.ui_snapshots(ui);
        ui.separator();
        self.ui_test_code(ui);
    }

    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Snapshots");
            if ui.small_button("+ New").clicked() {
                self.project.create_snapshot();
                self.project_dirty = true;
            }
        });
        ui.small("Saved adjustment test states.");
        let active_id = self.project.active_snapshot_id;
        let mut requested_load = None;
        for snapshot in &self.project.snapshots {
            let selected = active_id == Some(snapshot.id);
            let label = if selected && !self.project.active_snapshot_matches() {
                format!("{}  *", snapshot.name)
            } else {
                snapshot.name.clone()
            };
            if ui.selectable_label(selected, label).clicked() { requested_load = Some(snapshot.id); }
        }
        if let Some(id) = requested_load {
            if self.project.apply_snapshot(id) {
                self.mark_all_previews_dirty();
                self.report_info("Snapshot loaded");
            }
        }
        let Some(active_id) = self.project.active_snapshot_id else { return; };
        if let Some(snapshot) = self.project.snapshots.iter_mut().find(|snapshot| snapshot.id == active_id) {
            ui.label("Snapshot name");
            if ui.text_edit_singleline(&mut snapshot.name).changed() { self.project_dirty = true; }
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
            self.project_dirty = true;
            self.report_info("Snapshot deleted");
        }
    }

    fn ui_test_code(&mut self, ui: &mut egui::Ui) {
        ui.heading("Test code");
        let channel_names = self.faces.get(self.current_face)
            .map(|face| face.preview.metadata.channel_names.clone())
            .unwrap_or_default();
        let fallback = self.project.active_snapshot_name().unwrap_or("Test").to_owned();
        let mut changed = ui.checkbox(&mut self.project.test_code.enabled, "Write code on export").changed();
        ui.add_enabled_ui(self.project.test_code.enabled, |ui| {
            changed |= ui.add(
                egui::TextEdit::singleline(&mut self.project.test_code.text)
                    .hint_text(format!("Empty → {fallback}"))
            ).changed();
            if !channel_names.is_empty() {
                egui::ComboBox::from_label("Ink / channel")
                    .selected_text(&self.project.test_code.channel)
                    .show_ui(ui, |ui| {
                        for name in &channel_names {
                            changed |= ui.selectable_value(&mut self.project.test_code.channel, name.clone(), name).changed();
                        }
                    });
            }
            ui.horizontal(|ui| {
                ui.label("Font");
                ui.strong("Tahoma");
            });
            changed |= ui.add(egui::Slider::new(&mut self.project.test_code.font_size_pt, 6.0..=72.0).text("Size (pt)")).changed();
            changed |= ui.add(egui::Slider::new(&mut self.project.test_code.margin_cm, 0.0..=5.0).text("Edge margin (cm)")).changed();
            egui::ComboBox::from_label("Position")
                .selected_text(match self.project.test_code.position {
                    TestCodePosition::TopLeft => "Top left",
                    TestCodePosition::TopRight => "Top right",
                    TestCodePosition::BottomLeft => "Bottom left",
                    TestCodePosition::BottomRight => "Bottom right",
                })
                .show_ui(ui, |ui| {
                    changed |= ui.selectable_value(&mut self.project.test_code.position, TestCodePosition::TopLeft, "Top left").changed();
                    changed |= ui.selectable_value(&mut self.project.test_code.position, TestCodePosition::TopRight, "Top right").changed();
                    changed |= ui.selectable_value(&mut self.project.test_code.position, TestCodePosition::BottomLeft, "Bottom left").changed();
                    changed |= ui.selectable_value(&mut self.project.test_code.position, TestCodePosition::BottomRight, "Bottom right").changed();
                });
            ui.small("Default: top-left, 1 cm margin. Point size is converted using the TIFF DPI.");
        });
        if changed { self.project_dirty = true; }
    }

    fn ui_channels_histogram(&mut self, ui: &mut egui::Ui) {
        let Some(face) = self.faces.get(self.current_face) else {
            ui.heading("Channels");
            ui.label("No active face");
            return;
        };
        let channel_names = face.preview.metadata.channel_names.clone();
        let original_histograms = face.preview.histograms.clone();
        let adjusted_histograms = face.adjusted.iter().map(|values| render::histogram(values)).collect::<Vec<_>>();
        let base_count = face.preview.metadata.base_channel_count;
        let color_model = face.preview.metadata.color_model;
        if channel_names.is_empty() { return; }
        self.selected_channel = self.selected_channel.min(channel_names.len() - 1);

        ui.heading("Channels");
        ui.horizontal(|ui| {
            if ui.selectable_label(self.solo_channel.is_none(), "Composite").clicked() { self.show_composite(); }
            ui.label(format!("{} + {} extra", color_model.title(), channel_names.len().saturating_sub(base_count)));
        });
        for (index, name) in channel_names.iter().enumerate() {
            let suffix = if index >= base_count { "  • spot" } else { "" };
            let accent = channel_color(name, index);
            ui.horizontal(|ui| {
                ui.colored_label(accent, "●");
                if ui.selectable_label(self.selected_channel == index, format!("{name}{suffix}")).clicked() {
                    self.select_channel(index, true);
                }
            });
        }
        if self.solo_channel.is_some() && ui.small_button("Return to composite").clicked() { self.show_composite(); }

        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Histogram");
            let label = if self.settings.show_all_histograms { "All channels" } else { "Selected" };
            if ui.small_button(label).clicked() {
                self.settings.show_all_histograms = !self.settings.show_all_histograms;
                self.save_settings_quietly();
            }
        });
        if self.settings.show_all_histograms {
            for (index, name) in channel_names.iter().enumerate() {
                let accent = self.settings.colorize_histograms.then(|| channel_color(name, index));
                ui.colored_label(accent.unwrap_or(ui.visuals().text_color()), name);
                draw_histogram(ui, original_histograms.get(index), adjusted_histograms.get(index), accent);
            }
        } else {
            let index = self.selected_channel;
            let accent = self.settings.colorize_histograms.then(|| channel_color(&channel_names[index], index));
            ui.strong(format!("Histogram — {}", channel_names[index]));
            draw_histogram(ui, original_histograms.get(index), adjusted_histograms.get(index), accent);
        }
    }

    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {
        let Some(face) = self.faces.get(self.current_face) else {
            ui.heading("Adjustments");
            ui.label("No active face");
            return;
        };
        let channel_names = face.preview.metadata.channel_names.clone();
        if channel_names.is_empty() { return; }
        self.selected_channel = self.selected_channel.min(channel_names.len() - 1);
        let output_name = channel_names[self.selected_channel].clone();
        let active_histogram = face.adjusted.get(self.selected_channel).map(|values| render::histogram(values));
        let accent = self.settings.colorize_adjustments.then(|| channel_color(&output_name, self.selected_channel));

        ui.horizontal_wrapped(|ui| {
            ui.heading("Adjustments");
            ui.selectable_value(&mut self.adjustment_scope, AdjustmentScope::Selected, &output_name);
            ui.selectable_value(&mut self.adjustment_scope, AdjustmentScope::All, "All channels");
            let layout_label = if self.settings.adjustment_tabs { "Tabs" } else { "Stacked" };
            if ui.small_button(layout_label).clicked() {
                self.settings.adjustment_tabs = !self.settings.adjustment_tabs;
                self.save_settings_quietly();
            }
        });
        if ui.button("Reset all adjustments").clicked() {
            self.project.reset_adjustments(&channel_names);
            self.mark_all_previews_dirty();
            self.report_info("All adjustments reset to defaults");
        }

        let changed = match self.adjustment_scope {
            AdjustmentScope::Selected => self.ui_selected_adjustment(ui, &output_name, &channel_names, active_histogram.as_ref(), accent),
            AdjustmentScope::All => self.ui_all_adjustments(ui, &output_name, &channel_names, active_histogram.as_ref(), accent),
        };
        if changed { self.mark_all_previews_dirty(); }
    }

    fn ui_selected_adjustment(
        &mut self,
        ui: &mut egui::Ui,
        output_name: &str,
        channel_names: &[String],
        histogram: Option<&[u32; 256]>,
        accent: Option<egui::Color32>,
    ) -> bool {
        let mut changed = false;
        let adjustment = self.project.adjustments.entry(output_name.to_owned()).or_default();
        changed |= ui.checkbox(&mut adjustment.enabled, "Enable adjustment for this channel").changed();
        ui.add_enabled_ui(adjustment.enabled, |ui| {
            if self.settings.adjustment_tabs {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                    ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                    ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
                });
                changed |= match self.tool {
                    ToolPanel::Levels => levels_ui(ui, adjustment, accent),
                    ToolPanel::Curves => curves_ui(ui, adjustment, histogram.filter(|_| self.settings.show_curve_histogram), accent),
                    ToolPanel::Mixer => mixer_ui(ui, adjustment, output_name, channel_names, accent),
                };
            } else {
                ui.group(|ui| {
                    ui.strong("Levels");
                    changed |= levels_ui(ui, adjustment, accent);
                });
                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.strong("Curve");
                    changed |= curves_ui(ui, adjustment, histogram.filter(|_| self.settings.show_curve_histogram), accent);
                });
                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.strong("Channel Mixer");
                    changed |= mixer_ui(ui, adjustment, output_name, channel_names, accent);
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
        histogram: Option<&[u32; 256]>,
        accent: Option<egui::Color32>,
    ) -> bool {
        let mut changed = false;
        let enabled_count = channel_names.iter()
            .filter(|name| self.project.adjustments.get(*name).map(|adjustment| adjustment.enabled).unwrap_or(true))
            .count();
        let mut all_enabled = enabled_count == channel_names.len();
        if ui.checkbox(&mut all_enabled, "Enable adjustments on all channels").changed() {
            for name in channel_names {
                self.project.adjustments.entry(name.clone()).or_default().enabled = all_enabled;
            }
            changed = true;
        }
        ui.small("Levels and Curve broadcast to every channel. Mixer output rows remain independent.");

        if self.settings.adjustment_tabs {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
            });
            changed |= match self.tool {
                ToolPanel::Levels => broadcast_levels_ui(ui, &mut self.project.adjustments, template_name, channel_names, accent),
                ToolPanel::Curves => broadcast_curves_ui(ui, &mut self.project.adjustments, template_name, channel_names, histogram.filter(|_| self.settings.show_curve_histogram), accent),
                ToolPanel::Mixer => all_mixers_ui(ui, &mut self.project.adjustments, channel_names, self.settings.colorize_adjustments),
            };
        } else {
            ui.group(|ui| {
                ui.strong("Levels — all channels");
                changed |= broadcast_levels_ui(ui, &mut self.project.adjustments, template_name, channel_names, accent);
            });
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.strong("Curve — all channels");
                changed |= broadcast_curves_ui(ui, &mut self.project.adjustments, template_name, channel_names, histogram.filter(|_| self.settings.show_curve_histogram), accent);
            });
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.strong("Channel Mixer — all output rows");
                changed |= all_mixers_ui(ui, &mut self.project.adjustments, channel_names, self.settings.colorize_adjustments);
            });
        }
        changed
    }

    fn ui_tools(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Tools");
            let layout = if self.settings.sidebar_two_columns { "2 columns" } else { "1 column" };
            if ui.small_button(layout).clicked() {
                self.settings.sidebar_two_columns = !self.settings.sidebar_two_columns;
                self.save_settings_quietly();
            }
        });
        ui.separator();
        if self.settings.sidebar_two_columns {
            ui.columns(2, |columns| {
                egui::ScrollArea::vertical().id_salt("channels-column").show(&mut columns[0], |ui| self.ui_channels_histogram(ui));
                egui::ScrollArea::vertical().id_salt("adjustments-column").show(&mut columns[1], |ui| self.ui_adjustments(ui));
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
                    if ui.button("Add TIFF faces").clicked() { self.add_faces_dialog(); }
                });
            });
            return;
        };

        let title = self.project.faces.get(self.current_face)
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
                ui.label("DPI not set (72 used for test-code sizing)");
            }
        });
        ui.separator();

        let Some(texture) = texture else {
            ui.centered_and_justified(|ui| { ui.spinner(); });
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
                    (image_size.x + VIEWPORT_OVERSCROLL * 2.0).max(viewport.width() + VIEWPORT_OVERSCROLL * 2.0),
                    (image_size.y + VIEWPORT_OVERSCROLL * 2.0).max(viewport.height() + VIEWPORT_OVERSCROLL * 2.0),
                );
                let (canvas_rect, _) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
                let image_rect = egui::Rect::from_center_size(canvas_rect.center(), image_size);
                ui.put(image_rect, egui::Image::from_texture(&texture).fit_to_exact_size(image_size));
                if recenter {
                    ui.scroll_to_rect(image_rect, Some(egui::Align::Center));
                }
            });
        let _ = output;
        if recenter { self.viewport_recenter = false; }
    }

    fn ui_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let dirty = if self.project_dirty { " • modified" } else { "" };
            ui.label(format!("{}{}", self.status_message, dirty));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Fit").clicked() {
                    self.fit_requested = true;
                }
                let zoom = ui.add(egui::Slider::new(&mut self.zoom, 0.05..=8.0).logarithmic(true).text("Zoom"));
                if zoom.changed() { self.viewport_recenter = true; }
                if let Some(path) = &self.project_path { ui.label(path.display().to_string()); }
            });
        });
    }

    fn ui_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings { return; }
        let mut open = self.show_settings;
        egui::Window::new("Settings").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.heading("Application");
            let mut changed = false;
            changed |= ui.checkbox(&mut self.settings.auto_update, "Automatically check and download updates").changed();
            let dark_changed = ui.checkbox(&mut self.settings.dark_mode, "Dark mode").changed();
            changed |= dark_changed;
            changed |= ui.add(egui::Slider::new(&mut self.settings.max_preview_dimension, 600..=4000).text("Preview max dimension")).changed();
            ui.separator();
            ui.heading("Editor layout");
            changed |= ui.checkbox(&mut self.settings.sidebar_two_columns, "Use two-column tools sidebar").changed();
            changed |= ui.checkbox(&mut self.settings.show_all_histograms, "Show a histogram for every channel").changed();
            changed |= ui.checkbox(&mut self.settings.adjustment_tabs, "Use tabs for Levels / Curve / Mixer").changed();
            ui.separator();
            ui.heading("Color guides");
            changed |= ui.checkbox(&mut self.settings.colorize_histograms, "Colorize histograms by channel").changed();
            changed |= ui.checkbox(&mut self.settings.colorize_adjustments, "Colorize Levels / Curve / Mixer by channel").changed();
            changed |= ui.checkbox(&mut self.settings.show_curve_histogram, "Show active histogram behind Curve").changed();
            if dark_changed { apply_theme(ctx, self.settings.dark_mode); }
            if changed {
                if let Err(err) = self.settings.save() { self.report_error(err); }
            }
        });
        self.show_settings = open;
    }

    fn ui_about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about { return; }
        let mut open = self.show_about;
        egui::Window::new("About Shade Editor").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.heading("Shade Editor");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label("Native multi-channel TIFF shade editor for digital ceramic printing.");
            ui.separator();
            ui.label("Copyright © 2026 Emad Ghasemi");
            ui.label("MIT License");
            ui.hyperlink_to("GitHub repository", "https://github.com/emadgh/windows-shade-editor");
            ui.separator();
            ui.label("Update controls are located on the right side of the main toolbar.");
        });
        self.show_about = open;
    }

    fn ui_logs_window(&mut self, ctx: &egui::Context) {
        if !self.show_logs { return; }
        let mut open = self.show_logs;
        egui::Window::new("Application log").open(&mut open).default_size([780.0, 480.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Refresh").clicked() { self.log_cache = self.log.read(); }
                if ui.button("Clear").clicked() {
                    match self.log.clear() {
                        Ok(()) => self.log_cache.clear(),
                        Err(err) => self.report_error(err),
                    }
                }
                ui.label(self.log.path().display().to_string());
            });
            ui.separator();
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.log_cache).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(22));
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
            .show(ui, |ui| egui::ScrollArea::vertical().show(ui, |ui| self.ui_faces(ui)));
        let tools_width = if self.settings.sidebar_two_columns { 760.0 } else { 400.0 };
        egui::Panel::right("tools")
            .default_size(tools_width)
            .min_size(if self.settings.sidebar_two_columns { 560.0 } else { 300.0 })
            .resizable(true)
            .show(ui, |ui| self.ui_tools(ui));
        egui::CentralPanel::default().show(ui, |ui| self.ui_viewport(ui));

        self.ui_settings_window(ui.ctx());
        self.ui_about_window(ui.ctx());
        self.ui_logs_window(ui.ctx());

        self.start_render_if_needed(ui.ctx());
    }
}

fn levels_ui(ui: &mut egui::Ui, adjustment: &mut ChannelAdjustment, accent: Option<egui::Color32>) -> bool {
    with_accent(ui, accent, |ui| {
        let levels = &mut adjustment.levels;
        let mut changed = false;
        changed |= ui.add(egui::Slider::new(&mut levels.input_black, 0.0..=0.98).text("Input black")).changed();
        changed |= ui.add(egui::Slider::new(&mut levels.gamma, 0.1..=4.0).logarithmic(true).text("Gamma (relative)")).changed();
        changed |= ui.add(egui::Slider::new(&mut levels.input_white, 0.02..=1.0).text("Input white")).changed();
        changed |= ui.add(egui::Slider::new(&mut levels.output_black, 0.0..=1.0).text("Output black")).changed();
        changed |= ui.add(egui::Slider::new(&mut levels.output_white, 0.0..=1.0).text("Output white")).changed();
        if levels.input_white <= levels.input_black {
            levels.input_white = (levels.input_black + 0.01).min(1.0);
            changed = true;
        }
        ui.small(format!("Gamma midpoint output: {:.3}", model::levels_gamma_mid_output(*levels)));
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
        changed |= ui.add(egui::Slider::new(&mut adjustment.curve.black, 0.0..=1.0).text("Black output")).changed();
        changed |= ui.add(egui::Slider::new(&mut adjustment.curve.midpoint, 0.0..=1.0).text("Midpoint (relative)")).changed();
        changed |= ui.add(egui::Slider::new(&mut adjustment.curve.white, 0.0..=1.0).text("White output")).changed();
        ui.small(format!("Calculated midpoint output: {:.3}", model::curve_mid_output(adjustment.curve)));
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
) -> bool {
    with_accent(ui, accent, |ui| {
        ui.label(format!("Output: {output_name}"));
        let mut changed = false;
        for name in channel_names {
            let default = if name == output_name { 1.0 } else { 0.0 };
            let coefficient = adjustment.mixer.coefficients.entry(name.clone()).or_insert(default);
            changed |= ui.add(egui::Slider::new(coefficient, -2.0..=2.0).text(name)).changed();
        }
        changed |= ui.add(egui::Slider::new(&mut adjustment.mixer.constant, -1.0..=1.0).text("Constant")).changed();
        if ui.small_button("Reset Mixer").clicked() {
            adjustment.mixer.coefficients.clear();
            for name in channel_names {
                adjustment.mixer.coefficients.insert(name.clone(), if name == output_name { 1.0 } else { 0.0 });
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
    if !levels_ui(ui, &mut draft, accent) { return false; }
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
    if !curves_ui(ui, &mut draft, histogram, accent) { return false; }
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().curve = draft.curve;
    }
    true
}

fn all_mixers_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
    colorize: bool,
) -> bool {
    let mut changed = false;
    for (index, output_name) in channel_names.iter().enumerate() {
        ui.collapsing(format!("Output — {output_name}"), |ui| {
            let adjustment = adjustments.entry(output_name.clone()).or_default();
            let accent = colorize.then(|| channel_color(output_name, index));
            changed |= mixer_ui(ui, adjustment, output_name, channel_names, accent);
        });
    }
    changed
}

fn with_accent<R>(ui: &mut egui::Ui, accent: Option<egui::Color32>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        if let Some(color) = accent {
            let visuals = ui.visuals_mut();
            visuals.selection.bg_fill = color;
            visuals.selection.stroke.color = color;
            visuals.widgets.active.bg_fill = color.gamma_multiply(0.45);
            visuals.widgets.hovered.bg_fill = color.gamma_multiply(0.28);
        }
        add(ui)
    }).inner
}

fn channel_color(name: &str, index: usize) -> egui::Color32 {
    let lower = name.to_ascii_lowercase();
    if lower == "cyan" || lower == "c" { return egui::Color32::from_rgb(0, 190, 220); }
    if lower == "magenta" || lower == "m" { return egui::Color32::from_rgb(225, 45, 150); }
    if lower == "yellow" || lower == "y" { return egui::Color32::from_rgb(225, 190, 20); }
    if lower == "black" || lower == "k" { return egui::Color32::from_rgb(155, 155, 155); }
    const SPOTS: [(u8, u8, u8); 8] = [
        (130, 95, 220), (60, 180, 95), (235, 105, 55), (65, 135, 230),
        (220, 80, 95), (40, 180, 175), (190, 110, 45), (180, 80, 190),
    ];
    let (r, g, b) = SPOTS[index % SPOTS.len()];
    egui::Color32::from_rgb(r, g, b)
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
    painter.rect_stroke(rect, 2.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    let max_value = original.into_iter().flat_map(|bins| bins.iter())
        .chain(adjusted.into_iter().flat_map(|bins| bins.iter()))
        .copied().max().unwrap_or(1).max(1) as f32;
    let original_color = ui.visuals().weak_text_color();
    let adjusted_color = accent.unwrap_or(ui.visuals().selection.stroke.color);
    for index in 0..256 {
        let x = egui::lerp(rect.x_range(), index as f32 / 255.0);
        if let Some(bins) = original {
            let h = bins[index] as f32 / max_value * rect.height();
            painter.line_segment([egui::pos2(x, rect.bottom()), egui::pos2(x, rect.bottom() - h)], egui::Stroke::new(1.0, original_color));
        }
        if let Some(bins) = adjusted {
            let h = bins[index] as f32 / max_value * rect.height();
            painter.line_segment([egui::pos2(x, rect.bottom()), egui::pos2(x, rect.bottom() - h)], egui::Stroke::new(1.0, adjusted_color));
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
    painter.rect_stroke(rect, 2.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);

    if let Some(bins) = histogram {
        let max_value = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
        let hist_color = accent.unwrap_or(ui.visuals().weak_text_color()).gamma_multiply(0.35);
        for (index, value) in bins.iter().enumerate() {
            let x = egui::lerp(rect.x_range(), index as f32 / 255.0);
            let h = *value as f32 / max_value * rect.height();
            painter.line_segment([egui::pos2(x, rect.bottom()), egui::pos2(x, rect.bottom() - h)], egui::Stroke::new(1.0, hist_color));
        }
    }

    painter.line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.top())],
        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
    );
    let curve_color = accent.unwrap_or(ui.visuals().selection.stroke.color);
    let mut last = None;
    for step in 0..=96 {
        let x = step as f32 / 96.0;
        let y = model::apply_curve(x, curve);
        let point = egui::pos2(egui::lerp(rect.x_range(), x), egui::lerp(rect.bottom()..=rect.top(), y));
        if let Some(previous) = last {
            painter.line_segment([previous, point], egui::Stroke::new(2.0, curve_color));
        }
        last = Some(point);
    }
}

fn apply_theme(ctx: &egui::Context, dark: bool) {
    if dark { ctx.set_visuals(egui::Visuals::dark()); } else { ctx.set_visuals(egui::Visuals::light()); }
}

fn sanitize_filename(value: &str) -> String {
    let filtered = value
        .chars()
        .map(|ch| if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { ch })
        .collect::<String>();
    if filtered.trim().is_empty() { "shade-project".to_owned() } else { filtered }
}
