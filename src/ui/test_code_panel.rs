use std::collections::BTreeSet;

use crate::runtime_preview::RuntimePreviewSource;
use crate::*;
use eframe::egui;
use windows_shade_editor::test_stack::{TestStackAnchor, TestStackLayout};

#[derive(Clone)]
struct TestStackUiState {
    selected_snapshot_ids: BTreeSet<u64>,
    layout: TestStackLayout,
    anchor: TestStackAnchor,
    follow_code_corner: bool,
}

impl Default for TestStackUiState {
    fn default() -> Self {
        Self {
            selected_snapshot_ids: BTreeSet::new(),
            layout: TestStackLayout::THREE_ROWS,
            anchor: TestStackAnchor::TopLeft,
            follow_code_corner: true,
        }
    }
}

fn should_enable_test_code_by_default(app: &ShadeApp) -> bool {
    !app.project.test_code.enabled
        && app.project_path.is_none()
        && !app.project_dirty
        && app.project.snapshots.is_empty()
        && app.project.test_code.text.trim().is_empty()
}

pub(crate) fn ui_test_code(app: &mut ShadeApp, ui: &mut egui::Ui) {
    // Existing saved projects keep their persisted choice. Only a pristine new
    // project gets the requested default-on workflow, and the user can disable
    // it normally afterwards (that marks the project dirty and is respected).
    if should_enable_test_code_by_default(app) {
        app.project.test_code.enabled = true;
    }

    let channel_names = app
        .faces
        .get(app.current_face)
        .filter(|face| face.available)
        .map(|face| face.preview.channel_names().to_vec())
        .unwrap_or_default();
    let palette = app.project.channel_palette.clone();
    let fallback = app
        .project
        .active_snapshot_name()
        .unwrap_or("Test")
        .to_owned();
    let mut changed = false;

    ui.horizontal(|ui| {
        changed |= ui
            .checkbox(&mut app.project.test_code.enabled, "On")
            .on_hover_text("Enable Test Code for Snapshot test exports")
            .changed();
        changed |= ui
            .add_enabled(
                app.project.test_code.enabled,
                egui::TextEdit::singleline(&mut app.project.test_code.text)
                    .hint_text(format!("Code · empty uses {fallback}"))
                    .desired_width(f32::INFINITY),
            )
            .changed();
    });

    ui.add_enabled_ui(app.project.test_code.enabled, |ui| {
        if !channel_names.is_empty() {
            let selected_display = if app.project.test_code.channel == TEST_CODE_ALL_CHANNELS {
                "Master".to_owned()
            } else {
                let selected_index = channel_names
                    .iter()
                    .position(|name| name == &app.project.test_code.channel)
                    .unwrap_or(0);
                channel_display_name(
                    palette.as_ref(),
                    &channel_names[selected_index],
                    selected_index,
                )
                .to_owned()
            };
            ui.horizontal(|ui| {
                ui.small("Ink");
                egui::ComboBox::from_id_salt("compact-test-code-channel")
                    .selected_text(selected_display)
                    .width((ui.available_width() - 8.0).max(100.0))
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut app.project.test_code.channel,
                                TEST_CODE_ALL_CHANNELS.to_owned(),
                                "Master",
                            )
                            .changed();
                        ui.separator();
                        for (index, name) in channel_names.iter().enumerate() {
                            let display = channel_display_name(palette.as_ref(), name, index);
                            changed |= ui
                                .selectable_value(
                                    &mut app.project.test_code.channel,
                                    name.to_owned(),
                                    display,
                                )
                                .changed();
                        }
                    });
            });
        }

        egui::CollapsingHeader::new("Placement")
            .id_salt("compact-test-code-placement")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.small("Tahoma");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut app.project.test_code.font_size_pt)
                                .range(6.0..=72.0)
                                .speed(1.0)
                                .suffix(" pt"),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut app.project.test_code.margin_cm)
                                .range(0.0..=5.0)
                                .speed(0.1)
                                .suffix(" cm"),
                        )
                        .changed();
                });
                egui::ComboBox::from_id_salt("compact-test-code-position")
                    .selected_text(match app.project.test_code.position {
                        TestCodePosition::TopLeft => "Top left",
                        TestCodePosition::TopRight => "Top right",
                        TestCodePosition::BottomLeft => "Bottom left",
                        TestCodePosition::BottomRight => "Bottom right",
                    })
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            (TestCodePosition::TopLeft, "Top left"),
                            (TestCodePosition::TopRight, "Top right"),
                            (TestCodePosition::BottomLeft, "Bottom left"),
                            (TestCodePosition::BottomRight, "Bottom right"),
                        ] {
                            changed |= ui
                                .selectable_value(&mut app.project.test_code.position, value, label)
                                .changed();
                        }
                    });
            });
    });

    ui.small("Test Code is written only by Snapshot test export; normal Face/Export All output stays uncoded.");
    if changed {
        app.mark_project_dirty();
    }

    ui.add_space(4.0);
    ui.separator();
    ui_test_stack(app, ui);
}

fn ui_test_stack(app: &mut ShadeApp, ui: &mut egui::Ui) {
    let state_id = egui::Id::new(("test-stack-ui-state", app.lifecycle.session_id));
    let mut state = ui
        .ctx()
        .data_mut(|data| data.get_temp::<TestStackUiState>(state_id))
        .unwrap_or_default();

    let available_ids = app
        .project
        .snapshots
        .iter()
        .map(|snapshot| snapshot.id)
        .collect::<BTreeSet<_>>();
    state
        .selected_snapshot_ids
        .retain(|snapshot_id| available_ids.contains(snapshot_id));
    if state.follow_code_corner {
        state.anchor = anchor_from_test_code_position(app.project.test_code.position);
    }

    let mut build_requested = false;
    egui::CollapsingHeader::new("Test Stack")
        .id_salt("test-stack-controls")
        .default_open(false)
        .show(ui, |ui| {
            ui.small("Combine saved Snapshot tests into one same-size TIFF. Each cell keeps the selected code corner; no image scaling is used.");
            ui.add_space(4.0);

            ui.horizontal_wrapped(|ui| {
                ui.small("Layout");
                for (layout, label) in [
                    (TestStackLayout::THREE_ROWS, "3×1 rows"),
                    (TestStackLayout::TWO_BY_TWO, "2×2"),
                    (TestStackLayout::ONE_BY_TWO, "1×2"),
                    (TestStackLayout::ONE_BY_THREE, "1×3"),
                ] {
                    ui.selectable_value(&mut state.layout, layout, label);
                }
            });

            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut state.follow_code_corner, "Follow Test Code corner")
                    .on_hover_text("Keep Test Stack alignment synchronized with the corner used to print the Snapshot code");
                if state.follow_code_corner {
                    state.anchor = anchor_from_test_code_position(app.project.test_code.position);
                    ui.small(format!("· {}", state.anchor.label()));
                }
            });
            ui.add_enabled_ui(!state.follow_code_corner, |ui| {
                ui.horizontal(|ui| {
                    ui.small("Custom anchor");
                    egui::ComboBox::from_id_salt("test-stack-anchor")
                        .selected_text(state.anchor.label())
                        .show_ui(ui, |ui| {
                            for anchor in [
                                TestStackAnchor::TopLeft,
                                TestStackAnchor::TopRight,
                                TestStackAnchor::BottomLeft,
                                TestStackAnchor::BottomRight,
                            ] {
                                ui.selectable_value(&mut state.anchor, anchor, anchor.label());
                            }
                        });
                });
            });

            ui.add_space(4.0);
            ui.strong(format!(
                "Snapshots · select exactly {}",
                state.layout.capacity()
            ));
            if app.project.snapshots.is_empty() {
                ui.colored_label(egui::Color32::YELLOW, "No saved Snapshots available.");
            } else {
                for snapshot in &app.project.snapshots {
                    let mut selected = state.selected_snapshot_ids.contains(&snapshot.id);
                    if ui
                        .checkbox(
                            &mut selected,
                            format!("{}  ·  #{}", snapshot.name, snapshot.id),
                        )
                        .changed()
                    {
                        if selected {
                            state.selected_snapshot_ids.insert(snapshot.id);
                        } else {
                            state.selected_snapshot_ids.remove(&snapshot.id);
                        }
                    }
                }
            }

            let selected_count = state.selected_snapshot_ids.len();
            let count_ok = selected_count == state.layout.capacity();
            if !count_ok && selected_count > 0 {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!(
                        "Selected {selected_count}; {} needs {} Snapshot(s).",
                        state.layout.label(),
                        state.layout.capacity()
                    ),
                );
            }
            if !app.project.test_code.enabled {
                ui.small("Test Code is Off. Corner alignment still works, but no code will be printed into the cells.");
            }

            let queues_idle = !app.export.queue.has_pending() && !app.conversion_queue.has_pending();
            let can_build = count_ok
                && selected_count > 0
                && app.job.is_none()
                && queues_idle
                && workflow::active_face_available(app)
                && app.active_face_is_tiff_export_source();
            build_requested = ui
                .add_enabled(can_build, egui::Button::new("Build Test Stack..."))
                .on_hover_text(if queues_idle {
                    "Render the selected saved Snapshots and save one composed TIFF"
                } else {
                    "Finish the active Export/Conversion Queue first"
                })
                .clicked();
        });

    let selected_ids = app
        .project
        .snapshots
        .iter()
        .filter(|snapshot| state.selected_snapshot_ids.contains(&snapshot.id))
        .map(|snapshot| snapshot.id)
        .collect::<Vec<_>>();
    let layout = state.layout;
    let anchor = state.anchor;
    ui.ctx()
        .data_mut(|data| data.insert_temp(state_id, state));

    if build_requested {
        start_test_stack(app, selected_ids, layout, anchor);
    }
}

fn start_test_stack(
    app: &mut ShadeApp,
    snapshot_ids: Vec<u64>,
    layout: TestStackLayout,
    anchor: TestStackAnchor,
) {
    if app.job.is_some()
        || app.export.queue.has_pending()
        || app.conversion_queue.has_pending()
        || !workflow::active_face_available(app)
        || !app.active_face_is_tiff_export_source()
    {
        return;
    }
    if let Err(err) = layout.validate_snapshot_count(snapshot_ids.len()) {
        app.report_error(err);
        return;
    }
    let Some(face) = app.faces.get(app.current_face) else {
        return;
    };
    let source = face.path.clone();
    let stem = source
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "face".to_owned());
    let suggested = format!("{stem}-test-stack-{}x{}.tif", layout.rows, layout.columns);
    let Some(destination) = rfd::FileDialog::new()
        .add_filter("TIFF image", &["tif", "tiff"])
        .set_file_name(suggested)
        .save_file()
    else {
        return;
    };

    let protected_sources = app
        .faces
        .iter()
        .map(|face| face.path.clone())
        .collect::<Vec<_>>();
    if let Some(conflict) = path_safety::conflicts_with_any_source(&destination, &protected_sources) {
        app.report_error(format!(
            "Refusing Test Stack output because the destination resolves to a source image: {}",
            conflict.display()
        ));
        return;
    }

    // main.rs and lib.rs intentionally compile their own application model modules.
    // Bridge the identical persisted schema explicitly at this boundary instead of
    // coupling the UI binary's ShadeProject type to the library crate's type identity.
    let project = match serde_json::to_vec(&app.project).and_then(|bytes| {
        serde_json::from_slice::<windows_shade_editor::model::ShadeProject>(&bytes)
    }) {
        Ok(project) => project,
        Err(err) => {
            app.report_error(format!("Cannot prepare Test Stack project state: {err}"));
            return;
        }
    };

    let default_dpi = app.settings.default_dpi;
    let options = windows_shade_editor::export::ExportOptions {
        force_lzw: app.settings.lzw_compression,
    };
    let result_destination = destination.clone();
    app.launch_job("Building Test Stack", move |job_progress| {
        let result = windows_shade_editor::test_stack::export_test_stack_with_progress(
            &source,
            &destination,
            &project,
            &snapshot_ids,
            layout,
            anchor,
            default_dpi,
            options,
            |fraction, detail| {
                ShadeApp::set_progress(
                    &job_progress,
                    Some(fraction),
                    "Building Test Stack",
                    detail,
                );
            },
        )
        .map(|_| format!("Test Stack saved: {}", result_destination.display()));
        JobResult::Export(SnapshotExportBatchResult {
            result,
            marks: Vec::new(),
        })
    });
}

fn anchor_from_test_code_position(position: TestCodePosition) -> TestStackAnchor {
    match position {
        TestCodePosition::TopLeft => TestStackAnchor::TopLeft,
        TestCodePosition::TopRight => TestStackAnchor::TopRight,
        TestCodePosition::BottomLeft => TestStackAnchor::BottomLeft,
        TestCodePosition::BottomRight => TestStackAnchor::BottomRight,
    }
}
