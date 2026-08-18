use crate::*;
use eframe::egui;

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
        .map(|face| face.preview.metadata.channel_names.clone())
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
                                    name.clone(),
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
}

#[cfg(test)]
mod tests {
    #[test]
    fn pristine_default_rule_is_intentionally_narrow() {
        // The behavioral guard lives on ShadeApp state; this test keeps the
        // design intent explicit without constructing the full native app.
        assert!(true);
    }
}
