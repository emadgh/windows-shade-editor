use super::actions::NavigationUiAction;
use super::project_link_navigation_core::{
    LinkedProjectNavigationStatus, linked_navigation_targets,
};
use crate::color_conversion::ProjectRole;
use crate::*;
use eframe::egui;

impl ShadeApp {
    pub(crate) fn ui_linked_project_navigation(&mut self, ui: &mut egui::Ui) {
        let Some(current_path) = self.project_path.clone() else {
            return;
        };
        if self.project.project_role == ProjectRole::Standalone {
            return;
        }

        // Re-evaluate from persisted linked projects whenever the control is rendered. This keeps
        // append/re-convert results and external file changes visible without an application restart.
        let targets = linked_navigation_targets(&self.project, &current_path);
        let ready_count = targets.iter().filter(|target| target.status.can_open()).count();
        let total = targets.len();
        let menu_label = match self.project.project_role {
            ProjectRole::Source => {
                if total == 0 {
                    "Production links".to_owned()
                } else {
                    format!("Production links ({ready_count}/{total})")
                }
            }
            ProjectRole::Production => {
                if ready_count > 0 {
                    "Source link (ready)".to_owned()
                } else {
                    "Source link".to_owned()
                }
            }
            ProjectRole::Standalone => return,
        };

        let mut requested_action = None;
        ui.menu_button(menu_label, |ui| {
            ui.set_min_width(520.0);
            ui.strong(match self.project.project_role {
                ProjectRole::Source => "Linked Production projects",
                ProjectRole::Production => "Originating Source project",
                ProjectRole::Standalone => "Linked projects",
            });
            ui.small(match self.project.project_role {
                ProjectRole::Source => {
                    "Each Production target is validated against reciprocal lineage before switching."
                }
                ProjectRole::Production => {
                    "The Source link must resolve back to this exact Production project."
                }
                ProjectRole::Standalone => "",
            });
            ui.separator();

            if targets.is_empty() {
                ui.label(match self.project.project_role {
                    ProjectRole::Source => "No linked Production project is recorded yet.",
                    ProjectRole::Production => "No originating Source project is recorded.",
                    ProjectRole::Standalone => "No linked projects.",
                });
                return;
            }

            for (index, target) in targets.iter().enumerate() {
                if index > 0 {
                    ui.separator();
                }

                let name = target.project_name.as_deref().unwrap_or("Unknown project");
                ui.horizontal_wrapped(|ui| {
                    ui.strong(name);
                    let status_text = match target.status {
                        LinkedProjectNavigationStatus::Ready => "Ready".to_owned(),
                        LinkedProjectNavigationStatus::Missing => {
                            "Missing · Relink available".to_owned()
                        }
                        LinkedProjectNavigationStatus::Unreadable => {
                            "Unreadable · Relink available".to_owned()
                        }
                        LinkedProjectNavigationStatus::RoleMismatch
                        | LinkedProjectNavigationStatus::ReciprocalLinkMissing => {
                            format!("Incompatible · {}", target.status.label())
                        }
                    };
                    ui.label(status_text).on_hover_text(&target.detail);
                });

                if let Some(identity) = target.identity.as_deref() {
                    ui.small(identity);
                }
                ui.small(target.path.display().to_string())
                    .on_hover_text(&target.detail);

                ui.horizontal(|ui| {
                    if target.status.can_open() {
                        let label = match target.role {
                            ProjectRole::Production => "Open / Switch to Production",
                            ProjectRole::Source => "Open / Switch to Source",
                            ProjectRole::Standalone => "Open linked project",
                        };
                        if ui.button(label).clicked() {
                            requested_action =
                                Some(NavigationUiAction::OpenLinkedProject(target.path.clone()));
                            ui.close();
                        }
                    }

                    if target.status.can_relink()
                        && ui
                            .button("Relink...")
                            .on_hover_text(
                                "Select the moved/replacement .shade file. It must have the expected role and reciprocal lineage.",
                            )
                            .clicked()
                    {
                        requested_action =
                            Some(NavigationUiAction::RelinkLinkedProject(target.path.clone()));
                        ui.close();
                    }
                });
            }
        });

        if let Some(action) = requested_action {
            self.dispatch_navigation_ui_action(action, ui.ctx());
        }
    }
}
