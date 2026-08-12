#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod export;
mod model;
mod render;
mod settings;
mod tiff_io;
mod update;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui;
use model::{ChannelAdjustment, ShadeProject};
use settings::AppSettings;
use tiff_io::PreviewFace;
use update::{UpdateManager, UpdateStatus};

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("Shade Editor")
            .with_inner_size([1500.0, 900.0])
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
    preview: PreviewFace,
    adjusted: Vec<Vec<u16>>,
    texture: Option<egui::TextureHandle>,
    dirty: bool,
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
    settings: AppSettings,
    updater: UpdateManager,
    show_settings: bool,
    show_about: bool,
    status_message: String,
    project_dirty: bool,
}

impl ShadeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = AppSettings::load();
        apply_theme(&cc.egui_ctx, settings.dark_mode);
        let updater = UpdateManager::default();
        if settings.auto_update {
            updater.start_check(true);
        }
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
            settings,
            updater,
            show_settings: false,
            show_about: false,
            status_message: "Ready".to_owned(),
            project_dirty: false,
        }
    }

    fn new_project(&mut self) {
        self.project = ShadeProject::default();
        self.project_path = None;
        self.faces.clear();
        self.current_face = 0;
        self.selected_channel = 0;
        self.solo_channel = None;
        self.adjustment_scope = AdjustmentScope::Selected;
        self.status_message = "New shade project".to_owned();
        self.project_dirty = false;
    }

    fn add_faces_dialog(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("TIFF images", &["tif", "tiff"])
            .pick_files()
        else { return; };

        let mut added = 0usize;
        let mut last_error = None;
        for path in paths {
            match tiff_io::load_preview(&path, self.settings.max_preview_dimension) {
                Ok(preview) => {
                    self.project.ensure_channels(&preview.metadata.channel_names);
                    let label = path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Face".to_owned());
                    self.project.faces.push(model::FaceRef {
                        path: path.to_string_lossy().into_owned(),
                        label,
                    });
                    self.faces.push(RuntimeFace {
                        path,
                        preview,
                        adjusted: Vec::new(),
                        texture: None,
                        dirty: true,
                    });
                    added += 1;
                }
                Err(err) => last_error = Some(err),
            }
        }

        if added > 0 {
            self.current_face = self.faces.len().saturating_sub(added);
            self.selected_channel = 0;
            self.solo_channel = None;
            self.project_dirty = true;
            self.status_message = format!("Added {added} face(s)");
        }
        if let Some(err) = last_error {
            self.status_message = format!("Some TIFF files could not be loaded: {err}");
        }
    }

    fn open_project_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Shade project", &["shade"])
            .pick_file()
        else { return; };
        self.open_project(&path);
    }

    fn open_project(&mut self, path: &Path) {
        let project = match ShadeProject::load(path) {
            Ok(project) => project,
            Err(err) => {
                self.status_message = err;
                return;
            }
        };
        let resolved = project.resolve_face_paths(path);
        let mut runtime_faces = Vec::new();
        let mut errors = Vec::new();
        for source in &resolved {
            match tiff_io::load_preview(source, self.settings.max_preview_dimension) {
                Ok(preview) => runtime_faces.push(RuntimeFace {
                    path: source.clone(),
                    preview,
                    adjusted: Vec::new(),
                    texture: None,
                    dirty: true,
                }),
                Err(err) => errors.push(format!("{}: {err}", source.display())),
            }
        }

        let mut project = project;
        for face in &runtime_faces {
            project.ensure_channels(&face.preview.metadata.channel_names);
        }
        self.project = project;
        self.project_path = Some(path.to_path_buf());
        self.faces = runtime_faces;
        self.current_face = 0;
        self.selected_channel = 0;
        self.solo_channel = None;
        self.adjustment_scope = AdjustmentScope::Selected;
        self.project_dirty = false;
        self.status_message = if errors.is_empty() {
            format!("Opened {}", path.display())
        } else {
            format!("Project opened with {} missing/unsupported face(s)", errors.len())
        };
    }

    fn save_project(&mut self, save_as: bool) {
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
        let paths = self.faces.iter().map(|face| face.path.clone()).collect::<Vec<_>>();
        match self.project.save(&path, &paths) {
            Ok(()) => {
                self.project_path = Some(path.clone());
                self.project_dirty = false;
                self.status_message = format!("Saved {}", path.display());
            }
            Err(err) => self.status_message = err,
        }
    }

    fn export_current_dialog(&mut self) {
        let Some(face) = self.faces.get(self.current_face) else {
            self.status_message = "No active face to export".to_owned();
            return;
        };
        let stem = face.path.file_stem().map(|value| value.to_string_lossy()).unwrap_or_default();
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("TIFF image", &["tif", "tiff"])
            .set_file_name(format!("{stem}-shade.tif"))
            .save_file()
        else { return; };
        match export::export_face(&face.path, &destination, &self.project) {
            Ok(()) => self.status_message = format!("Exported {}", destination.display()),
            Err(err) => self.status_message = format!("Export failed: {err}"),
        }
    }

    fn export_all_dialog(&mut self) {
        if self.faces.is_empty() {
            self.status_message = "No faces to export".to_owned();
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else { return; };
        let mut exported = 0usize;
        for face in &self.faces {
            let stem = face.path.file_stem().map(|value| value.to_string_lossy()).unwrap_or_default();
            let destination = folder.join(format!("{stem}-shade.tif"));
            if let Err(err) = export::export_face(&face.path, &destination, &self.project) {
                self.status_message = format!("Export stopped at {}: {err}", face.path.display());
                return;
            }
            exported += 1;
        }
        self.status_message = format!("Exported {exported} face(s) to {}", folder.display());
    }

    fn remove_current_face(&mut self) {
        if self.current_face >= self.faces.len() { return; }
        self.faces.remove(self.current_face);
        if self.current_face < self.project.faces.len() {
            self.project.faces.remove(self.current_face);
        }
        self.current_face = self.current_face.min(self.faces.len().saturating_sub(1));
        self.selected_channel = 0;
        self.solo_channel = None;
        self.project_dirty = true;
        self.status_message = "Face removed from project (source TIFF was not deleted)".to_owned();
    }

    fn mark_all_previews_dirty(&mut self) {
        for face in &mut self.faces { face.dirty = true; }
        self.project_dirty = true;
    }

    fn ensure_current_texture(&mut self, ctx: &egui::Context) {
        let Some(face) = self.faces.get_mut(self.current_face) else { return; };
        if !face.dirty && face.texture.is_some() { return; }
        face.adjusted = render::adjusted_planes(&face.preview, &self.project);
        let rgba = render::rgba_from_planes(&face.preview, &face.adjusted, self.solo_channel);
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [face.preview.width, face.preview.height],
            &rgba,
        );
        let options = egui::TextureOptions::LINEAR;
        if let Some(texture) = &mut face.texture {
            texture.set(image, options);
        } else {
            let id = format!("face-preview-{}", self.current_face);
            face.texture = Some(ctx.load_texture(id, image, options));
        }
        face.dirty = false;
    }

    fn select_channel(&mut self, channel: usize, isolate: bool) {
        self.selected_channel = channel;
        let next_solo = if isolate { Some(channel) } else { None };
        if self.solo_channel != next_solo {
            self.solo_channel = next_solo;
            if let Some(face) = self.faces.get_mut(self.current_face) { face.dirty = true; }
        }
    }

    fn show_composite(&mut self) {
        if self.solo_channel.is_some() {
            self.solo_channel = None;
            if let Some(face) = self.faces.get_mut(self.current_face) { face.dirty = true; }
        }
    }

    fn save_settings_quietly(&mut self) {
        if let Err(err) = self.settings.save() { self.status_message = err; }
    }

    fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("New").clicked() { self.new_project(); }
            if ui.button("Open .shade").clicked() { self.open_project_dialog(); }
            if ui.button("Add TIFF faces").clicked() { self.add_faces_dialog(); }
            ui.separator();
            if ui.add_enabled(!self.faces.is_empty(), egui::Button::new("Save")).clicked() { self.save_project(false); }
            if ui.add_enabled(!self.faces.is_empty(), egui::Button::new("Save As")).clicked() { self.save_project(true); }
            ui.separator();
            if ui.add_enabled(!self.faces.is_empty(), egui::Button::new("Export face")).clicked() { self.export_current_dialog(); }
            if ui.add_enabled(!self.faces.is_empty(), egui::Button::new("Export all")).clicked() { self.export_all_dialog(); }
            ui.separator();
            if ui.button("Settings").clicked() { self.show_settings = true; }
            if ui.button("About").clicked() { self.show_about = true; }
        });
    }

    fn ui_update_banner(&mut self, ui: &mut egui::Ui) {
        match self.updater.status() {
            UpdateStatus::Idle | UpdateStatus::UpToDate => {}
            UpdateStatus::Checking => { ui.horizontal(|ui| { ui.spinner(); ui.label("Checking for updates…"); }); }
            UpdateStatus::Available(info) => {
                ui.horizontal(|ui| {
                    ui.label(format!("Shade Editor {} is available.", info.version));
                    if ui.button("Download").clicked() { self.updater.start_download(); }
                    ui.hyperlink_to("Release", info.release_url);
                });
            }
            UpdateStatus::Downloading(info) => { ui.horizontal(|ui| { ui.spinner(); ui.label(format!("Downloading Shade Editor {}…", info.version)); }); }
            UpdateStatus::Ready(info, _) => {
                ui.horizontal(|ui| {
                    ui.label(format!("Shade Editor {} is ready to install.", info.version));
                    if ui.button("Restart and update").clicked() {
                        match self.updater.apply_ready() {
                            Ok(true) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                            Ok(false) => {}
                            Err(err) => self.status_message = err,
                        }
                    }
                });
            }
            UpdateStatus::Failed(message) => {
                ui.horizontal(|ui| {
                    ui.label(format!("Update check failed: {message}"));
                    if ui.button("Retry").clicked() { self.updater.start_check(false); }
                });
            }
        }
    }

    fn ui_faces(&mut self, ui: &mut egui::Ui) {
        ui.heading("Faces");
        if self.faces.is_empty() {
            ui.label("Add TIFF files to create a shade project.");
            return;
        }
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
            if let Some(face) = self.faces.get_mut(index) { face.dirty = true; }
        }
        ui.separator();
        if ui.button("Remove active face").clicked() { self.remove_current_face(); }
        ui.separator();
        self.ui_snapshots(ui);
    }

    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Snapshots");
            if ui.small_button("+ New").clicked() {
                let id = self.project.create_snapshot();
                self.project_dirty = true;
                self.status_message = format!("Created snapshot #{id}");
            }
        });
        ui.small("Snapshots store adjustment settings only.");

        if self.project.snapshots.is_empty() {
            ui.label("No saved tests yet.");
            return;
        }

        let active_id = self.project.active_snapshot_id;
        let mut requested_load = None;
        for snapshot in &self.project.snapshots {
            let selected = active_id == Some(snapshot.id);
            let label = if selected && !self.project.active_snapshot_matches() {
                format!("{}  *", snapshot.name)
            } else {
                snapshot.name.clone()
            };
            if ui.selectable_label(selected, label).clicked() {
                requested_load = Some(snapshot.id);
            }
        }

        if let Some(id) = requested_load {
            if self.project.apply_snapshot(id) {
                self.mark_all_previews_dirty();
                self.status_message = "Snapshot loaded".to_owned();
            }
        }

        let Some(active_id) = self.project.active_snapshot_id else { return; };
        ui.separator();
        ui.label("Snapshot name");
        let mut renamed = false;
        if let Some(snapshot) = self.project.snapshots.iter_mut().find(|snapshot| snapshot.id == active_id) {
            renamed = ui.text_edit_singleline(&mut snapshot.name).changed();
        }
        if renamed { self.project_dirty = true; }

        if !self.project.active_snapshot_matches() {
            ui.small("Current adjustments differ from this snapshot.");
        }

        let mut update = false;
        let mut delete = false;
        ui.horizontal_wrapped(|ui| {
            update = ui.button("Update").clicked();
            delete = ui.button("Delete").clicked();
        });
        if update && self.project.update_snapshot(active_id) {
            self.project_dirty = true;
            self.status_message = "Snapshot updated from current adjustments".to_owned();
        }
        if delete && self.project.delete_snapshot(active_id) {
            self.project_dirty = true;
            self.status_message = "Snapshot deleted".to_owned();
        }
    }

    fn ui_channels_and_tools(&mut self, ui: &mut egui::Ui) {
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
            let suffix = if index >= base_count { "  • extra/spot" } else { "" };
            if ui.selectable_label(self.selected_channel == index, format!("{name}{suffix}")).clicked() {
                self.select_channel(index, true);
            }
        }
        if self.solo_channel.is_some() && ui.small_button("Return to composite (keep channel selected)").clicked() {
            self.show_composite();
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Histogram");
            let label = if self.settings.show_all_histograms { "All channels" } else { "Selected only" };
            if ui.small_button(label).clicked() {
                self.settings.show_all_histograms = !self.settings.show_all_histograms;
                self.save_settings_quietly();
            }
        });
        if self.settings.show_all_histograms {
            for (index, name) in channel_names.iter().enumerate() {
                ui.label(name);
                draw_histogram(ui, original_histograms.get(index), adjusted_histograms.get(index));
            }
        } else {
            let index = self.selected_channel;
            ui.strong(format!("Histogram — {}", channel_names[index]));
            draw_histogram(ui, original_histograms.get(index), adjusted_histograms.get(index));
        }

        ui.separator();
        let output_name = channel_names[self.selected_channel].clone();
        ui.horizontal_wrapped(|ui| {
            ui.strong("Adjustments");
            ui.selectable_value(&mut self.adjustment_scope, AdjustmentScope::Selected, &output_name);
            ui.selectable_value(&mut self.adjustment_scope, AdjustmentScope::All, "All channels");
            let layout_label = if self.settings.adjustment_tabs { "Tabs" } else { "Stacked" };
            if ui.small_button(layout_label).clicked() {
                self.settings.adjustment_tabs = !self.settings.adjustment_tabs;
                self.save_settings_quietly();
            }
        });

        let reset_all = ui.button("Reset all adjustments").clicked();
        if reset_all {
            self.project.reset_adjustments(&channel_names);
            self.mark_all_previews_dirty();
            self.status_message = "All adjustments reset to defaults".to_owned();
        }

        let changed = match self.adjustment_scope {
            AdjustmentScope::Selected => self.ui_selected_adjustment(ui, &output_name, &channel_names),
            AdjustmentScope::All => self.ui_all_adjustments(ui, &output_name, &channel_names),
        };
        if changed { self.mark_all_previews_dirty(); }

        ui.separator();
        ui.collapsing("Test code", |ui| {
            let mut test_changed = ui.checkbox(&mut self.project.test_code.enabled, "Write test code on export").changed();
            test_changed |= ui.text_edit_singleline(&mut self.project.test_code.text).changed();
            egui::ComboBox::from_label("Channel")
                .selected_text(&self.project.test_code.channel)
                .show_ui(ui, |ui| {
                    for name in &channel_names {
                        test_changed |= ui.selectable_value(&mut self.project.test_code.channel, name.clone(), name).changed();
                    }
                });
            test_changed |= ui.add(egui::Slider::new(&mut self.project.test_code.scale, 1..=8).text("Text scale")).changed();
            test_changed |= ui.add(egui::Slider::new(&mut self.project.test_code.margin_px, 0..=500).text("Margin px")).changed();
            if test_changed { self.project_dirty = true; }
        });
    }

    fn ui_selected_adjustment(
        &mut self,
        ui: &mut egui::Ui,
        output_name: &str,
        channel_names: &[String],
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
                    ToolPanel::Levels => levels_ui(ui, adjustment),
                    ToolPanel::Curves => curves_ui(ui, adjustment),
                    ToolPanel::Mixer => mixer_ui(ui, adjustment, output_name, channel_names),
                };
            } else {
                ui.group(|ui| {
                    ui.strong("Levels");
                    changed |= levels_ui(ui, adjustment);
                });
                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.strong("Curve");
                    changed |= curves_ui(ui, adjustment);
                });
                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.strong("Channel Mixer");
                    changed |= mixer_ui(ui, adjustment, output_name, channel_names);
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
        ui.small(format!("{enabled_count}/{} channels currently enabled", channel_names.len()));
        ui.small("Levels and Curve changes are broadcast to every channel. Mixer rows remain independent per output.");

        if self.settings.adjustment_tabs {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
            });
            changed |= match self.tool {
                ToolPanel::Levels => broadcast_levels_ui(ui, &mut self.project.adjustments, template_name, channel_names),
                ToolPanel::Curves => broadcast_curves_ui(ui, &mut self.project.adjustments, template_name, channel_names),
                ToolPanel::Mixer => all_mixers_ui(ui, &mut self.project.adjustments, channel_names),
            };
        } else {
            ui.group(|ui| {
                ui.strong("Levels — all channels");
                changed |= broadcast_levels_ui(ui, &mut self.project.adjustments, template_name, channel_names);
            });
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.strong("Curve — all channels");
                changed |= broadcast_curves_ui(ui, &mut self.project.adjustments, template_name, channel_names);
            });
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.strong("Channel Mixer — all output rows");
                changed |= all_mixers_ui(ui, &mut self.project.adjustments, channel_names);
            });
        }
        changed
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
        let meta = &face.preview.metadata;
        ui.horizontal_wrapped(|ui| {
            ui.strong(title);
            ui.separator();
            ui.label(format!("{} × {} px", meta.width, meta.height));
            ui.label(format!("{}-bit", meta.bit_depth));
            ui.label(meta.color_model.title());
            ui.label(format!("{} channels", meta.samples_per_pixel));
            if meta.samples_per_pixel > meta.base_channel_count {
                ui.label(format!("{} extra/spot", meta.samples_per_pixel - meta.base_channel_count));
            }
        });
        ui.separator();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            if let Some(texture) = self.faces.get(self.current_face).and_then(|face| face.texture.as_ref()) {
                let size = texture.size_vec2() * self.zoom;
                ui.add(egui::Image::from_texture(texture).fit_to_exact_size(size));
            }
        });
    }

    fn ui_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let dirty = if self.project_dirty { " • modified" } else { "" };
            ui.label(format!("{}{}", self.status_message, dirty));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.zoom, 0.1..=4.0).logarithmic(true).text("Zoom"));
                if let Some(path) = &self.project_path { ui.label(path.display().to_string()); }
            });
        });
    }

    fn ui_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings { return; }
        let mut open = self.show_settings;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading("Application");
                let mut changed = false;
                changed |= ui.checkbox(&mut self.settings.auto_update, "Automatically check and download updates").changed();
                let dark_changed = ui.checkbox(&mut self.settings.dark_mode, "Dark mode").changed();
                changed |= dark_changed;
                changed |= ui.add(
                    egui::Slider::new(&mut self.settings.max_preview_dimension, 600..=4000)
                        .text("Preview max dimension"),
                ).changed();
                ui.separator();
                ui.heading("Editor layout");
                changed |= ui.checkbox(&mut self.settings.show_all_histograms, "Show a histogram for every channel").changed();
                changed |= ui.checkbox(&mut self.settings.adjustment_tabs, "Use tabs for Levels / Curve / Mixer").changed();
                if dark_changed { apply_theme(ctx, self.settings.dark_mode); }
                if changed {
                    if let Err(err) = self.settings.save() { self.status_message = err; }
                }
                ui.separator();
                ui.label("Automatic updates can be disabled here. Manual update checking remains available in About.");
            });
        self.show_settings = open;
    }

    fn ui_about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about { return; }
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
                ui.hyperlink_to("GitHub repository", "https://github.com/emadgh/windows-shade-editor");
                ui.separator();
                match self.updater.status() {
                    UpdateStatus::Checking => { ui.spinner(); ui.label("Checking for updates…"); }
                    UpdateStatus::UpToDate => { ui.label("You are using the latest version."); }
                    UpdateStatus::Available(info) => {
                        ui.label(format!("Version {} is available.", info.version));
                        if ui.button("Download update").clicked() { self.updater.start_download(); }
                    }
                    UpdateStatus::Downloading(info) => { ui.label(format!("Downloading {}…", info.version)); }
                    UpdateStatus::Ready(info, _) => { ui.label(format!("Version {} is downloaded and ready to install.", info.version)); }
                    UpdateStatus::Failed(message) => { ui.label(format!("Update check failed: {message}")); }
                    UpdateStatus::Idle => {}
                }
                if ui.button("Check for updates").clicked() { self.updater.start_check(false); }
            });
        self.show_about = open;
    }
}

impl eframe::App for ShadeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        self.ensure_current_texture(ui.ctx());

        egui::Panel::top("toolbar").show(ui, |ui| {
            self.ui_toolbar(ui);
            self.ui_update_banner(ui);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.ui_status(ui));
        egui::Panel::left("faces")
            .default_size(240.0)
            .resizable(true)
            .show(ui, |ui| self.ui_faces(ui));
        egui::Panel::right("tools")
            .default_size(380.0)
            .resizable(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.ui_channels_and_tools(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| self.ui_viewport(ui));

        self.ui_settings_window(ui.ctx());
        self.ui_about_window(ui.ctx());
    }
}

fn levels_ui(ui: &mut egui::Ui, adjustment: &mut ChannelAdjustment) -> bool {
    let levels = &mut adjustment.levels;
    let mut changed = false;
    changed |= ui.add(egui::Slider::new(&mut levels.input_black, 0.0..=0.98).text("Input black")).changed();
    changed |= ui.add(egui::Slider::new(&mut levels.gamma, 0.1..=4.0).logarithmic(true).text("Gamma")).changed();
    changed |= ui.add(egui::Slider::new(&mut levels.input_white, 0.02..=1.0).text("Input white")).changed();
    changed |= ui.add(egui::Slider::new(&mut levels.output_black, 0.0..=1.0).text("Output black")).changed();
    changed |= ui.add(egui::Slider::new(&mut levels.output_white, 0.0..=1.0).text("Output white")).changed();
    if levels.input_white <= levels.input_black {
        levels.input_white = (levels.input_black + 0.01).min(1.0);
        changed = true;
    }
    if levels.output_white < levels.output_black {
        levels.output_white = levels.output_black;
        changed = true;
    }
    if ui.small_button("Reset Levels").clicked() {
        adjustment.levels = model::Levels::default();
        changed = true;
    }
    changed
}

fn curves_ui(ui: &mut egui::Ui, adjustment: &mut ChannelAdjustment) -> bool {
    draw_curve(ui, adjustment.curve);
    let mut changed = false;
    changed |= ui.add(egui::Slider::new(&mut adjustment.curve.black, 0.0..=1.0).text("Black output")).changed();
    changed |= ui.add(egui::Slider::new(&mut adjustment.curve.midpoint, 0.0..=1.0).text("Mid output")).changed();
    changed |= ui.add(egui::Slider::new(&mut adjustment.curve.white, 0.0..=1.0).text("White output")).changed();
    if ui.small_button("Reset Curve").clicked() {
        adjustment.curve = model::Curve::default();
        changed = true;
    }
    changed
}

fn mixer_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    output_name: &str,
    channel_names: &[String],
) -> bool {
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
}

fn broadcast_levels_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
) -> bool {
    let mut draft = adjustments.get(template_name).cloned().unwrap_or_default();
    if !levels_ui(ui, &mut draft) { return false; }
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
) -> bool {
    let mut draft = adjustments.get(template_name).cloned().unwrap_or_default();
    if !curves_ui(ui, &mut draft) { return false; }
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().curve = draft.curve;
    }
    true
}

fn all_mixers_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
) -> bool {
    let mut changed = false;
    for output_name in channel_names {
        ui.collapsing(format!("Output — {output_name}"), |ui| {
            let adjustment = adjustments.entry(output_name.clone()).or_default();
            changed |= mixer_ui(ui, adjustment, output_name, channel_names);
        });
    }
    changed
}

fn draw_histogram(ui: &mut egui::Ui, original: Option<&[u32; 256]>, adjusted: Option<&[u32; 256]>) {
    let desired = egui::vec2(ui.available_width().max(80.0), 105.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_stroke(rect, 2.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);

    let max_value = original.into_iter().flat_map(|bins| bins.iter())
        .chain(adjusted.into_iter().flat_map(|bins| bins.iter()))
        .copied().max().unwrap_or(1).max(1) as f32;
    let original_color = ui.visuals().weak_text_color();
    let adjusted_color = ui.visuals().selection.stroke.color;
    for index in 0..256 {
        let x = egui::lerp(rect.x_range(), index as f32 / 255.0);
        if let Some(bins) = original {
            let h = bins[index] as f32 / max_value * rect.height();
            painter.line_segment(
                [egui::pos2(x, rect.bottom()), egui::pos2(x, rect.bottom() - h)],
                egui::Stroke::new(1.0, original_color),
            );
        }
        if let Some(bins) = adjusted {
            let h = bins[index] as f32 / max_value * rect.height();
            painter.line_segment(
                [egui::pos2(x, rect.bottom()), egui::pos2(x, rect.bottom() - h)],
                egui::Stroke::new(1.0, adjusted_color),
            );
        }
    }
}

fn draw_curve(ui: &mut egui::Ui, curve: model::Curve) {
    let desired = egui::vec2(ui.available_width().min(300.0).max(120.0), 150.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_stroke(rect, 2.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    painter.line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.top())],
        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
    );
    let mut last = None;
    for step in 0..=64 {
        let x = step as f32 / 64.0;
        let y = model::apply_curve(x, curve);
        let point = egui::pos2(
            egui::lerp(rect.x_range(), x),
            egui::lerp(rect.bottom()..=rect.top(), y),
        );
        if let Some(previous) = last {
            painter.line_segment([previous, point], ui.visuals().selection.stroke);
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
