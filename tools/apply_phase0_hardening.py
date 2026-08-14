from pathlib import Path
import re


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def sub_once(text: str, pattern: str, repl: str, label: str, flags=0) -> str:
    out, count = re.subn(pattern, repl, text, count=1, flags=flags)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one regex match, found {count}")
    return out


path = Path("src/main.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    "mod palette;\nmod previous_shades;\nmod recovery;\n",
    "mod palette;\nmod path_safety;\nmod previous_shades;\nmod project_lifecycle;\nmod recovery;\n",
    "module wiring",
)
text = replace_once(
    text,
    "use palette::ChannelPalette;\nuse settings::AppSettings;\n",
    "use palette::ChannelPalette;\nuse project_lifecycle::ProjectTransition;\nuse settings::AppSettings;\n",
    "lifecycle import",
)

text = replace_once(
    text,
    "    pending_snapshot_load: Option<u64>,\n    show_new_confirmation: bool,\n    new_after_save: bool,\n    show_close_confirmation: bool,\n    close_after_save: bool,\n    allow_close_once: bool,\n",
    "    pending_snapshot_load: Option<u64>,\n    pending_transition: Option<ProjectTransition>,\n    transition_after_save: Option<ProjectTransition>,\n    allow_close_once: bool,\n    project_session_id: u64,\n",
    "ShadeApp lifecycle fields",
)
text = replace_once(
    text,
    "            pending_snapshot_load: None,\n            show_new_confirmation: false,\n            new_after_save: false,\n            show_close_confirmation: false,\n            close_after_save: false,\n            allow_close_once: false,\n",
    "            pending_snapshot_load: None,\n            pending_transition: None,\n            transition_after_save: None,\n            allow_close_once: false,\n            project_session_id: 1,\n",
    "ShadeApp lifecycle defaults",
)

new_lifecycle_block = r'''    fn new_project(&mut self) {
        self.request_project_transition(ProjectTransition::New, None);
    }

    fn request_project_transition(
        &mut self,
        transition: ProjectTransition,
        ctx: Option<&egui::Context>,
    ) {
        if self.job.is_some() {
            self.report_info("Finish the current operation before changing projects.");
            return;
        }
        if self.export_queue.has_pending() {
            self.show_export_queue = true;
            self.report_info(
                "Finish or cancel the Export Queue before changing projects or exiting.",
            );
            return;
        }
        if project_lifecycle::requires_save_confirmation(
            self.project_dirty,
            !self.faces.is_empty(),
            self.project_path.is_some(),
        ) {
            self.pending_transition = Some(transition);
            return;
        }
        self.execute_project_transition(transition, ctx);
    }

    fn execute_project_transition(
        &mut self,
        transition: ProjectTransition,
        ctx: Option<&egui::Context>,
    ) {
        match transition {
            ProjectTransition::New => self.reset_to_new_project(),
            ProjectTransition::Open(path) => {
                self.show_previous_shades = false;
                self.open_project_path(path);
            }
            ProjectTransition::Exit => {
                if let Some(ctx) = ctx {
                    self.allow_close_once = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.pending_transition = Some(ProjectTransition::Exit);
                }
            }
            ProjectTransition::Recover => self.recover_project_now(),
        }
    }

    fn complete_transition_after_save(&mut self, ctx: &egui::Context) {
        if self.job.is_some() || self.project_dirty {
            return;
        }
        let Some(transition) = self.transition_after_save.take() else {
            return;
        };
        self.execute_project_transition(transition, Some(ctx));
    }

    fn bump_project_session(&mut self) {
        self.project_session_id = self.project_session_id.wrapping_add(1).max(1);
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
        self.pending_transition = None;
        self.transition_after_save = None;
        self.show_color_management = false;
        self.icc_profile_query.clear();
        self.icc_profile_selected = None;
        self.remind_after_export = false;
        self.show_snapshot_save_reminder = false;
        self.history.reset(&self.project.adjustments, "New project");
        self.history_clear_backup = None;
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.bump_project_session();
        self.report_info("New shade project");
    }

'''
text = sub_once(
    text,
    r"    fn new_project\(&mut self\) \{.*?\n    fn make_runtime_face",
    new_lifecycle_block + "    fn make_runtime_face",
    "central lifecycle implementation",
    re.S,
)

text = replace_once(
    text,
    "        self.open_project_path(path);\n    }\n\n    fn open_project_path",
    "        self.request_project_transition(ProjectTransition::Open(path), None);\n    }\n\n    fn open_project_path",
    "Open dialog guard",
)
text = replace_once(
    text,
    "        if let Some(path) = requested_open {\n            self.show_previous_shades = false;\n            self.open_project_path(PathBuf::from(path));\n        }\n",
    "        if let Some(path) = requested_open {\n            self.request_project_transition(\n                ProjectTransition::Open(PathBuf::from(path)),\n                None,\n            );\n        }\n",
    "Previous Shades open guard",
)

text = replace_once(
    text,
    "    fn recover_project(&mut self) {\n        if self.job.is_some() {\n",
    "    fn recover_project(&mut self) {\n        self.request_project_transition(ProjectTransition::Recover, None);\n    }\n\n    fn recover_project_now(&mut self) {\n        if self.job.is_some() {\n",
    "Recovery transition guard",
)

text = replace_once(
    text,
    "                Ok(payload) => {\n                    self.project = payload.project;\n                    self.snapshot_rename_id = None;\n",
    "                Ok(payload) => {\n                    self.project = payload.project;\n                    self.bump_project_session();\n                    self.snapshot_rename_id = None;\n",
    "Open session identity",
)
text = replace_once(
    text,
    "                Ok(payload) => {\n                    self.project = payload.project;\n                    self.project_path = payload.origin_path;\n",
    "                Ok(payload) => {\n                    self.project = payload.project;\n                    self.bump_project_session();\n                    self.project_path = payload.origin_path;\n",
    "Recovery session identity",
)

old_save = r'''            JobResult::Save { path, result } => match result {
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
new_save = r'''            JobResult::Save { path, result } => match result {
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
                    if let Some(transition) = self.transition_after_save.take() {
                        self.pending_transition = Some(transition);
                    }
                    self.report_error(err);
                }
            },
'''
text = replace_once(text, old_save, new_save, "Save continuation semantics")

transition_ui = r'''    fn ui_project_transition_confirmation(&mut self, ctx: &egui::Context) {
        let Some(transition) = self.pending_transition.clone() else {
            return;
        };
        let action = transition.action_label();
        let mut save_and_continue = false;
        let mut discard_and_continue = false;
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
                ui.label(format!(
                    "Shade Editor is about to {}. Save the current .shade project first?",
                    transition.verb()
                ));
                ui.label("Faces, Snapshots and adjustment state remain untouched unless Save succeeds or Discard is explicit.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    save_and_continue = ui
                        .add_enabled(
                            self.job.is_none() && !self.faces.is_empty(),
                            egui::Button::new(format!("Save and {action}")),
                        )
                        .clicked();
                    discard_and_continue = ui
                        .button(format!("Discard and {action}"))
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });

        if cancel {
            self.pending_transition = None;
            self.transition_after_save = None;
        } else if discard_and_continue {
            self.pending_transition = None;
            self.transition_after_save = None;
            if !matches!(transition, ProjectTransition::Recover) {
                if let Err(err) = recovery::clear() {
                    self.log.error(&err);
                }
            }
            self.execute_project_transition(transition, Some(ctx));
        } else if save_and_continue {
            self.pending_transition = None;
            self.transition_after_save = Some(transition.clone());
            if !self.save_project(false) {
                self.transition_after_save = None;
                self.pending_transition = Some(transition);
            }
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        if self.allow_close_once {
            self.allow_close_once = false;
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.request_project_transition(ProjectTransition::Exit, Some(ctx));
    }

'''
text = sub_once(
    text,
    r"    fn ui_new_project_confirmation\(&mut self, ctx: &egui::Context\) \{.*?\n    fn sync_history_to_active_snapshot",
    transition_ui + "    fn sync_history_to_active_snapshot",
    "unified transition dialog and close guard",
    re.S,
)

# Queue wrapper centralizes project ownership and protects every project Face source.
marker = r'''    fn snapshot_project_needs_save_reminder(&self) -> bool {
        self.project.active_snapshot_id.is_some()
            && (self.project_dirty || self.project_path.is_none())
    }

'''
queue_helper = marker + r'''    fn enqueue_export(&mut self, spec: export_queue::ExportQueueSpec) -> bool {
        let protected_sources = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        match self.export_queue.enqueue_for_project(
            spec,
            protected_sources,
            self.project_session_id,
        ) {
            Ok(_) => true,
            Err(err) => {
                self.report_error(err);
                false
            }
        }
    }

'''
text = replace_once(text, marker, queue_helper, "queue wrapper")

# Add runtime conflict policy to all four queue specs, then route through protected wrapper.
text = replace_once(
    text,
    "            validate_after_export: self.settings.validate_after_export,\n            mark: None,\n        });\n        self.show_export_queue = true;",
    "            validate_after_export: self.settings.validate_after_export,\n            conflict_policy: export_batch::ConflictPolicy::Overwrite,\n            mark: None,\n        }) {\n            return;\n        }\n        self.show_export_queue = true;",
    "current export policy/body",
)
text = text.replace(
    "        self.export_queue.enqueue(export_queue::ExportQueueSpec {\n            label: format!(\"{face_name} / {state_name}\"),",
    "        if !self.enqueue_export(export_queue::ExportQueueSpec {\n            label: format!(\"{face_name} / {state_name}\"),",
    1,
)

text = replace_once(
    text,
    "                validate_after_export: self.settings.validate_after_export,\n                mark: None,\n            });\n            queued += 1;",
    "                validate_after_export: self.settings.validate_after_export,\n                conflict_policy,\n                mark: None,\n            }) {\n                return;\n            }\n            queued += 1;",
    "Export All queue policy/body",
)
text = text.replace(
    "            self.export_queue.enqueue(export_queue::ExportQueueSpec {\n                label: format!(\"{face_name} / {snapshot_code}\"),",
    "            if !self.enqueue_export(export_queue::ExportQueueSpec {\n                label: format!(\"{face_name} / {snapshot_code}\"),",
    1,
)

text = replace_once(
    text,
    "            validate_after_export: self.settings.validate_after_export,\n            mark: Some(export_queue::ExportQueueMark {\n                snapshot_id,",
    "            validate_after_export: self.settings.validate_after_export,\n            conflict_policy: export_batch::ConflictPolicy::Overwrite,\n            mark: Some(export_queue::ExportQueueMark {\n                snapshot_id,",
    "single Snapshot policy",
)
text = replace_once(
    text,
    "        self.export_queue.enqueue(export_queue::ExportQueueSpec {\n            label: format!(\"Face {} / {}\", self.current_face + 1, snapshot.name),",
    "        if !self.enqueue_export(export_queue::ExportQueueSpec {\n            label: format!(\"Face {} / {}\", self.current_face + 1, snapshot.name),",
    "single Snapshot wrapper",
)
text = replace_once(
    text,
    "            }),\n        });\n        self.show_export_queue = true;\n        self.report_info(\"Snapshot export added to queue\");",
    "            }),\n        }) {\n            return;\n        }\n        self.show_export_queue = true;\n        self.report_info(\"Snapshot export added to queue\");",
    "single Snapshot wrapper close",
)

# Snapshot group: use same conflict policy as configured Export All behavior.
text = replace_once(
    text,
    "        let date = Local::now().format(\"%Y-%m-%d\").to_string();\n        let mut reserved = BTreeSet::new();\n        let mut queued = 0usize;\n        let mut skipped = 0usize;\n\n        for snapshot in snapshots {",
    "        let date = Local::now().format(\"%Y-%m-%d\").to_string();\n        let mut reserved = self.export_queue.reserved_destination_keys();\n        let conflict_policy = self.settings.export_all_conflict_policy;\n        let mut queued = 0usize;\n        let mut skipped = 0usize;\n\n        for snapshot in snapshots {",
    "Snapshot group reservations",
)
text = replace_once(
    text,
    "                validate_after_export: self.settings.validate_after_export,\n                mark: Some(export_queue::ExportQueueMark {\n                    snapshot_id: snapshot.id,",
    "                validate_after_export: self.settings.validate_after_export,\n                conflict_policy,\n                mark: Some(export_queue::ExportQueueMark {\n                    snapshot_id: snapshot.id,",
    "Snapshot group policy",
)
text = replace_once(
    text,
    "            self.export_queue.enqueue(export_queue::ExportQueueSpec {\n                label: format!(\"{face_name} / {}\", snapshot.name),",
    "            if !self.enqueue_export(export_queue::ExportQueueSpec {\n                label: format!(\"{face_name} / {}\", snapshot.name),",
    "Snapshot group wrapper",
)
text = replace_once(
    text,
    "                }),\n            });\n            queued += 1;\n        }\n\n        if queued > 0 {",
    "                }),\n            }) {\n                return;\n            }\n            queued += 1;\n        }\n\n        if queued > 0 {",
    "Snapshot group wrapper close",
)

# Export All reservations must include jobs already waiting in the global queue.
text = replace_once(
    text,
    "        let date = Local::now().format(\"%Y-%m-%d\").to_string();\n        let mut reserved = BTreeSet::new();\n        let mut queued = 0usize;",
    "        let date = Local::now().format(\"%Y-%m-%d\").to_string();\n        let mut reserved = self.export_queue.reserved_destination_keys();\n        let mut queued = 0usize;",
    "Export All global reservations",
)

# A completion belongs only to the project session that enqueued it.
text = replace_once(
    text,
    "        for completion in completions {\n            if let Some(mark) = completion.mark {",
    "        for completion in completions {\n            if completion.project_session_id != self.project_session_id {\n                self.log.info(&format!(\n                    \"Ignored queue completion #{} from previous project session {}\",\n                    completion.id, completion.project_session_id\n                ));\n                continue;\n            }\n            if let Some(mark) = completion.mark {",
    "queue completion project ownership",
)

# Remove legacy post-save close continuation and use typed continuation.
text = replace_once(
    text,
    "        self.poll_job();\n        self.poll_export_queue();",
    "        self.poll_job();\n        self.complete_transition_after_save(ui.ctx());\n        self.poll_export_queue();",
    "post-save typed transition",
)
text = sub_once(
    text,
    r"        self\.handle_close_request\(ui\.ctx\(\)\);\n        if self\.close_after_save.*?\n        \}\n\n        egui::Panel::top",
    "        self.handle_close_request(ui.ctx());\n\n        egui::Panel::top",
    "remove legacy close continuation",
    re.S,
)
text = replace_once(
    text,
    "        self.ui_new_project_confirmation(ui.ctx());\n        self.ui_close_confirmation(ui.ctx());\n",
    "        self.ui_project_transition_confirmation(ui.ctx());\n",
    "transition UI call",
)

# Remove legacy helper/tests now owned by project_lifecycle.rs.
text = sub_once(
    text,
    r"\nfn should_confirm_new_project\(.*?\nfn project_name_for_path",
    "\nfn project_name_for_path",
    "remove legacy New guard helper",
    re.S,
)

path.write_text(text, encoding="utf-8")
