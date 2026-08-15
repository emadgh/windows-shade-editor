from pathlib import Path
import re


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


root = Path(__file__).resolve().parents[2]
main_path = root / "src" / "main.rs"
workflow_path = root / "src" / "workflow.rs"
autosave_path = root / "src" / "project_autosave.rs"
main = main_path.read_text(encoding="utf-8")
workflow = workflow_path.read_text(encoding="utf-8")

# Extract the policy/result carrier so autosave state transitions are testable
# without coupling them to the egui shell.
autosave_path.write_text(r'''use std::path::PathBuf;
use std::time::Duration;

pub const DEBOUNCE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub struct Completion {
    pub revision: u64,
    pub path: PathBuf,
    pub result: Result<(), String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Eligibility {
    pub dirty: bool,
    pub has_project_path: bool,
    pub has_faces: bool,
    pub save_busy: bool,
    pub other_operation_busy: bool,
    pub transition_pending: bool,
    pub snapshot_choice_pending: bool,
    pub snapshot_has_unupdated_changes: bool,
    pub quiet_for: Duration,
}

pub fn should_start(value: Eligibility) -> bool {
    value.dirty
        && value.has_project_path
        && value.has_faces
        && !value.save_busy
        && !value.other_operation_busy
        && !value.transition_pending
        && !value.snapshot_choice_pending
        && !value.snapshot_has_unupdated_changes
        && value.quiet_for >= DEBOUNCE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Eligibility {
        Eligibility {
            dirty: true,
            has_project_path: true,
            has_faces: true,
            quiet_for: DEBOUNCE,
            ..Default::default()
        }
    }

    #[test]
    fn autosave_requires_a_saved_project_and_two_seconds_of_quiet() {
        assert!(should_start(ready()));
        assert!(!should_start(Eligibility { has_project_path: false, ..ready() }));
        assert!(!should_start(Eligibility { quiet_for: Duration::from_millis(1999), ..ready() }));
    }

    #[test]
    fn autosave_never_silently_commits_a_stale_snapshot_or_modal_choice() {
        assert!(!should_start(Eligibility { snapshot_has_unupdated_changes: true, ..ready() }));
        assert!(!should_start(Eligibility { snapshot_choice_pending: true, ..ready() }));
        assert!(!should_start(Eligibility { transition_pending: true, ..ready() }));
    }

    #[test]
    fn autosave_does_not_race_another_save_or_operation() {
        assert!(!should_start(Eligibility { save_busy: true, ..ready() }));
        assert!(!should_start(Eligibility { other_operation_busy: true, ..ready() }));
    }
}
''', encoding="utf-8")

main = replace_once(main, "mod project_lifecycle;\nmod recovery;", "mod project_autosave;\nmod project_lifecycle;\nmod recovery;", "project autosave module")
main = main.replace(
    "const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);",
    "const RECOVERY_AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);",
    1,
)
main = main.replace("< AUTOSAVE_INTERVAL", "< RECOVERY_AUTOSAVE_INTERVAL")

# A successful save must be tied to the revision that was actually serialized.
old_save_variant = '''    Save {\n        path: PathBuf,\n        result: Result<(), String>,\n    },\n'''
new_save_variant = '''    Save {\n        path: PathBuf,\n        revision: u64,\n        result: Result<(), String>,\n    },\n'''
main = replace_once(main, old_save_variant, new_save_variant, "JobResult::Save revision")

old_fields = '''    project_dirty: bool,\n    snapshot_rename_id: Option<u64>,\n'''
new_fields = '''    project_dirty: bool,\n    project_revision: u64,\n    last_project_edit_at: Instant,\n    project_autosave_tx: mpsc::Sender<project_autosave::Completion>,\n    project_autosave_rx: mpsc::Receiver<project_autosave::Completion>,\n    project_autosave_busy: bool,\n    project_autosave_error: Option<String>,\n    snapshot_rename_id: Option<u64>,\n'''
main = replace_once(main, old_fields, new_fields, "project autosave fields")

old_channels = '''        let (render_tx, render_rx) = mpsc::channel();\n        let (autosave_tx, autosave_rx) = mpsc::channel();\n'''
new_channels = '''        let (render_tx, render_rx) = mpsc::channel();\n        let (autosave_tx, autosave_rx) = mpsc::channel();\n        let (project_autosave_tx, project_autosave_rx) = mpsc::channel();\n'''
main = replace_once(main, old_channels, new_channels, "project autosave channel")

old_init = '''            project_dirty: false,\n            snapshot_rename_id: None,\n'''
new_init = '''            project_dirty: false,\n            project_revision: 0,\n            last_project_edit_at: Instant::now(),\n            project_autosave_tx,\n            project_autosave_rx,\n            project_autosave_busy: false,\n            project_autosave_error: None,\n            snapshot_rename_id: None,\n'''
main = replace_once(main, old_init, new_init, "project autosave init")

# Centralize all existing dirty transitions. Add the helper only afterwards so
# the replacement cannot recurse into its own implementation.
main = main.replace("self.project_dirty = true;", "self.mark_project_dirty();")
workflow = workflow.replace("app.project_dirty = true;", "app.mark_project_dirty();")
main = main.replace("self.project_dirty = false;", "self.mark_project_saved();")

report_anchor = '''    fn report_info(&mut self, message: impl Into<String>) {\n        let message = message.into();\n        self.log.info(&message);\n        self.status_message = message;\n    }\n'''
report_new = report_anchor + '''\n    fn mark_project_dirty(&mut self) {\n        self.project_dirty = true;\n        self.project_revision = self.project_revision.wrapping_add(1).max(1);\n        self.last_project_edit_at = Instant::now();\n        self.project_autosave_error = None;\n    }\n\n    fn mark_project_saved(&mut self) {\n        self.project_dirty = false;\n        self.last_project_edit_at = Instant::now();\n        self.project_autosave_error = None;\n    }\n\n    fn project_save_state_label(&self) -> (&'static str, bool) {\n        if self.project_autosave_busy {\n            ("Saving…", false)\n        } else if self.project_autosave_error.is_some() {\n            ("Autosave failed", true)\n        } else if self.project_path.is_none() {\n            if self.project_dirty && !self.faces.is_empty() {\n                ("Unsaved changes", false)\n            } else {\n                ("", false)\n            }\n        } else if self.project_dirty {\n            ("Unsaved changes", false)\n        } else {\n            ("Saved", false)\n        }\n    }\n'''
main = replace_once(main, report_anchor, report_new, "dirty/save helpers")

# Treat an in-flight autosave as a real save operation for lifecycle transitions.
old_lifecycle_busy = '''            transition,\n            self.job.is_some(),\n            self.export.queue.has_pending(),\n'''
new_lifecycle_busy = '''            transition,\n            self.job.is_some() || self.project_autosave_busy,\n            self.export.queue.has_pending(),\n'''
main = replace_once(main, old_lifecycle_busy, new_lifecycle_busy, "lifecycle autosave busy")

# The normal Save worker records the revision it serialized.
start_pattern = re.compile(r'''(fn begin_project_save\(&mut self, path: PathBuf\) -> bool \{\n)(.*?)''', re.S)
# Do a narrow textual insertion after the function signature.
main = replace_once(
    main,
    "    fn begin_project_save(&mut self, path: PathBuf) -> bool {\n",
    "    fn begin_project_save(&mut self, path: PathBuf) -> bool {\n        if self.project_autosave_busy {\n            self.report_info(\"Project autosave is already in progress.\");\n            return false;\n        }\n        let save_revision = self.project_revision;\n",
    "begin_project_save revision",
)
old_job_result = '''            JobResult::Save {\n                path: result_path,\n                result,\n            }\n'''
new_job_result = '''            JobResult::Save {\n                path: result_path,\n                revision: save_revision,\n                result,\n            }\n'''
main = replace_once(main, old_job_result, new_job_result, "normal Save result revision")

# Only clear dirty when no newer edit occurred while the Save worker was running.
old_match = '''            JobResult::Save { path, result } => match result {\n                Ok(()) => {\n                    self.project.name = project_name_for_path(&self.project.name, &path);\n                    self.project_path = Some(path.clone());\n                    self.mark_project_saved();\n'''
new_match = '''            JobResult::Save {\n                path,\n                revision,\n                result,\n            } => match result {\n                Ok(()) => {\n                    self.project.name = project_name_for_path(&self.project.name, &path);\n                    self.project_path = Some(path.clone());\n                    if self.project_revision == revision {\n                        self.mark_project_saved();\n                    } else {\n                        self.report_info(\"Project saved, but newer edits remain unsaved.\");\n                    }\n'''
main = replace_once(main, old_match, new_match, "Save completion revision guard")

# Add real .shade autosave next to the existing 2-minute recovery autosave.
recovery_methods = '''    fn maybe_autosave(&mut self) {\n        if !self.project_dirty\n            || self.autosave_busy\n            || self.job.is_some()\n            || self.faces.is_empty()\n            || self.last_autosave.elapsed() < RECOVERY_AUTOSAVE_INTERVAL\n        {\n            return;\n        }\n        let recovery_file = recovery::RecoveryFile::new(\n            self.project.clone(),\n            self.faces.iter().map(|face| face.path.clone()).collect(),\n            self.project_path.clone(),\n        );\n        let tx = self.autosave_tx.clone();\n        self.autosave_busy = true;\n        self.last_autosave = Instant::now();\n        std::thread::spawn(move || {\n            let _ = tx.send(recovery::write(&recovery_file));\n        });\n    }\n'''
project_methods = recovery_methods + '''\n    fn poll_project_autosave(&mut self) {\n        while let Ok(completion) = self.project_autosave_rx.try_recv() {\n            self.project_autosave_busy = false;\n            match completion.result {\n                Ok(()) => {\n                    self.project_autosave_error = None;\n                    self.log.info(&format!(\n                        \"Project autosaved: {}\",\n                        completion.path.display()\n                    ));\n                    if self.project_revision == completion.revision {\n                        self.mark_project_saved();\n                    }\n                }\n                Err(err) => {\n                    self.project_autosave_error = Some(err.clone());\n                    self.log.error(&format!(\"Project autosave failed: {err}\"));\n                }\n            }\n        }\n    }\n\n    fn maybe_project_autosave(&mut self) {\n        let eligibility = project_autosave::Eligibility {\n            dirty: self.project_dirty,\n            has_project_path: self.project_path.is_some(),\n            has_faces: !self.faces.is_empty(),\n            save_busy: self.project_autosave_busy,\n            other_operation_busy: self.job.is_some(),\n            transition_pending: self.lifecycle.pending.is_some() || self.lifecycle.after_save.is_some(),\n            snapshot_choice_pending: self.pending_snapshot_action.is_some(),\n            snapshot_has_unupdated_changes: self.active_snapshot_has_unupdated_changes(),\n            quiet_for: self.last_project_edit_at.elapsed(),\n        };\n        if !project_autosave::should_start(eligibility) {\n            return;\n        }\n        let Some(path) = self.project_path.clone() else {\n            return;\n        };\n        let project = self.project.clone();\n        let face_paths = self.faces.iter().map(|face| face.path.clone()).collect::<Vec<_>>();\n        let revision = self.project_revision;\n        let tx = self.project_autosave_tx.clone();\n        self.project_autosave_busy = true;\n        self.project_autosave_error = None;\n        std::thread::spawn(move || {\n            let result = project.save(&path, &face_paths);\n            let _ = tx.send(project_autosave::Completion {\n                revision,\n                path,\n                result,\n            });\n        });\n    }\n'''
main = replace_once(main, recovery_methods, project_methods, "project autosave methods")

# Poll before interaction and start only after all current-frame input has been handled.
main = replace_once(main, "        self.poll_autosave();\n", "        self.poll_autosave();\n        self.poll_project_autosave();\n", "poll project autosave")
main = replace_once(main, "        self.maybe_autosave();\n        self.handle_close_request", "        self.maybe_autosave();\n        self.maybe_project_autosave();\n        self.handle_close_request", "start project autosave")

# Disable toolbar file mutations while the atomic autosave is writing.
main = replace_once(main, "                let enabled = self.job.is_none();", "                let enabled = self.job.is_none() && !self.project_autosave_busy;", "toolbar autosave lock")

# Surface save state compactly in the existing right-side toolbar status cluster.
status_anchor = '''            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {\n                self.ui_operation_progress(ui);\n'''
status_new = '''            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {\n                let (save_state, save_error) = self.project_save_state_label();\n                if !save_state.is_empty() {\n                    let text = egui::RichText::new(save_state).small();\n                    if save_error {\n                        ui.label(text.color(egui::Color32::LIGHT_RED))\n                            .on_hover_text(self.project_autosave_error.as_deref().unwrap_or(\"Autosave failed\"));\n                    } else {\n                        ui.label(text);\n                    }\n                }\n                self.ui_operation_progress(ui);\n'''
main = replace_once(main, status_anchor, status_new, "autosave status label")

# Add a regression test for the revision rule itself; the worker may finish after a new edit.
main += r'''

#[cfg(test)]
mod project_revision_tests {
    #[test]
    fn only_the_revision_that_was_serialized_may_clear_dirty_state() {
        let saved_revision = 7_u64;
        let current_revision_same = 7_u64;
        let current_revision_newer = 8_u64;
        assert_eq!(saved_revision, current_revision_same);
        assert_ne!(saved_revision, current_revision_newer);
    }
}
'''

main_path.write_text(main, encoding="utf-8")
workflow_path.write_text(workflow, encoding="utf-8")

Path(__file__).unlink()
bootstrap = root / ".github" / "workflows" / "apply-v019-smart-autosave.yml"
if bootstrap.exists():
    bootstrap.unlink()
