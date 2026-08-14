from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


main_path = Path("src/main.rs")
text = main_path.read_text(encoding="utf-8")

text = replace_once(
    text,
    "    pending_snapshot_load: Option<u64>,\n    show_close_confirmation: bool,\n",
    "    pending_snapshot_load: Option<u64>,\n    show_new_confirmation: bool,\n    new_after_save: bool,\n    show_close_confirmation: bool,\n",
    "ShadeApp pending-new fields",
)

text = replace_once(
    text,
    "            pending_snapshot_load: None,\n            show_close_confirmation: false,\n",
    "            pending_snapshot_load: None,\n            show_new_confirmation: false,\n            new_after_save: false,\n            show_close_confirmation: false,\n",
    "ShadeApp pending-new defaults",
)

old_new_project = '''    fn new_project(&mut self) {
        if self.job.is_some() {
            return;
        }
        self.project = ShadeProject::default();
        self.project.channel_palette = self.settings.default_project_palette();
        self.project_path = None;
        self.faces.clear();
        self.current_face = 0;
        self.selected_channel = 0;
        self.solo_channel = None;
        self.adjustment_scope = AdjustmentScope::All;
        self.viewport_recenter = true;
        self.fit_requested = true;
        self.project_dirty = false;
        self.snapshot_rename_id = None;
        self.snapshot_rename_buffer.clear();
        self.pending_snapshot_load = None;
        self.show_close_confirmation = false;
        self.close_after_save = false;
        self.show_color_management = false;
        self.icc_profile_query.clear();
        self.icc_profile_selected = None;
        self.remind_after_export = false;
        self.show_snapshot_save_reminder = false;
        self.history.reset(&self.project.adjustments, "New project");
        self.history_clear_backup = None;
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.report_info("New shade project");
    }
'''

new_new_project = '''    fn new_project(&mut self) {
        if self.job.is_some() {
            return;
        }
        if should_confirm_new_project(
            self.project_dirty,
            !self.faces.is_empty(),
            self.project_path.is_some(),
        ) {
            self.show_new_confirmation = true;
            return;
        }
        self.reset_to_new_project();
    }

    fn reset_to_new_project(&mut self) {
        self.project = ShadeProject::default();
        self.project.channel_palette = self.settings.default_project_palette();
        self.project_path = None;
        self.faces.clear();
        self.current_face = 0;
        self.selected_channel = 0;
        self.solo_channel = None;
        self.adjustment_scope = AdjustmentScope::All;
        self.viewport_recenter = true;
        self.fit_requested = true;
        self.project_dirty = false;
        self.snapshot_rename_id = None;
        self.snapshot_rename_buffer.clear();
        self.pending_snapshot_load = None;
        self.show_new_confirmation = false;
        self.new_after_save = false;
        self.show_close_confirmation = false;
        self.close_after_save = false;
        self.show_color_management = false;
        self.icc_profile_query.clear();
        self.icc_profile_selected = None;
        self.remind_after_export = false;
        self.show_snapshot_save_reminder = false;
        self.history.reset(&self.project.adjustments, "New project");
        self.history_clear_backup = None;
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.report_info("New shade project");
    }
'''
text = replace_once(text, old_new_project, new_new_project, "new_project implementation")

old_save_ok = '''            JobResult::Save { path, result } => match result {
                Ok(()) => {
                    self.project.name = project_name_for_path(&self.project.name, &path);
                    self.project_path = Some(path.clone());
                    self.project_dirty = false;
                    self.remind_after_export = false;
                    self.show_snapshot_save_reminder = false;
                    self.remember_previous_shade(&path);
                    if let Err(err) = recovery::clear() {
                        self.log.error(&err);
                    }
                    self.report_info(format!("Saved {}", path.display()));
                }
                Err(err) => {
                    self.close_after_save = false;
                    self.report_error(err);
                }
            },
'''
new_save_ok = '''            JobResult::Save { path, result } => match result {
                Ok(()) => {
                    let create_new_after_save = self.new_after_save;
                    self.new_after_save = false;
                    self.project.name = project_name_for_path(&self.project.name, &path);
                    self.project_path = Some(path.clone());
                    self.project_dirty = false;
                    self.remind_after_export = false;
                    self.show_snapshot_save_reminder = false;
                    self.remember_previous_shade(&path);
                    if let Err(err) = recovery::clear() {
                        self.log.error(&err);
                    }
                    self.report_info(format!("Saved {}", path.display()));
                    if create_new_after_save {
                        self.reset_to_new_project();
                    }
                }
                Err(err) => {
                    if self.new_after_save {
                        self.new_after_save = false;
                        self.show_new_confirmation = true;
                    }
                    self.close_after_save = false;
                    self.report_error(err);
                }
            },
'''
text = replace_once(text, old_save_ok, new_save_ok, "save completion continuation")

new_confirmation_fn = '''    fn ui_new_project_confirmation(&mut self, ctx: &egui::Context) {
        if !self.show_new_confirmation {
            return;
        }

        let mut save_and_new = false;
        let mut discard_and_new = false;
        let mut cancel = false;
        egui::Window::new("Save current project?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                if self.project_path.is_some() {
                    ui.strong("The current project has unsaved changes.");
                } else {
                    ui.strong("The current project has not been saved yet.");
                }
                ui.label("Creating a new project will remove the current Faces, Snapshots and adjustment state from the editor.");
                ui.label("Save the current .shade project before continuing?");
                if self.job.is_some() {
                    ui.small("Wait for the current operation to finish before saving.");
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    save_and_new = ui
                        .add_enabled(
                            self.job.is_none() && !self.faces.is_empty(),
                            egui::Button::new("Save and create new"),
                        )
                        .clicked();
                    discard_and_new = ui.button("Discard and create new").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });

        if cancel {
            self.show_new_confirmation = false;
            self.new_after_save = false;
        } else if discard_and_new {
            self.show_new_confirmation = false;
            self.new_after_save = false;
            if let Err(err) = recovery::clear() {
                self.log.error(&err);
            }
            self.reset_to_new_project();
        } else if save_and_new {
            self.new_after_save = true;
            if self.save_project(false) {
                self.show_new_confirmation = false;
            } else {
                self.new_after_save = false;
            }
        }
    }

'''
text = replace_once(
    text,
    "    fn handle_close_request(&mut self, ctx: &egui::Context) {\n",
    new_confirmation_fn + "    fn handle_close_request(&mut self, ctx: &egui::Context) {\n",
    "new-project confirmation UI insertion",
)

text = replace_once(
    text,
    "        self.ui_snapshot_save_reminder(ui.ctx());\n        self.ui_close_confirmation(ui.ctx());\n",
    "        self.ui_snapshot_save_reminder(ui.ctx());\n        self.ui_new_project_confirmation(ui.ctx());\n        self.ui_close_confirmation(ui.ctx());\n",
    "new-project confirmation UI call",
)

helper = '''fn should_confirm_new_project(project_dirty: bool, has_faces: bool, has_saved_path: bool) -> bool {
    project_dirty || (has_faces && !has_saved_path)
}

#[cfg(test)]
mod new_project_guard_tests {
    use super::should_confirm_new_project;

    #[test]
    fn dirty_project_always_requires_confirmation() {
        assert!(should_confirm_new_project(true, true, true));
        assert!(should_confirm_new_project(true, false, true));
    }

    #[test]
    fn unsaved_project_with_faces_requires_confirmation_even_if_clean_flag_is_false() {
        assert!(should_confirm_new_project(false, true, false));
    }

    #[test]
    fn clean_saved_or_empty_project_can_be_replaced_without_prompt() {
        assert!(!should_confirm_new_project(false, true, true));
        assert!(!should_confirm_new_project(false, false, false));
    }
}

'''
text = replace_once(
    text,
    "fn project_name_for_path(current: &str, path: &Path) -> String {\n",
    helper + "fn project_name_for_path(current: &str, path: &Path) -> String {\n",
    "new-project guard helper/tests",
)

main_path.write_text(text, encoding="utf-8")

cargo_path = Path("Cargo.toml")
cargo = cargo_path.read_text(encoding="utf-8")
cargo = replace_once(cargo, 'version = "0.17.1"', 'version = "0.17.2"', "Cargo.toml version")
cargo_path.write_text(cargo, encoding="utf-8")

lock_path = Path("Cargo.lock")
lock = lock_path.read_text(encoding="utf-8")
lock = replace_once(
    lock,
    'name = "windows-shade-editor"\nversion = "0.17.1"',
    'name = "windows-shade-editor"\nversion = "0.17.2"',
    "Cargo.lock package version",
)
lock_path.write_text(lock, encoding="utf-8")

notes_path = Path("RELEASE_NOTES.md")
notes = notes_path.read_text(encoding="utf-8")
notes = """# Shade Editor 0.17.2

- Protect `New project` from destructive state loss: dirty projects and unsaved projects with Faces now require an explicit Save / Discard / Cancel decision.
- `Save and create new` waits for the asynchronous `.shade` save to complete successfully before resetting the editor; save cancellation or failure keeps the current project intact.
- Clear recovery state only after an explicit discard, successful save, or normal safe project transition.

""" + notes
notes_path.write_text(notes, encoding="utf-8")
