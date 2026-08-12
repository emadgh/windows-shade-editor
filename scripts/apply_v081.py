from pathlib import Path

APP = Path("src/app_main.rs")
CARGO = Path("Cargo.toml")
NOTES = Path("RELEASE_NOTES.md")

app = APP.read_text(encoding="utf-8")


def replace_once(old: str, new: str, label: str) -> None:
    global app
    if old not in app:
        raise SystemExit(f"anchor not found: {label}")
    app = app.replace(old, new, 1)


replace_once(
    "    snapshot_rename_id: Option<u64>,\n    snapshot_rename_buffer: String,\n    job: Option<JobHandle>,",
    "    snapshot_rename_id: Option<u64>,\n    snapshot_rename_buffer: String,\n    pending_snapshot_load: Option<u64>,\n    show_close_confirmation: bool,\n    close_after_save: bool,\n    allow_close_once: bool,\n    job: Option<JobHandle>,",
    "ShadeApp state fields",
)

replace_once(
    "            snapshot_rename_id: None,\n            snapshot_rename_buffer: String::new(),\n            job: None,",
    "            snapshot_rename_id: None,\n            snapshot_rename_buffer: String::new(),\n            pending_snapshot_load: None,\n            show_close_confirmation: false,\n            close_after_save: false,\n            allow_close_once: false,\n            job: None,",
    "ShadeApp state initialization",
)

replace_once(
    "        self.snapshot_rename_id = None;\n        self.snapshot_rename_buffer.clear();\n        self.report_info(\"New shade project\");",
    "        self.snapshot_rename_id = None;\n        self.snapshot_rename_buffer.clear();\n        self.pending_snapshot_load = None;\n        self.show_close_confirmation = false;\n        self.close_after_save = false;\n        self.report_info(\"New shade project\");",
    "new project state reset",
)

# Turn save_project into a success/failure-to-launch operation so Save & Exit can
# wait for the existing background save job without guessing whether Save As was cancelled.
save_start = app.index("    fn save_project(&mut self, save_as: bool) {")
save_end = app.index("    fn export_current_dialog", save_start)
save = app[save_start:save_end]
save = save.replace(
    "    fn save_project(&mut self, save_as: bool) {\n        if self.job.is_some() || self.faces.is_empty() {\n            return;\n        }",
    "    fn save_project(&mut self, save_as: bool) -> bool {\n        if self.job.is_some() || self.faces.is_empty() {\n            return false;\n        }",
    1,
)
save = save.replace("        let Some(path) = target else {\n            return;\n        };", "        let Some(path) = target else {\n            return false;\n        };", 1)
if not save.rstrip().endswith("});"):
    raise SystemExit("unexpected save_project ending")
save = save.rstrip() + "\n        true\n    }\n\n"
app = app[:save_start] + save + app[save_end:]

replace_once(
    "                Err(err) => self.report_error(err),\n            },\n            JobResult::Export(payload) => {",
    "                Err(err) => {\n                    self.close_after_save = false;\n                    self.report_error(err);\n                }\n            },\n            JobResult::Export(payload) => {",
    "save error close guard",
)

# Mark duplicate Faces without preventing them from being added.
old_faces_loop = '''            let mut requested_face = None;
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
            }'''
new_faces_loop = '''            let duplicate_counts = duplicate_face_counts(&self.faces);
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
                let duplicate_count = duplicate_counts
                    .get(&face_identity_key(&face.path))
                    .copied()
                    .unwrap_or(1);
                let display_label = if duplicate_count > 1 {
                    format!("{label}  [duplicate x{duplicate_count}]")
                } else {
                    label.to_owned()
                };
                let duplicate_accent = (duplicate_count > 1)
                    .then_some(egui::Color32::from_rgb(235, 155, 70));
                if clickable_row(
                    ui,
                    self.current_face == index,
                    &display_label,
                    None,
                    duplicate_accent,
                    32.0,
                )
                .on_hover_text(if duplicate_count > 1 {
                    "This TIFF is referenced more than once in the Faces list."
                } else {
                    "Face"
                })
                .clicked()
                {
                    requested_face = Some(index);
                }
            }'''
replace_once(old_faces_loop, new_faces_loop, "Faces duplicate marker")

# Snapshot switching now protects dirty edits.
old_snapshot_load = '''        if let Some(id) = requested_load {
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
        }'''
new_snapshot_load = '''        if let Some(id) = requested_load {
            self.request_snapshot_load(id);
        }'''
replace_once(old_snapshot_load, new_snapshot_load, "snapshot load guard")

# File information: filename, bit depth, physical cm, pixel dimensions, DPI, model, channels.
old_view_header = '''        let title = self
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
            ui.label(format!("{}-bit", meta.bit_depth));
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
        });'''
new_view_header = '''        let file_name = face
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| face.path.display().to_string());
        let meta = face.preview.metadata.clone();
        let dpi_info = face.dpi;
        let texture = face.texture.clone();
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
        });'''
replace_once(old_view_header, new_view_header, "file information header")

# Add dirty-snapshot and dirty-project close methods just before ui_faces.
methods_anchor = "    fn ui_faces(&mut self, ui: &mut egui::Ui) {"
methods = r'''    fn apply_snapshot_now(&mut self, id: u64) {
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

    fn request_snapshot_load(&mut self, id: u64) {
        if self.project.active_snapshot_id == Some(id) {
            return;
        }
        let active_snapshot_dirty = self.project.active_snapshot_id.is_some()
            && !self.project.active_snapshot_matches();
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
                        .add_enabled(self.job.is_none() && !self.faces.is_empty(), egui::Button::new("Save and exit"))
                        .clicked();
                    discard_and_exit = ui.button("Discard and exit").clicked();
                    stay = ui.button("Stay").clicked();
                });
            });

        if stay {
            self.show_close_confirmation = false;
        } else if discard_and_exit {
            self.show_close_confirmation = false;
            self.allow_close_once = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if save_and_exit && self.save_project(false) {
            self.show_close_confirmation = false;
            self.close_after_save = true;
        }
    }

'''
replace_once(methods_anchor, methods + methods_anchor, "dirty confirmation methods")

# Update frame logic: intercept OS close and close only after a successful async Save.
old_ui_start = '''        self.poll_job();
        self.poll_render(ui.ctx());
        self.sync_update_state();

        egui::Panel::top("toolbar").show(ui, |ui| self.ui_toolbar(ui));'''
new_ui_start = '''        self.poll_job();
        self.poll_render(ui.ctx());
        self.sync_update_state();
        self.handle_close_request(ui.ctx());
        if self.close_after_save && self.job.is_none() && !self.project_dirty {
            self.close_after_save = false;
            self.allow_close_once = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.ui_toolbar(ui));'''
replace_once(old_ui_start, new_ui_start, "App close interception")

old_windows = '''        self.ui_settings_window(ui.ctx());
        self.ui_about_window(ui.ctx());
        self.ui_logs_window(ui.ctx());

        self.start_render_if_needed(ui.ctx());'''
new_windows = '''        self.ui_settings_window(ui.ctx());
        self.ui_about_window(ui.ctx());
        self.ui_logs_window(ui.ctx());
        self.ui_snapshot_discard_confirmation(ui.ctx());
        self.ui_close_confirmation(ui.ctx());

        self.start_render_if_needed(ui.ctx());'''
replace_once(old_windows, new_windows, "confirmation windows")

# Helper functions are outside ShadeApp so they can be reused/tested without UI state.
helper_anchor = "fn build_project_file_metadata(\n"
helpers = r'''fn physical_dimensions_cm(width_px: u32, height_px: u32, dpi: dpi::DpiInfo) -> (f64, f64) {
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

'''
replace_once(helper_anchor, helpers + helper_anchor, "physical dimensions and duplicate helpers")

APP.write_text(app, encoding="utf-8")

cargo = CARGO.read_text(encoding="utf-8")
if 'version = "0.8.0"' not in cargo:
    raise SystemExit("Cargo version anchor not found")
cargo = cargo.replace('version = "0.8.0"', 'version = "0.8.1"', 1)
CARGO.write_text(cargo, encoding="utf-8")

notes = NOTES.read_text(encoding="utf-8")
release = '''# Shade Editor 0.8.1

Physical Face dimensions and safeguards against accidental state loss.

- File information now shows physical dimensions in centimeters between bit depth and pixel dimensions, calculated from the TIFF pixel size and effective DPI.
- Duplicate TIFF references remain allowed but every duplicated Face row is highlighted and marked with its duplicate count.
- Closing the application with unsaved project changes now offers Save and exit, Discard and exit, or Stay. Save and exit waits for the existing background Save job to complete successfully before closing.
- Switching away from an active Snapshot with edits that have not been written back using Update now asks whether to Stay editing or Discard changes and switch.
- `.shade` schema remains v8.

'''
NOTES.write_text(release + notes, encoding="utf-8")
