use crate::*;
use eframe::egui;

impl ShadeApp {
    pub(crate) fn project_save_state_label(&self) -> (&'static str, bool) {
        if self.project_path.is_none() {
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
        // Candidate rendering and the durable batch queue are runtimes of the single
        // Production Color Conversion workflow, not independent operator surfaces.
        self.poll_conversion_candidate_runtime(ui.ctx());
        self.poll_conversion_batch_runtime();

        ui.horizontal(|ui| {
            let dirty = if self.project_dirty { " * modified" } else { "" };
            ui.label(format!("{}{}", self.status_message, dirty));
            self.ui_linked_project_navigation(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(path) = &self.project_path {
                    ui.label(path.display().to_string());
                }
            });
        });
        self.ui_color_conversion_window(ui.ctx());
        self.ui_conversion_route_migration(ui.ctx());
    }
}
