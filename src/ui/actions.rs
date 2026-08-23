use std::collections::BTreeMap;

use crate::workflow::*;
use crate::*;
use eframe::egui;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FaceUiAction {
    RenameProject(String),
    AddFacesDialog,
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
    OpenLinkedProject(PathBuf),
    RelinkLinkedProject(PathBuf),
    ShowProjectView,
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ExportQueueUiAction {
    SetOpen(bool),
    ResumeRecovered,
    TogglePaused,
    RetryAllFailed,
    ClearJobs,
    Resume(u64),
    Cancel(u64),
    Retry(u64),
    RevealFolder(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AdjustmentUiAction {
    Undo,
    Redo,
    ClearHistory,
    RestoreClearedHistory,
    JumpHistory(usize),
    SelectProjectPalette(palette::ChannelPalette),
    ShowComposite,
    SelectChannel(usize),
    PersistSettings,
    InvalidatePreviews,
    QueueHistory(BTreeMap<String, model::ChannelAdjustment>),
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
            FaceUiAction::AddFacesDialog => self.add_faces_dialog(),
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
            NavigationUiAction::OpenRecent(path)
            | NavigationUiAction::OpenLinkedProject(path) => {
                self.request_project_transition(ProjectTransition::Open(path), Some(ctx));
            }
            NavigationUiAction::RelinkLinkedProject(previous_path) => {
                let Some(current_project_path) = self.project_path.clone() else {
                    self.report_error("Save the current project before repairing linked-project paths.");
                    return;
                };
                let Some(replacement_path) = rfd::FileDialog::new()
                    .add_filter("Shade projects", &["shade"])
                    .pick_file()
                else {
                    return;
                };

                match super::project_link_navigation_core::relink_navigation_target(
                    &mut self.project,
                    &current_project_path,
                    &previous_path,
                    &replacement_path,
                ) {
                    Ok(target) => {
                        self.mark_project_dirty();
                        let name = target.project_name.as_deref().unwrap_or("linked project");
                        self.report_info(format!(
                            "Relinked {name} to {}. Save the current project to persist the repaired link.",
                            replacement_path.display()
                        ));
                    }
                    Err(err) => self.report_error(err),
                }
            }
            NavigationUiAction::ShowProjectView => self.project_view.open = true,
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
            ProjectViewUiAction::SetOpen(open) => self.project_view.open = open,
            ProjectViewUiAction::Select(path) => {
                if self.project_view.needs_preview_load(&path) {
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
                            self.project_view.forget_path(&old_path);
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
                    self.project_view.forget_path(&path);
                }
            }
            ProjectViewUiAction::Open(path) => {
                self.request_project_transition(ProjectTransition::Open(PathBuf::from(path)), None);
            }
        }
    }

    pub(crate) fn dispatch_export_queue_ui_action(&mut self, action: ExportQueueUiAction) {
        match action {
            ExportQueueUiAction::SetOpen(open) => self.export.show_queue = open,
            ExportQueueUiAction::ResumeRecovered => {
                let count = self.export.queue.resume_recovered();
                if count > 0 {
                    self.report_info(format!("Resumed {count} recovered export(s)"));
                }
            }
            ExportQueueUiAction::TogglePaused => {
                let paused = !self.export.queue.is_paused();
                self.export.queue.set_paused(paused);
                self.report_info(if paused {
                    "Export Queue paused; current atomic export may finish safely"
                } else {
                    "Export Queue resumed"
                });
            }
            ExportQueueUiAction::RetryAllFailed => {
                let count = self.export.queue.retry_all_failed();
                if count > 0 {
                    self.report_info(format!("Retried {count} failed export(s)"));
                }
            }
            ExportQueueUiAction::ClearJobs => {
                self.export.queue.clear_finished();
            }
            ExportQueueUiAction::Resume(id) => {
                self.export.queue.resume(id);
            }
            ExportQueueUiAction::Cancel(id) => {
                self.export.queue.cancel(id);
            }
            ExportQueueUiAction::Retry(id) => {
                self.export.queue.retry(id);
            }
            ExportQueueUiAction::RevealFolder(folder) => {
                if let Err(err) = open_folder(&folder) {
                    self.report_error(err);
                }
            }
        }
    }

    pub(crate) fn dispatch_adjustment_ui_action(
        &mut self,
        action: AdjustmentUiAction,
        ctx: &egui::Context,
    ) {
        match action {
            AdjustmentUiAction::Undo => self.undo_adjustment(ctx),
            AdjustmentUiAction::Redo => self.redo_adjustment(ctx),
            AdjustmentUiAction::ClearHistory => {
                let scope = self.project.active_snapshot_id;
                self.flush_history_now();
                self.history_clear_backup = Some((scope, self.history.clone()));
                self.history
                    .reset(&self.project.adjustments, "Current state");
                self.sync_history_to_active_snapshot();
                self.report_info("History cleared - Undo clear is available once");
            }
            AdjustmentUiAction::RestoreClearedHistory => {
                let scope = self.project.active_snapshot_id;
                if let Some((backup_scope, backup)) = self.history_clear_backup.take() {
                    if backup_scope == scope {
                        self.history = backup;
                        self.sync_history_to_active_snapshot();
                        self.report_info("Cleared history restored");
                    }
                }
            }
            AdjustmentUiAction::JumpHistory(index) => {
                self.flush_history_now();
                if let Some(adjustments) = self.history.jump(index) {
                    self.apply_history_adjustments(adjustments, "History state selected");
                }
            }
            AdjustmentUiAction::SelectProjectPalette(palette) => {
                self.select_project_palette(palette);
            }
            AdjustmentUiAction::ShowComposite => {
                self.adjustment_scope = AdjustmentScope::All;
                self.show_composite();
            }
            AdjustmentUiAction::SelectChannel(index) => self.select_channel(index, true),
            AdjustmentUiAction::PersistSettings => self.save_settings_quietly(),
            AdjustmentUiAction::InvalidatePreviews => self.mark_all_previews_dirty(),
            AdjustmentUiAction::QueueHistory(before) => {
                self.queue_adjustment_history(&before);
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
        assert_eq!(FaceUiAction::AddFacesDialog, FaceUiAction::AddFacesDialog);
    }

    #[test]
    fn navigation_project_paths_are_payload_preserving() {
        let path = PathBuf::from(r"C:\work\sample.shade");
        assert_eq!(
            NavigationUiAction::OpenRecent(path.clone()),
            NavigationUiAction::OpenRecent(path.clone())
        );
        assert_eq!(
            NavigationUiAction::OpenLinkedProject(path.clone()),
            NavigationUiAction::OpenLinkedProject(path.clone())
        );
        assert_eq!(
            NavigationUiAction::RelinkLinkedProject(path.clone()),
            NavigationUiAction::RelinkLinkedProject(path)
        );
    }

    #[test]
    fn export_queue_actions_preserve_job_and_folder_payloads() {
        assert_eq!(
            ExportQueueUiAction::Retry(42),
            ExportQueueUiAction::Retry(42)
        );
        assert_eq!(ExportQueueUiAction::ClearJobs, ExportQueueUiAction::ClearJobs);
        let folder = PathBuf::from(r"C:\exports\batch-42");
        assert_eq!(
            ExportQueueUiAction::RevealFolder(folder.clone()),
            ExportQueueUiAction::RevealFolder(folder)
        );
    }

    #[test]
    fn adjustment_actions_preserve_history_and_palette_payloads() {
        assert_eq!(
            AdjustmentUiAction::JumpHistory(17),
            AdjustmentUiAction::JumpHistory(17)
        );
        let palette = palette::builtin_cmyk();
        assert_eq!(
            AdjustmentUiAction::SelectProjectPalette(palette.clone()),
            AdjustmentUiAction::SelectProjectPalette(palette)
        );
        let mut before = BTreeMap::new();
        before.insert("Cyan".to_owned(), model::ChannelAdjustment::default());
        assert_eq!(
            AdjustmentUiAction::QueueHistory(before.clone()),
            AdjustmentUiAction::QueueHistory(before)
        );
    }
}
