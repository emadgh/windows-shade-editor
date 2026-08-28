use super::actions::FaceUiAction;
use crate::*;
use eframe::egui;
use windows_shade_editor::file_observer::{self, ExternalFileRole};

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

fn verify_changed_face_metadata(
    preview: &runtime_preview::RuntimePreview,
    expected: Option<&model::FaceFileMetadata>,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let mut bad = Vec::new();
    let (width, height) = preview.source_dimensions();
    if (width, height) != (expected.width, expected.height) {
        bad.push(format!(
            "dimensions expected {}x{}, got {}x{}",
            expected.width, expected.height, width, height
        ));
    }
    if preview.bit_depth() != expected.bit_depth {
        bad.push(format!(
            "bit depth expected {}, got {}",
            expected.bit_depth,
            preview.bit_depth()
        ));
    }
    if preview.color_model().title() != expected.color_model {
        bad.push(format!(
            "color model expected {}, got {}",
            expected.color_model,
            preview.color_model().title()
        ));
    }
    if preview.channel_count() != expected.channel_count
        || preview.base_channel_count() != expected.base_channel_count
    {
        bad.push(format!(
            "channel layout expected {}/{} base, got {}/{} base",
            expected.channel_count,
            expected.base_channel_count,
            preview.channel_count(),
            preview.base_channel_count()
        ));
    }
    if preview.channel_names() != expected.channel_names.as_slice() {
        bad.push(format!(
            "channel names/order expected {:?}, got {:?}",
            expected.channel_names,
            preview.channel_names()
        ));
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Changed source does not match the accepted Face metadata: {}",
            bad.join("; ")
        ))
    }
}

fn reload_changed_current_face(app: &mut ShadeApp) {
    if app.job.is_some() {
        return;
    }
    let index = app.current_face;
    let Some(face) = app.faces.get(index) else {
        return;
    };
    let path = face.path.clone();
    let observed = file_observer::rescan(&path)
        .unwrap_or_else(|| file_observer::observe(&path, ExternalFileRole::Face));
    if !observed.is_available() || !observed.is_changed() {
        return;
    }
    let expected = app
        .project
        .file_metadata
        .as_ref()
        .and_then(|metadata| metadata.faces.get(index))
        .cloned();
    let max_dimension = app.settings.max_preview_dimension;
    let default_dpi = app.settings.default_dpi;
    app.launch_job("Reloading changed Face", move |progress| {
        ShadeApp::set_progress(
            &progress,
            Some(0.15),
            "Reloading changed Face",
            "Loading current source bytes",
        );
        let result = (|| -> Result<LoadedFace, String> {
            let preview = runtime_preview::RuntimePreview::load(&path, max_dimension)
                .map_err(|err| format!("Cannot reload changed source {}: {err}", path.display()))?;
            verify_changed_face_metadata(&preview, expected.as_ref())?;
            let item = LoadedFace {
                dpi: dpi::read_dpi(&path, default_dpi),
                path: path.clone(),
                available: true,
                preview,
            };
            // The job remains active until this result is applied, so acknowledging here
            // cannot open an export window with the old preview still interactive.
            file_observer::acknowledge(&path).ok_or_else(|| {
                format!(
                    "Changed Face was verified but its observer baseline disappeared before acceptance: {}",
                    path.display()
                )
            })?;
            Ok(item)
        })();
        ShadeApp::set_progress(
            &progress,
            Some(1.0),
            "Reloading changed Face",
            "Complete",
        );
        JobResult::RelinkFace { index, result }
    });
}

pub(crate) fn ui_faces(app: &mut ShadeApp, ui: &mut egui::Ui) {
    let mut actions = Vec::new();

    // Availability is synchronized from the shared observer. Recreated/modified files are
    // intentionally not auto-reloaded: the current RuntimePreview remains authoritative until
    // an explicit verified reload/relink accepts new source bytes.
    for face in &mut app.faces {
        let observed = file_observer::observe(&face.path, ExternalFileRole::Face);
        if !observed.is_available() {
            face.available = false;
        }
    }

    egui::CollapsingHeader::new("Faces")
        .id_salt("workspace-faces-panel")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.small("Project");
                let mut project_name = app.project.name.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut project_name)
                            .hint_text("Project title")
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    actions.push(FaceUiAction::RenameProject(project_name));
                }
            });
            ui.horizontal(|ui| {
                ui.small(format!("{} Face(s)", app.faces.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let add_enabled = app.job.is_none();
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
                        ui.small("Retained for reference and excluded from Export All.");
                    });
                ui.add_space(4.0);
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
                    let observed = file_observer::observe(&face.path, ExternalFileRole::Face);
                    let externally_changed = observed.is_changed();
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
                    } else if externally_changed {
                        display_label.push_str("  [changed externally]");
                    }
                    if duplicate_count > 1 {
                        display_label.push_str(&format!("  [duplicate x{duplicate_count}]"));
                    }
                    let accent = if status.is_rejected() || !face.available {
                        Some(egui::Color32::from_rgb(235, 95, 95))
                    } else if externally_changed || duplicate_count > 1 {
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
                            "Source TIFF is missing or unreadable: {}. Use Locate file or Locate folder.",
                            face.path.display()
                        )
                    } else if externally_changed {
                        format!(
                            "Source TIFF changed outside Shade Editor: {}. Cached preview remains authoritative until Reload changed file verifies and accepts the new bytes.",
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
                    if let Some(face) = app.faces.get(index) {
                        file_observer::release(&face.path, ExternalFileRole::Face);
                    }
                    actions.push(FaceUiAction::Delete(index));
                } else if let Some(index) = requested_face {
                    actions.push(FaceUiAction::Select(index));
                }

                let active_observed = app.faces.get(app.current_face).map(|face| {
                    file_observer::observe(&face.path, ExternalFileRole::Face)
                });
                let active_missing = app
                    .faces
                    .get(app.current_face)
                    .is_some_and(|face| !face.available);
                let active_changed = active_observed
                    .as_ref()
                    .is_some_and(|observed| observed.is_available() && observed.is_changed());
                let missing_count = app.faces.iter().filter(|face| !face.available).count();
                let mut locate_file = false;
                let mut reload_changed = false;
                let mut locate_folder = false;
                ui.horizontal_wrapped(|ui| {
                    if active_missing {
                        locate_file = ui.button("Locate file").clicked();
                    } else if active_changed {
                        reload_changed = ui
                            .button("Reload changed file")
                            .on_hover_text(
                                "Verify dimensions, bit depth, color model and channel topology, then accept the new source bytes",
                            )
                            .clicked();
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
                if reload_changed {
                    reload_changed_current_face(app);
                } else if locate_file {
                    actions.push(FaceUiAction::RelinkCurrent);
                } else if locate_folder {
                    actions.push(FaceUiAction::RelinkMissingFolder);
                }
            }
        });

    ui.separator();
    super::reference_panel::ui_reference_file(app, ui);
    ui.separator();
    egui::CollapsingHeader::new("Snapshots")
        .id_salt("workspace-snapshots-panel")
        .default_open(true)
        .show(ui, |ui| super::snapshots_panel::ui_snapshots(app, ui));
    ui.separator();
    egui::CollapsingHeader::new("Test Code")
        .id_salt("workspace-test-code-panel")
        .default_open(true)
        .show(ui, |ui| super::test_code_panel::ui_test_code(app, ui));
    ui.separator();
    egui::CollapsingHeader::new("History")
        .id_salt("workspace-history-panel")
        .default_open(true)
        .show(ui, |ui| super::history_panel::ui_history(app, ui));

    for action in actions {
        app.dispatch_face_ui_action(action);
    }
}
