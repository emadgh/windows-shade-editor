from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing replacement marker: {label}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, new: str, label: str) -> str:
    a = text.find(start)
    if a < 0:
        raise SystemExit(f"missing start marker: {label}")
    b = text.find(end, a)
    if b < 0:
        raise SystemExit(f"missing end marker: {label}")
    return text[:a] + new + text[b:]


# ---------------- model v5 ----------------
model = (ROOT / "src/model_v4.rs").read_text(encoding="utf-8")
model = replace_once(model, "pub const SHADE_SCHEMA_VERSION: u32 = 4;", "pub const SHADE_SCHEMA_VERSION: u32 = 5;", "schema")

old_curve = '''#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Curve {
    /// Output at input 0.0.
    pub black: f32,
    /// Relative midpoint position inside the current [black, white] output range.
    /// 0.5 is always a straight line between the two endpoints.
    pub midpoint: f32,
    /// Output at input 1.0.
    pub white: f32,
}

impl Default for Curve {
    fn default() -> Self {
        Self {
            black: 0.0,
            midpoint: 0.5,
            white: 1.0,
        }
    }
}
'''
new_curve = '''#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Curve {
    /// Input position that maps to the black output endpoint.
    pub input_black: f32,
    /// Input position that maps to the white output endpoint.
    pub input_white: f32,
    /// Output at the black input endpoint.
    pub black: f32,
    /// Relative midpoint position inside the current [black, white] output range.
    /// 0.5 is always a straight line between the two endpoints.
    pub midpoint: f32,
    /// Output at the white input endpoint.
    pub white: f32,
}

impl Default for Curve {
    fn default() -> Self {
        Self {
            input_black: 0.0,
            input_white: 1.0,
            black: 0.0,
            midpoint: 0.5,
            white: 1.0,
        }
    }
}
'''
model = replace_once(model, old_curve, new_curve, "curve struct")

old_snapshot = '''#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdjustmentSnapshot {
    pub id: u64,
    pub name: String,
    /// Snapshot creation time as Unix milliseconds. Older schema versions do
    /// not have this field and deserialize it as zero.
    #[serde(default)]
    pub created_at_unix_ms: i64,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
}
'''
new_snapshot = '''#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotExportRecord {
    pub face_key: String,
    pub folder: String,
    pub exported_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdjustmentSnapshot {
    pub id: u64,
    pub name: String,
    /// Snapshot creation time as Unix milliseconds. Older schema versions do
    /// not have this field and deserialize it as zero.
    #[serde(default)]
    pub created_at_unix_ms: i64,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
    /// Latest successful export per source Face. This is UI history only and
    /// never prevents another export.
    #[serde(default)]
    pub exports: Vec<SnapshotExportRecord>,
}
'''
model = replace_once(model, old_snapshot, new_snapshot, "snapshot export record")
model = replace_once(
    model,
    '''            created_at_unix_ms: now_unix_ms(),
            adjustments: self.adjustments.clone(),
        });''',
    '''            created_at_unix_ms: now_unix_ms(),
            adjustments: self.adjustments.clone(),
            exports: Vec::new(),
        });''',
    "snapshot create exports",
)

old_effective_end = '''    pub fn effective_test_code_text(&self) -> String {
        let explicit = self.test_code.text.trim();
        if !explicit.is_empty() {
            return explicit.to_owned();
        }
        self.active_snapshot_name()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("Test")
            .to_owned()
    }
}
'''
new_effective_end = '''    pub fn effective_test_code_text(&self) -> String {
        let explicit = self.test_code.text.trim();
        if !explicit.is_empty() {
            return explicit.to_owned();
        }
        self.active_snapshot_name()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("Test")
            .to_owned()
    }

    pub fn record_snapshot_export(
        &mut self,
        id: u64,
        face_key: String,
        folder: String,
        exported_at_unix_ms: i64,
    ) -> bool {
        let Some(snapshot) = self.snapshots.iter_mut().find(|snapshot| snapshot.id == id) else {
            return false;
        };
        snapshot.exports.retain(|record| record.face_key != face_key);
        snapshot.exports.push(SnapshotExportRecord {
            face_key,
            folder,
            exported_at_unix_ms,
        });
        true
    }

    pub fn snapshot_export_for_face(
        &self,
        id: u64,
        face_key: &str,
    ) -> Option<&SnapshotExportRecord> {
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)?
            .exports
            .iter()
            .filter(|record| record.face_key == face_key)
            .max_by_key(|record| record.exported_at_unix_ms)
    }
}
'''
model = replace_once(model, old_effective_end, new_effective_end, "snapshot export methods")

old_apply_curve = '''pub fn apply_curve(value: f32, curve: Curve) -> f32 {
    let x = value.clamp(0.0, 1.0);
    let midpoint_output = curve_mid_output(curve);
    let y = if x <= 0.5 {
        lerp(curve.black, midpoint_output, x * 2.0)
    } else {
        lerp(midpoint_output, curve.white, (x - 0.5) * 2.0)
    };
    y.clamp(0.0, 1.0)
}
'''
new_apply_curve = '''pub fn apply_curve(value: f32, curve: Curve) -> f32 {
    let input_black = curve.input_black.clamp(0.0, 0.9999);
    let input_white = curve.input_white.clamp(input_black + 0.0001, 1.0);
    let x = ((value - input_black) / (input_white - input_black)).clamp(0.0, 1.0);
    let midpoint_output = curve_mid_output(curve);
    let y = if x <= 0.5 {
        lerp(curve.black, midpoint_output, x * 2.0)
    } else {
        lerp(midpoint_output, curve.white, (x - 0.5) * 2.0)
    };
    y.clamp(0.0, 1.0)
}
'''
model = replace_once(model, old_apply_curve, new_apply_curve, "apply curve input endpoints")

# Existing Curve literals in tests need the new defaulted fields.
model = model.replace(
    '''        let curve = Curve {
            black: 0.2,
            midpoint: 0.5,
            white: 0.8,
        };''',
    '''        let curve = Curve {
            black: 0.2,
            midpoint: 0.5,
            white: 0.8,
            ..Curve::default()
        };''',
)

insert_test_marker = '''    #[test]
    fn levels_gamma_is_relative_to_output_range() {'''
new_curve_input_test = '''    #[test]
    fn curve_input_endpoints_define_the_active_input_range() {
        let curve = Curve {
            input_black: 0.2,
            input_white: 0.8,
            ..Curve::default()
        };
        assert!((apply_curve(0.2, curve) - 0.0).abs() < 0.0001);
        assert!((apply_curve(0.5, curve) - 0.5).abs() < 0.0001);
        assert!((apply_curve(0.8, curve) - 1.0).abs() < 0.0001);
        assert!((apply_curve(0.0, curve) - 0.0).abs() < 0.0001);
        assert!((apply_curve(1.0, curve) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn levels_gamma_is_relative_to_output_range() {'''
model = replace_once(model, insert_test_marker, new_curve_input_test, "curve input test")

end_test_marker = '''    #[test]
    fn snapshot_names_are_unique_and_rename_rejects_duplicates() {'''
export_test = '''    #[test]
    fn snapshot_export_history_is_per_face_and_replaceable() {
        let mut project = ShadeProject::default();
        let id = project.create_snapshot();
        assert!(project.record_snapshot_export(
            id,
            "face-a.tif".to_owned(),
            r"C:\\exports\\one".to_owned(),
            100,
        ));
        assert_eq!(
            project.snapshot_export_for_face(id, "face-a.tif").unwrap().folder,
            r"C:\\exports\\one"
        );
        assert!(project.record_snapshot_export(
            id,
            "face-a.tif".to_owned(),
            r"C:\\exports\\two".to_owned(),
            200,
        ));
        let record = project.snapshot_export_for_face(id, "face-a.tif").unwrap();
        assert_eq!(record.folder, r"C:\\exports\\two");
        assert_eq!(record.exported_at_unix_ms, 200);
        assert_eq!(project.snapshots[0].exports.len(), 1);
    }

    #[test]
    fn snapshot_names_are_unique_and_rename_rejects_duplicates() {'''
model = replace_once(model, end_test_marker, export_test, "snapshot export test")

(ROOT / "src/model_v5.rs").write_text(model, encoding="utf-8")

# ---------------- app main ----------------
app_path = ROOT / "src/app_main.rs"
app = app_path.read_text(encoding="utf-8")
app = replace_once(app, '#[path = "model_v4.rs"]', '#[path = "model_v5.rs"]', "model path")
app = replace_once(app, '    Export(Result<String, String>),', '    Export(SnapshotExportBatchResult),', "job result export")

render_marker = '''struct RenderResult {
'''
export_structs = '''struct SnapshotExportMark {
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
'''
app = replace_once(app, render_marker, export_structs, "export result structs")

new_export_functions = r'''    fn export_current_dialog(&mut self) {
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
        self.launch_job("Exporting TIFF", move |progress| {
            let result = export::export_face_with_progress(
                &source,
                &destination,
                &project,
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
        project.adjustments = snapshot.adjustments.clone();
        project.active_snapshot_id = Some(snapshot.id);
        self.launch_job("Exporting snapshot", move |progress| {
            let result = export::export_face_with_progress(
                &source,
                &destination,
                &project,
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

    fn open_export_folder(&mut self, folder: &str) {
        if let Err(err) = open_folder(Path::new(folder)) {
            self.report_error(err);
        }
    }

'''
app = replace_between(
    app,
    '    fn export_current_dialog(&mut self) {',
    '    fn poll_job(&mut self) {',
    new_export_functions,
    "export functions",
)

old_poll_export = '''            JobResult::Export(result) => match result {
                Ok(message) => self.report_info(message),
                Err(err) => self.report_error(format!("Export failed: {err}")),
            },'''
new_poll_export = '''            JobResult::Export(payload) => {
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
            },'''
app = replace_once(app, old_poll_export, new_poll_export, "poll export")

new_snapshots_ui = r'''    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {
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
                    egui::Button::new("⇧").small(),
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
        ui.small("Saved adjustment test states. Export always remains available after a successful run.");
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
            if !ui.available_rect_before_wrap().is_negative() {
                ui.add_space(2.0);
            }
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
                        egui::Button::new("⇧").small(),
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
'''
app = replace_between(app, '    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {', '    fn ui_test_code(&mut self, ui: &mut egui::Ui) {', new_snapshots_ui, "snapshots ui")

# Adjustment UI: add all histograms and use them for all-channel curves.
app = replace_once(
    app,
    '''        let active_histogram = face
            .adjusted
            .get(self.selected_channel)
            .map(|values| render::histogram(values));''',
    '''        let all_adjusted_histograms = face
            .adjusted
            .iter()
            .map(|values| render::histogram(values))
            .collect::<Vec<_>>();
        let active_histogram = all_adjusted_histograms.get(self.selected_channel).copied();''',
    "all adjusted histograms",
)
app = replace_once(
    app,
    '''                        &channel_names,
                        active_histogram.as_ref(),
                        control_accent,
                    ),''',
    '''                        &channel_names,
                        &all_adjusted_histograms,
                        control_accent,
                    ),''',
    "all adjustments histogram arg",
)

new_selected_adjustment = r'''    fn ui_selected_adjustment(
        &mut self,
        ui: &mut egui::Ui,
        output_name: &str,
        channel_names: &[String],
        histogram: Option<&[u32; 256]>,
        accent: Option<egui::Color32>,
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
                        mixer_ui(ui, adjustment, output_name, channel_names, accent)
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
                        changed |= mixer_ui(ui, adjustment, output_name, channel_names, accent);
                    });
            }
        });
        changed
    }

'''
app = replace_between(app, '    fn ui_selected_adjustment(', '    fn ui_all_adjustments(', new_selected_adjustment, "selected adjustment collapse")

new_all_adjustment = r'''    fn ui_all_adjustments(
        &mut self,
        ui: &mut egui::Ui,
        template_name: &str,
        channel_names: &[String],
        histograms: &[[u32; 256]],
        accent: Option<egui::Color32>,
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
                ),
                ToolPanel::Mixer => all_mixers_ui(
                    ui,
                    &mut self.project.adjustments,
                    channel_names,
                    self.settings.colorize_adjustments,
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
                    );
                });
        }
        changed
    }

'''
app = replace_between(app, '    fn ui_all_adjustments(', '    fn ui_tools(&mut self, ui: &mut egui::Ui) {', new_all_adjustment, "all adjustment curves")

new_curves_ui = r'''fn curves_ui(
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

'''
app = replace_between(app, 'fn curves_ui(', 'fn mixer_ui(', new_curves_ui, "curve input ui")

# Replace broadcast curve helper with broadcast + per-channel full Curve foldouts.
new_curve_helpers = r'''fn broadcast_curves_ui(
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
) -> bool {
    let mut changed = false;
    let template_index = channel_names
        .iter()
        .position(|name| name == template_name)
        .unwrap_or(0);
    let broadcast_accent = colorize.then(|| channel_color(template_name, template_index));
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
        let accent = colorize.then(|| channel_color(name, index));
        let title = if let Some(color) = accent {
            egui::RichText::new(format!("●  {name}")).color(color)
        } else {
            egui::RichText::new(name)
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

'''
app = replace_between(app, 'fn broadcast_curves_ui(', 'fn all_mixers_ui(', new_curve_helpers, "all curves helper")

# Add compact Snapshot export row helper before snapshot_day_time.
snapshot_helper = r'''fn snapshot_row_with_actions(
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
            egui::Button::new("⇧").frame(false).sense(egui::Sense::click()),
        )
        .on_hover_text("Export this snapshot for the active Face")
        .clicked()
        && export_enabled;
    let folder_clicked = if exported {
        ui.put(
            check_rect,
            egui::Button::new("✓").frame(false).sense(egui::Sense::click()),
        )
        .on_hover_text("Open export folder")
        .clicked()
    } else {
        false
    };
    (row_response, export_clicked, folder_clicked)
}

'''
app = replace_once(app, 'fn snapshot_day_time(created_at_unix_ms: i64) -> (String, String) {', snapshot_helper + 'fn snapshot_day_time(created_at_unix_ms: i64) -> (String, String) {', "snapshot row helper")

# Add filesystem helpers before sanitize_filename.
fs_helpers = r'''fn unix_ms_now() -> i64 {
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

'''
app = replace_once(app, 'fn sanitize_filename(value: &str) -> String {', fs_helpers + 'fn sanitize_filename(value: &str) -> String {', "filesystem helpers")

# Version and release notes.
cargo_path = ROOT / "Cargo.toml"
cargo = cargo_path.read_text(encoding="utf-8")
cargo = replace_once(cargo, 'version = "0.4.2"', 'version = "0.5.0"', "cargo version")
cargo_path.write_text(cargo, encoding="utf-8")

notes = '''# Shade Editor 0.5.0

Snapshot export workflow and expanded Curve controls.

## Snapshot export

- Compact export actions are available for every Snapshot, every day group, and the whole Snapshots panel.
- Snapshot export always targets the active Face and reuses the exact same TIFF/Test Code backend as Export face.
- A single Snapshot uses a Save dialog; day/all exports use a destination folder and export every selected Snapshot there.
- Successful exports are marked with a check. Clicking the check opens the latest export folder.
- Export history is stored per Snapshot + Face in `.shade` schema v5, but never locks or disables re-export.

## Curve

- Curve now has Input black and Input white endpoints in addition to output endpoints and relative midpoint.
- In All channels, Broadcast remains available and copies its Curve to every channel.
- Each channel also has its own collapsed full Curve panel for independent refinement after Broadcast.
- With four channels the Curve section therefore shows one Broadcast Curve plus four channel foldouts.

## Adjustment layout

- In Stacked mode, Levels, Curve and Channel Mixer can each be collapsed/expanded independently.
'''
(ROOT / "RELEASE_NOTES.md").write_text(notes, encoding="utf-8")

app_path.write_text(app, encoding="utf-8")
