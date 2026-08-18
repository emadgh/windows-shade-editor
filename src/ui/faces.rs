use super::actions::FaceUiAction;
use crate::*;
use eframe::egui;

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

pub(crate) fn ui_faces(app: &mut ShadeApp, ui: &mut egui::Ui) {
    let mut actions = Vec::new();
    ui.label("Project title");
    let mut project_name = app.project.name.clone();
    if ui
        .add(
            egui::TextEdit::singleline(&mut project_name)
                .hint_text("Uses the .shade filename after first save")
                .desired_width(f32::INFINITY),
        )
        .changed()
    {
        actions.push(FaceUiAction::RenameProject(project_name));
    }
    ui.add_space(4.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.heading("Faces");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let add_enabled = app.job.is_none() && !app.project_autosave_busy;
            if ui
                .add_enabled(add_enabled, egui::Button::new("+ Add TIFF Faces"))
                .on_hover_text("Add one or more TIFF files as Faces in this project")
                .clicked()
            {
                actions.push(FaceUiAction::AddFacesDialog);
            }
        });
    });

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
            actions.push(FaceUiAction::SetStatus { index, status });
        }
        if let Some(index) = requested_delete {
            actions.push(FaceUiAction::Delete(index));
        } else if let Some(index) = requested_face {
            actions.push(FaceUiAction::Select(index));
        }
        ui.add_space(4.0);
        let active_missing = app
            .faces
            .get(app.current_face)
            .is_some_and(|face| !face.available);
        let missing_count = app.faces.iter().filter(|face| !face.available).count();
        let mut locate_file = false;
        let mut locate_folder = false;
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
        });
        if locate_file {
            actions.push(FaceUiAction::RelinkCurrent);
        } else if locate_folder {
            actions.push(FaceUiAction::RelinkMissingFolder);
        }
    }
    ui.separator();
    app.ui_snapshots(ui);
    ui.separator();
    app.ui_test_code(ui);
    ui.separator();
    app.ui_history(ui);
    for action in actions {
        app.dispatch_face_ui_action(action);
    }
}
