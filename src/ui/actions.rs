use crate::workflow::*;
use crate::*;
use eframe::egui;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FaceUiAction {
    RenameProject(String),
    Select(usize),
    SetStatus {
        index: usize,
        status: model::FaceStatus,
    },
    Delete(usize),
    RelinkCurrent,
    RelinkMissingFolder,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NavigationUiAction {
    NewProject,
    OpenProjectDialog,
    OpenRecent(PathBuf),
    ShowProjectView,
    AddFacesDialog,
    QuickSave,
    Save,
    SaveAs,
    InspectTiff,
    ShowExportQueue,
    ExportCurrent,
    ExportAll,
    ValidateCurrent,
    ShowSettings,
    ShowAbout,
    ShowLogs,
    DismissError,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ProjectViewUiAction {
    SetOpen(bool),
    Select(String),
    Reveal(String),
    Relink(String),
    Remove(String),
    Open(String),
}

impl ShadeApp {
    pub(crate) fn dispatch_face_ui_action(&mut self, action: FaceUiAction) {
        match action {
            FaceUiAction::RenameProject(name) => {
                if self.project.name != name {
                    self.project.name = name;
                    self.mark_project_dirty();
                }
            }
            FaceUiAction::Select(index) => {
                if index >= self.faces.len() {
                    return;
                }
                self.current_face = index;
                self.selected_channel = 0;
                self.solo_channel = None;
                self.fit_requested = true;
                self.viewport_recenter = true;
                self.mark_current_preview_dirty();
                if self
                    .project
                    .faces
                    .get(index)
                    .is_some_and(|face| face.status.is_rejected())
                {
                    self.report_info(
                        "Warning: selected Face is Rejected and is excluded from Export All",
                    );
                }
            }
            FaceUiAction::SetStatus { index, status } => {
                if let Some(face) = self.project.faces.get_mut(index) {
                    if face.status != status {
                        face.status = status;
                        self.mark_project_dirty();
                        self.report_info(match status {
                            model::FaceStatus::Accepted => {
                                "Face marked Accepted — eligible for Export All"
                            }
                            model::FaceStatus::Rejected => {
                                "Face marked Rejected — retained for reference and excluded from Export All"
                            }
                        });
                    }
                }
            }
            FaceUiAction::Delete(index) => {
                if index < self.faces.len() {
                    self.current_face = index;
                    self.remove_current_face();
                }
            }
            FaceUiAction::RelinkCurrent => relink_current_face_dialog(self),
            FaceUiAction::RelinkMissingFolder => relink_missing_faces_folder_dialog(self),
        }
    }

    pub(crate) fn dispatch_navigation_ui_action(
        &mut self,
        action: NavigationUiAction,
        ctx: &egui::Context,
    ) {
        match action {
            NavigationUiAction::NewProject => self.new_project(),
            NavigationUiAction::OpenProjectDialog => self.open_project_dialog(),
            NavigationUiAction::OpenRecent(path) => {
                self.request_project_transition(ProjectTransition::Open(path), Some(ctx));
            }
            NavigationUiAction::ShowProjectView => self.show_previous_shades = true,
            NavigationUiAction::AddFacesDialog => self.add_faces_dialog(),
            NavigationUiAction::QuickSave => {
                self.quick_save_project();
            }
            NavigationUiAction::Save => {
                self.save_project(false);
            }
            NavigationUiAction::SaveAs => {
                self.save_project(true);
            }
            NavigationUiAction::InspectTiff => self.inspect_tiff_dialog(),
            NavigationUiAction::ShowExportQueue => self.export.show_queue = true,
            NavigationUiAction::ExportCurrent => self.export_current_dialog(),
            NavigationUiAction::ExportAll => self.export_all_dialog(),
            NavigationUiAction::ValidateCurrent => self.validate_current_face_dialog(),
            NavigationUiAction::ShowSettings => self.show_settings = true,
            NavigationUiAction::ShowAbout => self.show_about = true,
            NavigationUiAction::ShowLogs => {
                self.log_cache = self.log.read();
                self.show_logs = true;
            }
            NavigationUiAction::DismissError => {
                self.toast = None;
                if self.status_message == "Error - see Logs" {
                    self.status_message = "Ready".to_owned();
                }
            }
        }
    }

    pub(crate) fn dispatch_project_view_ui_action(
        &mut self,
        action: ProjectViewUiAction,
        ctx: &egui::Context,
    ) {
        match action {
            ProjectViewUiAction::SetOpen(open) => self.show_previous_shades = open,
            ProjectViewUiAction::Select(path) => {
                if self.previous_shades_selected.as_deref() != Some(path.as_str()) {
                    self.load_previous_shade_preview(ctx, &path);
                }
            }
            ProjectViewUiAction::Reveal(path) => {
                if let Err(err) = reveal_in_explorer(Path::new(&path)) {
                    self.report_error(err);
                }
            }
            ProjectViewUiAction::Relink(old_path) => {
                if let Some(new_path) = rfd::FileDialog::new()
                    .add_filter("Shade projects", &["shade"])
                    .pick_file()
                {
                    match self.previous_shades.relink_path(&old_path, &new_path) {
                        Ok(new_display) => {
                            if let Err(err) = self.previous_shades.save() {
                                self.log.error(&err);
                            }
                            self.previous_shade_list_textures.remove(&old_path);
                            self.previous_shade_list_texture_lru
                                .retain(|item| item != &old_path);
                            self.load_previous_shade_preview(ctx, &new_display);
                        }
                        Err(err) => self.report_error(err),
                    }
                }
            }
            ProjectViewUiAction::Remove(path) => {
                if self.previous_shades.remove_path(&path) {
                    if let Err(err) = self.previous_shades.save() {
                        self.log.error(&err);
                    }
                    self.previous_shade_list_textures.remove(&path);
                    self.previous_shade_list_texture_lru
                        .retain(|item| item != &path);
                    if self.previous_shades_selected.as_deref() == Some(path.as_str()) {
                        self.previous_shades_selected = None;
                        self.previous_shade_preview = None;
                        self.previous_shade_preview_error = None;
                        self.previous_shade_texture = None;
                    }
                }
            }
            ProjectViewUiAction::Open(path) => {
                self.request_project_transition(ProjectTransition::Open(PathBuf::from(path)), None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_actions_are_typed_and_payload_preserving() {
        let action = FaceUiAction::SetStatus {
            index: 7,
            status: model::FaceStatus::Rejected,
        };
        assert_eq!(
            action,
            FaceUiAction::SetStatus {
                index: 7,
                status: model::FaceStatus::Rejected,
            }
        );
    }

    #[test]
    fn navigation_open_recent_preserves_path() {
        let path = PathBuf::from(r"C:\work\sample.shade");
        assert_eq!(
            NavigationUiAction::OpenRecent(path.clone()),
            NavigationUiAction::OpenRecent(path)
        );
    }
}
