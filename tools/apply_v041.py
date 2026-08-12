from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_between(text: str, start: str, end: str, replacement: str) -> str:
    i = text.index(start)
    j = text.index(end, i)
    return text[:i] + replacement.rstrip() + "\n" + text[j:]


# --- Cargo / version -------------------------------------------------------
cargo_path = ROOT / "Cargo.toml"
cargo = cargo_path.read_text(encoding="utf-8")
cargo = cargo.replace('version = "0.4.0"', 'version = "0.4.1"', 1)
if 'chrono = ' not in cargo:
    cargo = cargo.replace('fontdue = "0.9.3"\n', 'fontdue = "0.9.3"\nchrono = { version = "0.4.42", default-features = false, features = ["clock"] }\n')
cargo_path.write_text(cargo, encoding="utf-8")


# --- .shade model / snapshots --------------------------------------------
model_path = ROOT / "src/model_v4.rs"
model = model_path.read_text(encoding="utf-8")
model = model.replace('pub const SHADE_SCHEMA_VERSION: u32 = 3;', 'pub const SHADE_SCHEMA_VERSION: u32 = 4;', 1)
model = model.replace(
'''fn default_next_snapshot_id() -> u64 { 1 }\n''',
'''fn default_next_snapshot_id() -> u64 { 1 }\n\nfn now_unix_ms() -> i64 {\n    std::time::SystemTime::now()\n        .duration_since(std::time::UNIX_EPOCH)\n        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)\n        .unwrap_or(0)\n}\n''',
1,
)
model = model.replace(
'''#[derive(Clone, Debug, Serialize, Deserialize)]\npub struct AdjustmentSnapshot {\n    pub id: u64,\n    pub name: String,\n    pub adjustments: BTreeMap<String, ChannelAdjustment>,\n}\n''',
'''#[derive(Clone, Debug, Serialize, Deserialize)]\npub struct AdjustmentSnapshot {\n    pub id: u64,\n    pub name: String,\n    /// Snapshot creation time as Unix milliseconds. Older schema versions do\n    /// not have this field and deserialize it as zero.\n    #[serde(default)]\n    pub created_at_unix_ms: i64,\n    pub adjustments: BTreeMap<String, ChannelAdjustment>,\n}\n''',
1,
)

new_snapshot_methods = '''    fn next_snapshot_name(&self) -> String {
        let mut number = 1usize;
        loop {
            let candidate = format!("Test {number}");
            if self.snapshot_name_available(&candidate, None) {
                return candidate;
            }
            number += 1;
        }
    }

    pub fn snapshot_name_available(&self, candidate: &str, except_id: Option<u64>) -> bool {
        let candidate = candidate.trim();
        !candidate.is_empty()
            && self.snapshots.iter().all(|snapshot| {
                except_id == Some(snapshot.id) || !snapshot.name.trim().eq_ignore_ascii_case(candidate)
            })
    }

    pub fn create_snapshot(&mut self) -> u64 {
        let id = self.next_snapshot_id.max(1);
        self.next_snapshot_id = id.saturating_add(1);
        let name = self.next_snapshot_name();
        self.snapshots.push(AdjustmentSnapshot {
            id,
            name,
            created_at_unix_ms: now_unix_ms(),
            adjustments: self.adjustments.clone(),
        });
        self.active_snapshot_id = Some(id);
        id
    }

    pub fn rename_snapshot(&mut self, id: u64, candidate: &str) -> Result<bool, String> {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            return Err("Snapshot name cannot be empty.".to_owned());
        }
        if !self.snapshot_name_available(candidate, Some(id)) {
            return Err(format!("A snapshot named ‘{candidate}’ already exists."));
        }
        let Some(snapshot) = self.snapshots.iter_mut().find(|snapshot| snapshot.id == id) else {
            return Err("Snapshot no longer exists.".to_owned());
        };
        if snapshot.name == candidate {
            return Ok(false);
        }
        snapshot.name = candidate.to_owned();
        Ok(true)
    }
'''
model = replace_between(model, '    pub fn create_snapshot(&mut self) -> u64 {', '    pub fn apply_snapshot(&mut self, id: u64) -> bool {', new_snapshot_methods)

unique_test = '''
    #[test]
    fn snapshot_names_are_unique_and_rename_rejects_duplicates() {
        let mut project = ShadeProject::default();
        let first = project.create_snapshot();
        let second = project.create_snapshot();
        assert_eq!(project.snapshots.iter().find(|item| item.id == first).unwrap().name, "Test 1");
        assert_eq!(project.snapshots.iter().find(|item| item.id == second).unwrap().name, "Test 2");
        assert!(project.rename_snapshot(second, " test 1 ").is_err());
        assert!(project.rename_snapshot(second, "Reference").unwrap());
        assert!(project.snapshot_name_available("Test 2", None));
        assert!(project.snapshots.iter().all(|item| item.created_at_unix_ms > 0));
    }
'''
if 'snapshot_names_are_unique_and_rename_rejects_duplicates' not in model:
    pos = model.rfind('\n}')
    model = model[:pos] + unique_test + model[pos:]
model_path.write_text(model, encoding="utf-8")


# --- UI -------------------------------------------------------------------
app_path = ROOT / "src/app_main.rs"
app = app_path.read_text(encoding="utf-8")
if 'use chrono::{Local, TimeZone};' not in app:
    app = app.replace('use eframe::egui;\n', 'use chrono::{Local, TimeZone};\nuse eframe::egui;\n', 1)
app = app.replace(
'''    project_dirty: bool,\n    job: Option<JobHandle>,''',
'''    project_dirty: bool,\n    snapshot_rename_id: Option<u64>,\n    snapshot_rename_buffer: String,\n    job: Option<JobHandle>,''',
1,
)
app = app.replace(
'''            project_dirty: false,\n            job: None,''',
'''            project_dirty: false,\n            snapshot_rename_id: None,\n            snapshot_rename_buffer: String::new(),\n            job: None,''',
1,
)
app = app.replace(
'''        self.project_dirty = false;\n        self.report_info("New shade project");''',
'''        self.project_dirty = false;\n        self.snapshot_rename_id = None;\n        self.snapshot_rename_buffer.clear();\n        self.report_info("New shade project");''',
1,
)
app = app.replace(
'''                    self.project = payload.project;\n                    self.project_path = Some(payload.path.clone());''',
'''                    self.project = payload.project;\n                    self.snapshot_rename_id = None;\n                    self.snapshot_rename_buffer.clear();\n                    self.project_path = Some(payload.path.clone());''',
1,
)

ui_faces = '''    fn ui_faces(&mut self, ui: &mut egui::Ui) {
        ui.heading("Faces");
        if self.faces.is_empty() {
            ui.label("Add TIFF files to create a shade project.");
        } else {
            let mut requested_face = None;
            for (index, face) in self.faces.iter().enumerate() {
                let label = self.project.faces.get(index)
                    .map(|item| item.label.as_str())
                    .unwrap_or_else(|| face.path.file_name().and_then(|name| name.to_str()).unwrap_or("Face"));
                if clickable_row(ui, self.current_face == index, label, None, None, 32.0).clicked() {
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
            if ui.button("Remove active face").clicked() { self.remove_current_face(); }
        }

        ui.separator();
        self.ui_snapshots(ui);
        ui.separator();
        self.ui_test_code(ui);
    }
'''
app = replace_between(app, '    fn ui_faces(&mut self, ui: &mut egui::Ui) {', '    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {', ui_faces)

ui_snapshots = '''    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Snapshots");
            if ui.small_button("+ New").clicked() {
                let id = self.project.create_snapshot();
                if let Some(snapshot) = self.project.snapshots.iter().find(|snapshot| snapshot.id == id) {
                    self.snapshot_rename_id = Some(id);
                    self.snapshot_rename_buffer = snapshot.name.clone();
                }
                self.project_dirty = true;
            }
        });
        ui.small("Saved adjustment test states.");
        ui.add_space(4.0);

        let active_id = self.project.active_snapshot_id;
        let active_dirty = active_id.is_some() && !self.project.active_snapshot_matches();
        let rows = self.project.snapshots.iter()
            .map(|snapshot| (snapshot.id, snapshot.name.clone(), snapshot.created_at_unix_ms))
            .collect::<Vec<_>>();
        let mut requested_load = None;
        let mut current_day = String::new();
        for (id, name, created_at) in rows {
            let (day, time) = snapshot_day_time(created_at);
            if day != current_day {
                if !current_day.is_empty() { ui.add_space(5.0); }
                ui.strong(day.clone());
                current_day = day;
            }
            let selected = active_id == Some(id);
            let display_name = if selected && active_dirty { format!("{name}  *") } else { name };
            if clickable_row(ui, selected, &display_name, Some(&time), None, 36.0).clicked() {
                requested_load = Some(id);
            }
        }

        if let Some(id) = requested_load {
            if self.project.apply_snapshot(id) {
                if let Some(snapshot) = self.project.snapshots.iter().find(|snapshot| snapshot.id == id) {
                    self.snapshot_rename_id = Some(id);
                    self.snapshot_rename_buffer = snapshot.name.clone();
                }
                self.mark_all_previews_dirty();
                self.report_info("Snapshot loaded");
            }
        }

        let Some(active_id) = self.project.active_snapshot_id else { return; };
        let Some(active_name) = self.project.snapshots.iter()
            .find(|snapshot| snapshot.id == active_id)
            .map(|snapshot| snapshot.name.clone())
        else { return; };
        if self.snapshot_rename_id != Some(active_id) {
            self.snapshot_rename_id = Some(active_id);
            self.snapshot_rename_buffer = active_name.clone();
        }

        ui.add_space(6.0);
        ui.label("Snapshot name");
        let rename_response = ui.add(
            egui::TextEdit::singleline(&mut self.snapshot_rename_buffer)
                .desired_width(f32::INFINITY)
        );
        let enter = rename_response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if (rename_response.lost_focus() || enter) && self.snapshot_rename_buffer.trim() != active_name {
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
'''
app = replace_between(app, '    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {', '    fn ui_test_code(&mut self, ui: &mut egui::Ui) {', ui_snapshots)

ui_channels = '''    fn ui_channels_histogram(&mut self, ui: &mut egui::Ui) {
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
        if clickable_row(ui, self.solo_channel.is_none(), "Composite", None, None, 32.0).clicked() {
            self.show_composite();
        }
        ui.small(format!("{} + {} extra", color_model.title(), channel_names.len().saturating_sub(base_count)));
        ui.add_space(3.0);
        for (index, name) in channel_names.iter().enumerate() {
            let suffix = if index >= base_count { "  • spot" } else { "" };
            let accent = channel_color(name, index);
            let label = format!("●  {name}{suffix}");
            if clickable_row(ui, self.selected_channel == index, &label, None, Some(accent), 32.0).clicked() {
                self.select_channel(index, true);
            }
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
'''
app = replace_between(app, '    fn ui_channels_histogram(&mut self, ui: &mut egui::Ui) {', '    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {', ui_channels)

ui_adjustments = '''    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {
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
        let control_accent = self.settings.colorize_adjustments.then(|| channel_color(&output_name, self.selected_channel));
        let panel_accent = (self.adjustment_scope == AdjustmentScope::Selected)
            .then(|| channel_color(&output_name, self.selected_channel));

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

        let mut frame = egui::Frame::new().inner_margin(8).corner_radius(6);
        if let Some(color) = panel_accent {
            frame = frame.stroke(egui::Stroke::new(1.5, color.gamma_multiply(0.72)));
        } else {
            frame = frame.stroke(ui.visuals().widgets.noninteractive.bg_stroke);
        }
        let changed = frame.show(ui, |ui| {
            if let Some(color) = panel_accent {
                ui.visuals_mut().widgets.noninteractive.bg_stroke.color = color.gamma_multiply(0.52);
                ui.colored_label(color, format!("Editing: {output_name}"));
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
                ),
                AdjustmentScope::All => self.ui_all_adjustments(
                    ui,
                    &output_name,
                    &channel_names,
                    active_histogram.as_ref(),
                    control_accent,
                ),
            }
        }).inner;
        if changed { self.mark_all_previews_dirty(); }
    }
'''
app = replace_between(app, '    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {', '    fn ui_selected_adjustment(', ui_adjustments)

mixer = '''fn mixer_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    output_name: &str,
    channel_names: &[String],
    accent: Option<egui::Color32>,
) -> bool {
    with_accent(ui, accent, |ui| {
        if let Some(color) = accent {
            ui.colored_label(color, format!("Output: {output_name}"));
        } else {
            ui.label(format!("Output: {output_name}"));
        }
        let mut changed = false;
        for (index, name) in channel_names.iter().enumerate() {
            let default = if name == output_name { 1.0 } else { 0.0 };
            let coefficient = adjustment.mixer.coefficients.entry(name.clone()).or_insert(default);
            let row_accent = accent.map(|_| channel_color(name, index));
            changed |= with_accent(ui, row_accent, |ui| {
                let mut slider = egui::Slider::new(coefficient, -2.0..=2.0).text(name).trailing_fill(true);
                if let Some(color) = row_accent {
                    slider = slider.text_color(color);
                }
                ui.add(slider).changed()
            });
        }
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
                adjustment.mixer.coefficients.insert(name.clone(), if name == output_name { 1.0 } else { 0.0 });
            }
            adjustment.mixer.constant = 0.0;
            changed = true;
        }
        changed
    })
}
'''
app = replace_between(app, 'fn mixer_ui(', 'fn broadcast_levels_ui(', mixer)

helpers = '''fn clickable_row(
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
        if selected { visuals.selection.stroke.color } else { visuals.text_color() }
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

fn snapshot_day_time(created_at_unix_ms: i64) -> (String, String) {
    if created_at_unix_ms <= 0 {
        return ("Earlier snapshots".to_owned(), "—".to_owned());
    }
    match Local.timestamp_millis_opt(created_at_unix_ms).single() {
        Some(value) => (value.format("%Y-%m-%d").to_string(), value.format("%H:%M").to_string()),
        None => ("Earlier snapshots".to_owned(), "—".to_owned()),
    }
}

'''
if 'fn clickable_row(' not in app:
    app = app.replace('fn draw_histogram(\n', helpers + 'fn draw_histogram(\n', 1)

app_path.write_text(app, encoding="utf-8")


# --- Release notes ---------------------------------------------------------
release_path = ROOT / "RELEASE_NOTES.md"
release_path.write_text('''# Shade Editor 0.4.1\n\nSnapshot organization and channel-color UX refinement.\n\n## Added / changed\n\n- Snapshots now store creation timestamps in `.shade` schema v4.\n- Snapshot list is grouped by local calendar day; each row shows its creation time.\n- Snapshot, Face and Channel rows are full-width click targets with larger row heights.\n- New snapshots automatically receive a unique `Test N` name. Rename validation rejects empty or duplicate names case-insensitively.\n- Legacy snapshots created before schema v4 remain compatible and appear under `Earlier snapshots` because their original creation time was never stored.\n- Channel Mixer source rows use each source channel's own accent for slider controls and labels when adjustment colorization is enabled.\n- The selected-channel Adjustment panel gets a subtle border and internal group-border tint matching the active channel, making the current separation visually explicit.\n\n## Retained\n\n- Multi-channel CMYK/RGB + Photoshop Spot support.\n- Snapshots, all-channel adjustments, DPI-aware Tahoma test code, Fit viewport, background preview rendering, operation progress, updater progress and application logs from 0.4.0.\n''', encoding="utf-8")

print("v0.4.1 source patch applied")
