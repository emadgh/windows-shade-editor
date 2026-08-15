from pathlib import Path
import re


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


root = Path(__file__).resolve().parents[2]
main_path = root / "src" / "main.rs"
model_path = root / "src" / "model.rs"
workflow_path = root / "src" / "workflow.rs"
main = main_path.read_text(encoding="utf-8")
model = model_path.read_text(encoding="utf-8")
workflow = workflow_path.read_text(encoding="utf-8")

old_face = '''#[derive(Clone, Debug, Serialize, Deserialize)]\npub struct FaceRef {\n    pub path: String,\n    pub label: String,\n}\n'''
new_face = '''#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]\n#[serde(rename_all = "snake_case")]\npub enum FaceStatus {\n    #[default]\n    Accepted,\n    Rejected,\n}\n\nimpl FaceStatus {\n    pub fn is_rejected(self) -> bool {\n        self == Self::Rejected\n    }\n}\n\n#[derive(Clone, Debug, Serialize, Deserialize)]\npub struct FaceRef {\n    pub path: String,\n    pub label: String,\n    #[serde(default)]\n    pub status: FaceStatus,\n}\n'''
model = replace_once(model, old_face, new_face, "FaceRef model")
model += r'''

#[cfg(test)]
mod face_status_tests {
    use super::*;

    #[test]
    fn legacy_face_without_status_defaults_to_accepted() {
        let face: FaceRef = serde_json::from_str(r#"{"path":"face.tif","label":"Face 1"}"#)
            .expect("legacy FaceRef should deserialize");
        assert_eq!(face.status, FaceStatus::Accepted);
    }

    #[test]
    fn rejected_status_round_trips() {
        let face = FaceRef {
            path: "face.tif".to_owned(),
            label: "Face 1".to_owned(),
            status: FaceStatus::Rejected,
        };
        let json = serde_json::to_string(&face).unwrap();
        let loaded: FaceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.status, FaceStatus::Rejected);
    }
}
'''
model_path.write_text(model, encoding="utf-8")

old_construct = '''                    self.project.faces.push(model::FaceRef {\n                        path: item.path.to_string_lossy().into_owned(),\n                        label,\n                    });\n'''
new_construct = '''                    self.project.faces.push(model::FaceRef {\n                        path: item.path.to_string_lossy().into_owned(),\n                        label,\n                        status: model::FaceStatus::Accepted,\n                    });\n'''
main = replace_once(main, old_construct, new_construct, "FaceRef construction")

old_remove = '''    fn remove_current_face(&mut self) {\n        if self.job.is_some() || self.current_face >= self.faces.len() {\n            return;\n        }\n        self.snapshot_preview_cache.clear();\n        self.faces.remove(self.current_face);\n        if self.current_face < self.project.faces.len() {\n            self.project.faces.remove(self.current_face);\n        }\n        self.current_face = self.current_face.min(self.faces.len().saturating_sub(1));\n        self.selected_channel = 0;\n        self.solo_channel = None;\n        self.fit_requested = true;\n        self.viewport_recenter = true;\n        self.project_dirty = true;\n        self.report_info("Face removed from project (source TIFF was not deleted)");\n    }\n'''
new_remove = '''    fn remove_current_face(&mut self) {\n        if self.job.is_some() || self.current_face >= self.faces.len() {\n            return;\n        }\n        self.snapshot_preview_cache.clear();\n        self.faces.remove(self.current_face);\n        if self.current_face < self.project.faces.len() {\n            self.project.faces.remove(self.current_face);\n        }\n        // A preview worker may still finish with the removed Face's old index.\n        // Invalidate every surviving generation before accepting any future result\n        // so a shifted Face can never receive stale pixels from that worker.\n        self.render_busy = None;\n        for face in &mut self.faces {\n            face.generation = face.generation.wrapping_add(1).max(1);\n            face.rendered_generation = 0;\n        }\n        self.current_face = self.current_face.min(self.faces.len().saturating_sub(1));\n        self.selected_channel = 0;\n        self.solo_channel = None;\n        self.fit_requested = true;\n        self.viewport_recenter = true;\n        self.project_dirty = true;\n        self.report_info("Face removed from project (source TIFF was not deleted)");\n    }\n'''
main = replace_once(main, old_remove, new_remove, "remove_current_face stale render guard")

# Confirm explicit single-Face export when the current Face was rejected.
export_anchor = '''        if !workflow::active_face_available(self) {\n            self.report_error(\n                "The active Face source TIFF is missing. Relink it before exporting.",\n            );\n            return;\n        }\n        let Some(face) = self.faces.get(self.current_face) else {\n'''
export_new = '''        if !workflow::active_face_available(self) {\n            self.report_error(\n                "The active Face source TIFF is missing. Relink it before exporting.",\n            );\n            return;\n        }\n        if self\n            .project\n            .faces\n            .get(self.current_face)\n            .is_some_and(|face| face.status.is_rejected())\n        {\n            let answer = rfd::MessageDialog::new()\n                .set_title("Export rejected Face?")\n                .set_description(\n                    "This Face is marked Rejected and is normally excluded from production output. Export this Face anyway?",\n                )\n                .set_buttons(rfd::MessageButtons::YesNo)\n                .set_level(rfd::MessageLevel::Warning)\n                .show();\n            if answer != rfd::MessageDialogResult::Yes {\n                return;\n            }\n        }\n        let Some(face) = self.faces.get(self.current_face) else {\n'''
main = replace_once(main, export_anchor, export_new, "rejected single export confirmation")

# Export All only requires available sources for Accepted Faces.
old_dialog_check = '''        if self.faces.iter().any(|face| !face.available) {\n            self.report_error("Export all requires every Face source TIFF to be available. Relink missing Faces first.");\n            return;\n        }\n'''
new_dialog_check = '''        let accepted_count = self\n            .project\n            .faces\n            .iter()\n            .filter(|face| !face.status.is_rejected())\n            .count();\n        if accepted_count == 0 {\n            self.report_error("Export all has no Accepted Faces. Re-accept at least one Face first.");\n            return;\n        }\n        if self.faces.iter().enumerate().any(|(index, face)| {\n            !self\n                .project\n                .faces\n                .get(index)\n                .is_some_and(|item| item.status.is_rejected())\n                && !face.available\n        }) {\n            self.report_error("Export all requires every Accepted Face source TIFF to be available. Relink missing Accepted Faces first.");\n            return;\n        }\n'''
if main.count(old_dialog_check) < 2:
    raise SystemExit("expected Export All availability guard at least twice")
main = main.replace(old_dialog_check, new_dialog_check, 2)

old_sources = '''        let sources = self\n            .faces\n            .iter()\n            .map(|face| face.path.clone())\n            .collect::<Vec<_>>();\n        let face_names = self\n            .project\n            .faces\n            .iter()\n            .map(|face| face.label.clone())\n            .collect::<Vec<_>>();\n'''
new_sources = '''        let rejected_count = self\n            .project\n            .faces\n            .iter()\n            .filter(|face| face.status.is_rejected())\n            .count();\n        let export_faces = self\n            .faces\n            .iter()\n            .enumerate()\n            .filter_map(|(index, face)| {\n                let project_face = self.project.faces.get(index)?;\n                (!project_face.status.is_rejected()).then(|| {\n                    (index, face.path.clone(), project_face.label.clone())\n                })\n            })\n            .collect::<Vec<_>>();\n'''
main = replace_once(main, old_sources, new_sources, "Export All accepted face list")

old_loop = '''        for (index, source) in sources.iter().enumerate() {\n            let face_name = face_names\n                .get(index)\n                .map(String::as_str)\n                .filter(|name| !name.trim().is_empty())\n                .or_else(|| source.file_stem().and_then(|value| value.to_str()))\n                .unwrap_or("face");\n'''
new_loop = '''        for (original_index, source, configured_name) in &export_faces {\n            let face_name = (!configured_name.trim().is_empty())\n                .then_some(configured_name.as_str())\n                .or_else(|| source.file_stem().and_then(|value| value.to_str()))\n                .unwrap_or("face");\n'''
main = replace_once(main, old_loop, new_loop, "Export All loop")
main = replace_once(main, "                face_number: index + 1,", "                face_number: original_index + 1,", "Export All face number")

old_report = '''        if queued > 0 {\n            self.report_info(if skipped > 0 {\n                format!("Queued {queued} export(s) · skipped {skipped} existing file(s)")\n            } else {\n                format!("Queued {queued} export(s)")\n            });\n        } else if skipped > 0 {\n            self.report_info(format!(\n                "No exports queued · skipped {skipped} existing file(s)"\n            ));\n        }\n'''
new_report = '''        if queued > 0 {\n            let mut parts = vec![format!("Queued {queued} export(s)")];\n            if rejected_count > 0 {\n                parts.push(format!("excluded {rejected_count} Rejected Face(s)"));\n            }\n            if skipped > 0 {\n                parts.push(format!("skipped {skipped} existing file(s)"));\n            }\n            self.report_info(parts.join(" · "));\n        } else {\n            let mut parts = vec!["No exports queued".to_owned()];\n            if rejected_count > 0 {\n                parts.push(format!("excluded {rejected_count} Rejected Face(s)"));\n            }\n            if skipped > 0 {\n                parts.push(format!("skipped {skipped} existing file(s)"));\n            }\n            self.report_info(parts.join(" · "));\n        }\n'''
main = replace_once(main, old_report, new_report, "Export All summary")

# Extend the common row primitive with an optional persistent tint for Rejected Faces.
click_pattern = re.compile(r'''fn clickable_row\(\n    ui: &mut egui::Ui,\n    selected: bool,\n    left: &str,\n    trailing: Option<&str>,\n    accent: Option<egui::Color32>,\n    height: f32,\n\) -> egui::Response \{.*?\n\}\n\nfn clipping_warning_color''', re.S)
click_new = r'''fn clickable_row(
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

fn clipping_warning_color'''
main, count = click_pattern.subn(click_new, main, count=1)
if count != 1:
    raise SystemExit(f"clickable_row: expected one match, found {count}")
main_path.write_text(main, encoding="utf-8")

faces_pattern = re.compile(r'''pub\(super\) fn ui_faces\(app: &mut ShadeApp, ui: &mut egui::Ui\) \{.*?\n\}\n\npub\(super\) fn ui_missing_viewport''', re.S)
faces_new = r'''pub(super) fn ui_faces(app: &mut ShadeApp, ui: &mut egui::Ui) {
    ui.label("Project title");
    if ui
        .add(
            egui::TextEdit::singleline(&mut app.project.name)
                .hint_text("Uses the .shade filename after first save")
                .desired_width(f32::INFINITY),
        )
        .changed()
    {
        app.project_dirty = true;
    }
    ui.add_space(4.0);
    ui.separator();
    ui.heading("Faces");

    let active_rejected = app
        .project
        .faces
        .get(app.current_face)
        .is_some_and(|face| face.status.is_rejected());
    if active_rejected {
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(175, 35, 35, 56))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(225, 80, 80)))
            .corner_radius(4)
            .inner_margin(7)
            .show(ui, |ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(245, 120, 120),
                    "REJECTED FACE",
                );
                ui.small("This Face was rejected because of a design/problem decision. It is kept for reference and Export All will skip it.");
            });
        ui.add_space(5.0);
    }

    if app.faces.is_empty() {
        ui.label("Add TIFF files to create a shade project.");
    } else {
        let duplicate_counts = duplicate_face_counts(&app.faces);
        let mut display_indices = (0..app.faces.len()).collect::<Vec<_>>();
        display_indices.sort_by_key(|index| {
            app.project
                .faces
                .get(*index)
                .is_some_and(|face| face.status.is_rejected())
        });

        let mut requested_face = None;
        let mut requested_status = None;
        let mut requested_delete = None;
        for index in display_indices {
            let face = &app.faces[index];
            let project_face = app.project.faces.get(index);
            let status = project_face.map(|item| item.status).unwrap_or_default();
            let label = project_face
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
            let mut display_label = label.to_owned();
            if status.is_rejected() {
                display_label.push_str("  [rejected]");
            }
            if !face.available {
                display_label.push_str("  [missing]");
            }
            if duplicate_count > 1 {
                display_label.push_str(&format!("  [duplicate x{duplicate_count}]"));
            }
            let accent = if status.is_rejected() || !face.available {
                Some(egui::Color32::from_rgb(235, 95, 95))
            } else if duplicate_count > 1 {
                Some(egui::Color32::from_rgb(235, 155, 70))
            } else {
                None
            };
            let tint = status
                .is_rejected()
                .then_some(egui::Color32::from_rgba_unmultiplied(180, 35, 35, 52));
            let hover = if status.is_rejected() {
                format!(
                    "Rejected Face — retained for reference and excluded from Export All. Source: {}",
                    face.path.display()
                )
            } else if !face.available {
                format!(
                    "Source TIFF is missing: {}. Use Locate file or Locate folder.",
                    face.path.display()
                )
            } else if duplicate_count > 1 {
                "This TIFF is referenced more than once in the Faces list.".to_owned()
            } else {
                face.path.display().to_string()
            };
            let response = clickable_row_tinted(
                ui,
                app.current_face == index,
                &display_label,
                None,
                accent,
                tint,
                32.0,
            )
            .on_hover_text(hover);
            if response.clicked() {
                requested_face = Some(index);
            }
            response.context_menu(|ui| {
                if status.is_rejected() {
                    if ui.button("Mark Accepted").clicked() {
                        requested_status = Some((index, model::FaceStatus::Accepted));
                        ui.close();
                    }
                } else if ui.button("Mark Rejected").clicked() {
                    requested_status = Some((index, model::FaceStatus::Rejected));
                    ui.close();
                }
                ui.separator();
                if ui.button("Delete from project").clicked() {
                    requested_delete = Some(index);
                    ui.close();
                }
            });
        }

        if let Some((index, status)) = requested_status {
            if let Some(face) = app.project.faces.get_mut(index) {
                if face.status != status {
                    face.status = status;
                    app.project_dirty = true;
                    app.report_info(match status {
                        model::FaceStatus::Accepted => "Face marked Accepted — eligible for Export All",
                        model::FaceStatus::Rejected => "Face marked Rejected — retained for reference and excluded from Export All",
                    });
                }
            }
        }
        if let Some(index) = requested_delete {
            app.current_face = index;
            app.remove_current_face();
        } else if let Some(index) = requested_face {
            app.current_face = index;
            app.selected_channel = 0;
            app.solo_channel = None;
            app.fit_requested = true;
            app.viewport_recenter = true;
            app.mark_current_preview_dirty();
            if app
                .project
                .faces
                .get(index)
                .is_some_and(|face| face.status.is_rejected())
            {
                app.report_info("Warning: selected Face is Rejected and is excluded from Export All");
            }
        }
        ui.add_space(4.0);
        let active_missing = app
            .faces
            .get(app.current_face)
            .is_some_and(|face| !face.available);
        let missing_count = app.faces.iter().filter(|face| !face.available).count();
        let mut locate_file = false;
        let mut locate_folder = false;
        let remove;
        ui.horizontal_wrapped(|ui| {
            if active_missing {
                locate_file = ui.button("Locate file").clicked();
            }
            if missing_count > 0 {
                locate_folder = ui
                    .button(format!("Locate folder ({missing_count})"))
                    .on_hover_text(
                        "Recursively find missing TIFF filenames and verify every replacement",
                    )
                    .clicked();
            }
            remove = ui.button("Remove active face").clicked();
        });
        if locate_file {
            relink_current_face_dialog(app);
        } else if locate_folder {
            relink_missing_faces_folder_dialog(app);
        } else if remove {
            app.remove_current_face();
        }
    }
    ui.separator();
    app.ui_snapshots(ui);
    ui.separator();
    app.ui_test_code(ui);
    ui.separator();
    app.ui_history(ui);
}

pub(super) fn ui_missing_viewport'''
workflow, count = faces_pattern.subn(faces_new, workflow, count=1)
if count != 1:
    raise SystemExit(f"ui_faces replacement: expected one match, found {count}")
workflow_path.write_text(workflow, encoding="utf-8")

Path(__file__).unlink()
bootstrap = root / ".github" / "workflows" / "apply-v019-face-status.yml"
if bootstrap.exists():
    bootstrap.unlink()
