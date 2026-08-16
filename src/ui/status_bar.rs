use crate::*;
use eframe::egui;

impl ShadeApp {
    pub(crate) fn project_save_state_label(&self) -> (&'static str, bool) {
        if self.project_autosave_busy {
            ("Saving…", false)
        } else if self.project_autosave_error.is_some() {
            ("Autosave failed", true)
        } else if self.project_path.is_none() {
            if self.project_dirty && !self.faces.is_empty() {
                ("Unsaved changes", false)
            } else {
                ("", false)
            }
        } else if self.project_dirty {
            ("Unsaved changes", false)
        } else {
            ("Saved", false)
        }
    }

    pub(crate) fn ui_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let dirty = if self.project_dirty {
                " * modified"
            } else {
                ""
            };
            ui.label(format!("{}{}", self.status_message, dirty));
            self.ui_color_conversion_status(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Fit").clicked() {
                    self.fit_requested = true;
                }
                let zoom = ui.add(
                    egui::Slider::new(&mut self.zoom, 0.05..=8.0)
                        .logarithmic(true)
                        .text("Zoom"),
                );
                if zoom.changed() {
                    self.viewport_recenter = true;
                }
                if let Some(path) = &self.project_path {
                    ui.label(path.display().to_string());
                }
            });
        });
        self.ui_color_conversion_window(ui.ctx());
    }
}
