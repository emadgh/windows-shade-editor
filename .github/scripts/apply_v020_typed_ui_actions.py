from pathlib import Path

root = Path(__file__).resolve().parents[2]
ui_dir = root / "src" / "ui"


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


def replace_all_required(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count == 0:
        raise SystemExit(f"{label}: no matches")
    path.write_text(text.replace(old, new), encoding="utf-8")


actions = r'''use crate::workflow::*;
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
            NavigationUiAction::QuickSave => self.quick_save_project(),
            NavigationUiAction::Save => self.save_project(false),
            NavigationUiAction::SaveAs => self.save_project(true),
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
                self.request_project_transition(
                    ProjectTransition::Open(PathBuf::from(path)),
                    None,
                );
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
'''
(ui_dir / "actions.rs").write_text(actions, encoding="utf-8")

# Register action module.
ui_mod = ui_dir / "mod.rs"
text = ui_mod.read_text(encoding="utf-8")
if "pub(crate) mod actions;" not in text:
    text = "pub(crate) mod actions;\n" + text
ui_mod.write_text(text, encoding="utf-8")

# Faces: render local edits, emit typed actions, and dispatch after the presentation pass.
faces = ui_dir / "faces.rs"
text = faces.read_text(encoding="utf-8")
text = text.replace("use crate::workflow::*;\n", "use super::actions::FaceUiAction;\n", 1)
old = '''pub(crate) fn ui_faces(app: &mut ShadeApp, ui: &mut egui::Ui) {
    ui.label("Project title");
    if ui
        .add(
            egui::TextEdit::singleline(&mut app.project.name)
                .hint_text("Uses the .shade filename after first save")
                .desired_width(f32::INFINITY),
        )
        .changed()
    {
        app.mark_project_dirty();
    }
'''
new = '''pub(crate) fn ui_faces(app: &mut ShadeApp, ui: &mut egui::Ui) {
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
'''
if old not in text:
    raise SystemExit("faces project title block not found")
text = text.replace(old, new, 1)
old = '''        if let Some((index, status)) = requested_status {
            if let Some(face) = app.project.faces.get_mut(index) {
                if face.status != status {
                    face.status = status;
                    app.mark_project_dirty();
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
                app.report_info(
                    "Warning: selected Face is Rejected and is excluded from Export All",
                );
            }
        }
'''
new = '''        if let Some((index, status)) = requested_status {
            actions.push(FaceUiAction::SetStatus { index, status });
        }
        if let Some(index) = requested_delete {
            actions.push(FaceUiAction::Delete(index));
        } else if let Some(index) = requested_face {
            actions.push(FaceUiAction::Select(index));
        }
'''
if old not in text:
    raise SystemExit("faces status/select block not found")
text = text.replace(old, new, 1)
text = text.replace(
    '''        if locate_file {
            relink_current_face_dialog(app);
        } else if locate_folder {
            relink_missing_faces_folder_dialog(app);
        } else if remove {
            app.remove_current_face();
        }
''',
    '''        if locate_file {
            actions.push(FaceUiAction::RelinkCurrent);
        } else if locate_folder {
            actions.push(FaceUiAction::RelinkMissingFolder);
        } else if remove {
            actions.push(FaceUiAction::Delete(app.current_face));
        }
''',
    1,
)
end = '''    app.ui_history(ui);
}
'''
replacement = '''    app.ui_history(ui);
    for action in actions {
        app.dispatch_face_ui_action(action);
    }
}
'''
if end not in text:
    raise SystemExit("faces dispatch insertion point not found")
text = text.replace(end, replacement, 1)
faces.write_text(text, encoding="utf-8")

# Navigation toolbar: button events become typed actions; orchestration moves to actions.rs.
nav = ui_dir / "project_navigation.rs"
text = nav.read_text(encoding="utf-8")
text = text.replace("use crate::*;\n", "use crate::*;\nuse super::actions::{NavigationUiAction, ProjectViewUiAction};\n", 1)
text = text.replace(
    '''    pub(crate) fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut dismiss_error = false;
''',
    '''    pub(crate) fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut actions = Vec::new();
        let mut dismiss_error = false;
''',
    1,
)
replacements = {
    "self.new_project();": "actions.push(NavigationUiAction::NewProject);",
    "self.open_project_dialog();": "actions.push(NavigationUiAction::OpenProjectDialog);",
    "self.add_faces_dialog();": "actions.push(NavigationUiAction::AddFacesDialog);",
    "self.quick_save_project();": "actions.push(NavigationUiAction::QuickSave);",
    "self.save_project(false);": "actions.push(NavigationUiAction::Save);",
    "self.save_project(true);": "actions.push(NavigationUiAction::SaveAs);",
    "self.export_current_dialog();": "actions.push(NavigationUiAction::ExportCurrent);",
    "self.export_all_dialog();": "actions.push(NavigationUiAction::ExportAll);",
    "self.validate_current_face_dialog();": "actions.push(NavigationUiAction::ValidateCurrent);",
    "self.show_previous_shades = true;": "actions.push(NavigationUiAction::ShowProjectView);",
    "self.show_settings = true;": "actions.push(NavigationUiAction::ShowSettings);",
    "self.show_about = true;": "actions.push(NavigationUiAction::ShowAbout);",
    "self.log_cache = self.log.read(); self.show_logs = true;": "actions.push(NavigationUiAction::ShowLogs);",
}
for old_value, new_value in replacements.items():
    if old_value not in text:
        raise SystemExit(f"navigation toolbar replacement missing: {old_value}")
    text = text.replace(old_value, new_value)
text = text.replace(
    '''        if inspect_requested {
            self.inspect_tiff_dialog();
        }
        if queue_requested {
            self.export.show_queue = true;
        }
        if let Some(path) = recent_requested {
            self.request_project_transition(ProjectTransition::Open(path), Some(ui.ctx()));
        }
        if dismiss_error {
            self.toast = None;
            if self.status_message == "Error - see Logs" {
                self.status_message = "Ready".to_owned();
            }
        }
''',
    '''        if inspect_requested {
            actions.push(NavigationUiAction::InspectTiff);
        }
        if queue_requested {
            actions.push(NavigationUiAction::ShowExportQueue);
        }
        if let Some(path) = recent_requested {
            actions.push(NavigationUiAction::OpenRecent(path));
        }
        if dismiss_error {
            actions.push(NavigationUiAction::DismissError);
        }
        for action in actions {
            self.dispatch_navigation_ui_action(action, ui.ctx());
        }
''',
    1,
)
# The compact queue button may have been a direct field mutation.
text = text.replace(
    '''if ui.button(queue_label).clicked() { self.export.show_queue = true; }''',
    '''if ui.button(queue_label).clicked() { actions.push(NavigationUiAction::ShowExportQueue); }''',
)

# Project View emits typed operations after rendering.
text = text.replace(
    '''    pub(crate) fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {
        if !self.show_previous_shades {
            return;
        }
''',
    '''    pub(crate) fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {
        if !self.show_previous_shades {
            return;
        }
        let mut actions = Vec::new();
''',
    1,
)
old_bottom = '''        self.show_previous_shades = open;
        if let Some(path) = requested_select {
            if self.previous_shades_selected.as_deref() != Some(path.as_str()) {
                self.load_previous_shade_preview(ctx, &path);
            }
        }
        if let Some(path) = requested_reveal {
            if let Err(err) = reveal_in_explorer(Path::new(&path)) {
                self.report_error(err);
            }
        }
        if let Some(old_path) = requested_relink {
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
        if let Some(path) = requested_remove {
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
        if let Some(path) = requested_open {
            self.request_project_transition(ProjectTransition::Open(PathBuf::from(path)), None);
        }
'''
new_bottom = '''        actions.push(ProjectViewUiAction::SetOpen(open));
        if let Some(path) = requested_select {
            actions.push(ProjectViewUiAction::Select(path));
        }
        if let Some(path) = requested_reveal {
            actions.push(ProjectViewUiAction::Reveal(path));
        }
        if let Some(path) = requested_relink {
            actions.push(ProjectViewUiAction::Relink(path));
        }
        if let Some(path) = requested_remove {
            actions.push(ProjectViewUiAction::Remove(path));
        }
        if let Some(path) = requested_open {
            actions.push(ProjectViewUiAction::Open(path));
        }
        for action in actions {
            self.dispatch_project_view_ui_action(action, ctx);
        }
'''
if old_bottom not in text:
    raise SystemExit("project view orchestration block not found")
text = text.replace(old_bottom, new_bottom, 1)
nav.write_text(text, encoding="utf-8")

# Architecture regression guard: high-risk cross-domain orchestration belongs in actions.rs.
mod_path = ui_dir / "mod.rs"
mod_text = mod_path.read_text(encoding="utf-8")
needle = '''    fn decomposed_ui_does_not_regress_back_into_application_shells() {'''
if needle not in mod_text:
    raise SystemExit("existing UI architecture test not found")
insert = r'''

    #[test]
    fn extracted_presentation_uses_typed_actions_for_cross_domain_mutations() {
        let faces = include_str!("faces.rs");
        for forbidden in [
            "app.remove_current_face()",
            "app.mark_project_dirty()",
            "relink_current_face_dialog(app)",
            "relink_missing_faces_folder_dialog(app)",
            "app.current_face =",
        ] {
            assert!(
                !faces.contains(forbidden),
                "Faces presentation bypassed typed actions with {forbidden}"
            );
        }

        let navigation = include_str!("project_navigation.rs");
        for forbidden in [
            "self.new_project();",
            "self.open_project_dialog();",
            "self.save_project(",
            "self.export_current_dialog();",
            "self.export_all_dialog();",
            "self.request_project_transition(",
            "self.validate_current_face_dialog();",
            "self.inspect_tiff_dialog();",
        ] {
            assert!(
                !navigation.contains(forbidden),
                "Project navigation bypassed typed actions with {forbidden}"
            );
        }
    }
'''
# Insert before final closing brace of tests module.
pos = mod_text.rfind("}\n")
if pos == -1:
    raise SystemExit("ui mod closing brace not found")
mod_text = mod_text[:pos] + insert + mod_text[pos:]
mod_path.write_text(mod_text, encoding="utf-8")

# Version 0.20.0.
replace_once(root / "Cargo.toml", 'version = "0.19.2"', 'version = "0.20.0"', "Cargo version")
(root / "VERSION").write_text("0.20.0\n", encoding="utf-8")
replace_once(
    root / "Cargo.lock",
    'name = "windows-shade-editor"\nversion = "0.19.2"',
    'name = "windows-shade-editor"\nversion = "0.20.0"',
    "Cargo.lock root version",
)

notes_path = root / "RELEASE_NOTES.md"
notes = notes_path.read_text(encoding="utf-8")
header = '''# Shade Editor 0.20.0

- Introduce typed Face, navigation and Project View UI actions so egui presentation code no longer directly orchestrates save/export/delete/relink/lifecycle operations.
- Centralize action dispatch in `src/ui/actions.rs` while preserving the existing project lifecycle, export, Face status, relink and autosave safety paths.
- Make project-title edits emit a typed rename action so revision-aware dirty tracking remains application-owned.
- Add architecture regression coverage that rejects high-risk direct cross-domain mutations from `ui/faces.rs` and `ui/project_navigation.rs`.
- No intended editing, color, TIFF or export behavior change.

'''
if notes.startswith("# Shade Editor 0.20.0"):
    raise SystemExit("0.20.0 release notes already exist")
notes_path.write_text(header + notes, encoding="utf-8")

(root / "docs" / "UI_ACTIONS.md").write_text(
    '''# Typed UI action boundary\n\nShade Editor's extracted egui modules render state and emit typed actions. Cross-domain operations are dispatched through `src/ui/actions.rs`, which delegates to the existing lifecycle/export/workflow safety paths.\n\n## Current action domains\n\n- `FaceUiAction` — rename/select/status/delete/relink.\n- `NavigationUiAction` — project lifecycle, save/export, queue, inspector, settings/about/logs.\n- `ProjectViewUiAction` — open/select/reveal/relink/remove Project View entries.\n\nPresentation modules may still own local widget state and read application state. They should not duplicate production safety rules or directly invoke lifecycle/export/destructive operations when a typed action exists.\n''',
    encoding="utf-8",
)

# Remove one-off validation machinery from the validated source commit.
Path(__file__).unlink()
workflow = root / ".github" / "workflows" / "apply-v020-typed-ui-actions.yml"
if workflow.exists():
    workflow.unlink()
