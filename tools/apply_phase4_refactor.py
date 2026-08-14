from pathlib import Path
import re


def once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1 match, found {count}")
    return text.replace(old, new, 1)

p = Path("src/main.rs")
t = p.read_text(encoding="utf-8")

# Compile the controller and feature-wiring modules.
t = once(t, "mod app_log;\nmod color_management;", "mod app_controllers;\nmod app_features;\nmod app_log;\nmod color_management;", "controller modules")
t = once(
    t,
    "use chrono::{Local, TimeZone};\nuse color_management::{InstalledIccProfile, PreviewColorConfig, PreviewColorStatus};",
    "use app_controllers::{ColorManagementController, ExportController, TiffInspectorController};\nuse chrono::{Local, TimeZone};\nuse color_management::{PreviewColorConfig, PreviewColorStatus};",
    "controller imports",
)
t = once(
    t,
    "use project_lifecycle::ProjectTransition;",
    "use project_lifecycle::{\n    ProjectLifecycleController, ProjectTransition, TransitionRequest,\n};",
    "lifecycle controller imports",
)

# Backup restore candidate now belongs to lifecycle controller.
t = once(
    t,
    '''#[derive(Clone)]
struct BackupRestoreCandidate {
    primary_path: PathBuf,
    backup_path: PathBuf,
    primary_error: String,
}

''',
    "",
    "remove local backup candidate",
)

# Consolidate controller-owned state in ShadeApp.
t = once(
    t,
    '''    show_settings: bool,
    show_color_management: bool,
    icc_profile_query: String,
    icc_profiles: Vec<InstalledIccProfile>,
    icc_profile_selected: Option<String>,
    icc_profile_scan_done: bool,
    icc_profile_scan_error: Option<String>,
    icc_show_incompatible: bool,
    show_about: bool,
''',
    '''    show_settings: bool,
    color: ColorManagementController,
    show_about: bool,
''',
    "color controller fields",
)
t = once(
    t,
    '''    previous_shade_list_texture_lru: VecDeque<String>,
    show_export_all: bool,
    export_all_folder: String,
    show_export_queue: bool,
    export_queue: export_queue::ExportQueue,
    export_queue_open_folder_after: Option<PathBuf>,
    show_tiff_inspector: bool,
    tiff_inspection: Option<tiff_inspect::TiffInspection>,
    tiff_inspect_error: Option<String>,
    opening_project_path: Option<PathBuf>,
    backup_restore_candidate: Option<BackupRestoreCandidate>,
    remind_after_export: bool,
    show_snapshot_save_reminder: bool,
    log: app_log::AppLog,
''',
    '''    previous_shade_list_texture_lru: VecDeque<String>,
    export: ExportController,
    inspector: TiffInspectorController,
    lifecycle: ProjectLifecycleController,
    log: app_log::AppLog,
''',
    "export inspector lifecycle controller fields",
)
t = once(
    t,
    '''    pending_snapshot_load: Option<u64>,
    pending_transition: Option<ProjectTransition>,
    transition_after_save: Option<ProjectTransition>,
    allow_close_once: bool,
    project_session_id: u64,
    history: history::AdjustmentHistory,
''',
    '''    pending_snapshot_load: Option<u64>,
    history: history::AdjustmentHistory,
''',
    "legacy lifecycle fields",
)

# Constructor controller composition.
t = once(
    t,
    '''            show_settings: false,
            show_color_management: false,
            icc_profile_query: String::new(),
            icc_profiles: Vec::new(),
            icc_profile_selected: None,
            icc_profile_scan_done: false,
            icc_profile_scan_error: None,
            icc_show_incompatible: false,
            show_about: false,
''',
    '''            show_settings: false,
            color: ColorManagementController::default(),
            show_about: false,
''',
    "color controller init",
)
t = once(
    t,
    '''            previous_shade_list_texture_lru: VecDeque::new(),
            show_export_all: false,
            export_all_folder: String::new(),
            show_export_queue: false,
            export_queue,
            export_queue_open_folder_after: None,
            show_tiff_inspector: false,
            tiff_inspection: None,
            tiff_inspect_error: None,
            opening_project_path: None,
            backup_restore_candidate: None,
            remind_after_export: false,
            show_snapshot_save_reminder: false,
            log,
''',
    '''            previous_shade_list_texture_lru: VecDeque::new(),
            export: ExportController::new(export_queue),
            inspector: TiffInspectorController::default(),
            lifecycle: ProjectLifecycleController::default(),
            log,
''',
    "controller init",
)
t = once(
    t,
    '''            pending_snapshot_load: None,
            pending_transition: None,
            transition_after_save: None,
            allow_close_once: false,
            project_session_id: 1,
            history,
''',
    '''            pending_snapshot_load: None,
            history,
''',
    "legacy lifecycle init",
)

# Mechanical controller field routing. Longest names first.
replacements = [
    ("self.export_queue_open_folder_after", "self.export.open_folder_after"),
    ("self.show_snapshot_save_reminder", "self.export.show_snapshot_save_reminder"),
    ("self.show_color_management", "self.color.show"),
    ("self.icc_profile_scan_error", "self.color.scan_error"),
    ("self.icc_profile_scan_done", "self.color.scan_done"),
    ("self.icc_profile_selected", "self.color.selected"),
    ("self.icc_show_incompatible", "self.color.show_incompatible"),
    ("self.icc_profile_query", "self.color.query"),
    ("self.icc_profiles", "self.color.profiles"),
    ("self.show_export_all", "self.export.show_all"),
    ("self.export_all_folder", "self.export.all_folder"),
    ("self.show_export_queue", "self.export.show_queue"),
    ("self.remind_after_export", "self.export.remind_after_export"),
    ("self.export_queue", "self.export.queue"),
    ("self.show_tiff_inspector", "self.inspector.show"),
    ("self.tiff_inspection", "self.inspector.inspection"),
    ("self.tiff_inspect_error", "self.inspector.error"),
    ("self.opening_project_path", "self.lifecycle.opening_path"),
    ("self.backup_restore_candidate", "self.lifecycle.backup_restore"),
    ("self.pending_transition", "self.lifecycle.pending"),
    ("self.transition_after_save", "self.lifecycle.after_save"),
    ("self.allow_close_once", "self.lifecycle.allow_close_once"),
    ("self.project_session_id", "self.lifecycle.session_id"),
]
for old, new in replacements:
    t = t.replace(old, new)

# Typed lifecycle controller now owns the guard decision.
old_request = '''    fn request_project_transition(
        &mut self,
        transition: ProjectTransition,
        ctx: Option<&egui::Context>,
    ) {
        if self.job.is_some() {
            self.report_info("Finish the current operation before changing projects.");
            return;
        }
        if self.export.queue.has_pending() {
            self.export.show_queue = true;
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
            self.lifecycle.pending = Some(transition);
            return;
        }
        self.execute_project_transition(transition, ctx);
    }
'''
new_request = '''    fn request_project_transition(
        &mut self,
        transition: ProjectTransition,
        ctx: Option<&egui::Context>,
    ) {
        match self.lifecycle.request(
            transition,
            self.job.is_some(),
            self.export.queue.has_pending(),
            self.project_dirty,
            !self.faces.is_empty(),
            self.project_path.is_some(),
        ) {
            TransitionRequest::BlockedByOperation => {
                self.report_info("Finish the current operation before changing projects.");
            }
            TransitionRequest::BlockedByExportQueue => {
                self.export.show_queue = true;
                self.report_info(
                    "Finish or cancel the Export Queue before changing projects or exiting.",
                );
            }
            TransitionRequest::AwaitingConfirmation => {}
            TransitionRequest::Execute(transition) => {
                self.execute_project_transition(transition, ctx);
            }
        }
    }
'''
t = once(t, old_request, new_request, "typed lifecycle request")

old_complete = '''    fn complete_transition_after_save(&mut self, ctx: &egui::Context) {
        if self.job.is_some() || self.project_dirty {
            return;
        }
        let Some(transition) = self.lifecycle.after_save.take() else {
            return;
        };
        self.execute_project_transition(transition, Some(ctx));
    }

    fn bump_project_session(&mut self) {
        self.lifecycle.session_id = self.lifecycle.session_id.wrapping_add(1).max(1);
    }
'''
new_complete = '''    fn complete_transition_after_save(&mut self, ctx: &egui::Context) {
        if let Some(transition) = self
            .lifecycle
            .take_after_successful_save(self.job.is_some(), self.project_dirty)
        {
            self.execute_project_transition(transition, Some(ctx));
        }
    }

    fn bump_project_session(&mut self) {
        self.lifecycle.bump_session();
    }
'''
t = once(t, old_complete, new_complete, "typed save continuation")

# Reset uses controller-owned lifecycle and controller-owned UI state.
t = once(
    t,
    '''        self.lifecycle.pending = None;
        self.lifecycle.after_save = None;
        self.color.show = false;
        self.color.query.clear();
        self.color.selected = None;
        self.export.remind_after_export = false;
        self.export.show_snapshot_save_reminder = false;
''',
    '''        self.lifecycle.cancel_pending();
        self.color.show = false;
        self.color.query.clear();
        self.color.selected = None;
        self.export.remind_after_export = false;
        self.export.show_snapshot_save_reminder = false;
''',
    "reset controller state",
)

# Save failure delegates to controller.
t = once(
    t,
    '''                Err(err) => {
                    if let Some(transition) = self.lifecycle.after_save.take() {
                        self.lifecycle.pending = Some(transition);
                    }
                    self.report_error(err);
                }
''',
    '''                Err(err) => {
                    self.lifecycle.save_failed();
                    self.report_error(err);
                }
''',
    "save failure controller",
)

# Transition dialog delegates pending/continuation state transitions to controller.
t = once(
    t,
    '''        if cancel {
            self.lifecycle.pending = None;
            self.lifecycle.after_save = None;
        } else if discard_and_continue {
            self.lifecycle.pending = None;
            self.lifecycle.after_save = None;
            if !matches!(transition, ProjectTransition::Recover) {
                if let Err(err) = recovery::clear() {
                    self.log.error(&err);
                }
            }
            self.execute_project_transition(transition, Some(ctx));
        } else if save_and_continue {
            self.lifecycle.pending = None;
            self.lifecycle.after_save = Some(transition.clone());
            if !self.save_project(false) {
                self.lifecycle.after_save = None;
                self.lifecycle.pending = Some(transition);
            }
        }
''',
    '''        if cancel {
            self.lifecycle.cancel_pending();
        } else if discard_and_continue {
            self.lifecycle.cancel_pending();
            if !matches!(transition, ProjectTransition::Recover) {
                if let Err(err) = recovery::clear() {
                    self.log.error(&err);
                }
            }
            self.execute_project_transition(transition, Some(ctx));
        } else if save_and_continue {
            self.lifecycle.begin_save_then(transition);
            if !self.save_project(false) {
                self.lifecycle.save_failed();
            }
        }
''',
    "transition dialog controller",
)

# Feature entry labels now come from a single stable wiring module.
t = t.replace('egui::Button::new("Inspect TIFF...")', 'egui::Button::new(app_features::TIFF_INSPECTOR_LABEL)')
t = t.replace('egui::Button::new("Export Queue")', 'egui::Button::new(app_features::EXPORT_QUEUE_LABEL)')
t = t.replace('ui.button("Export Queue")', 'ui.button(app_features::EXPORT_QUEUE_LABEL)')
# Preserve headings/window strings; replace only the explicit color-management opener if present.
t = t.replace('egui::Button::new("Color Management / ICC Preview")', 'egui::Button::new(app_features::COLOR_MANAGEMENT_LABEL)')

p.write_text(t, encoding="utf-8")

# Update wiring tests to accept centralized label constants referenced by main.
p = Path("src/app_features.rs")
t = p.read_text(encoding="utf-8")
t = once(
    t,
    '''        for label in [
            super::EXPORT_QUEUE_LABEL,
            super::TIFF_INSPECTOR_LABEL,
            super::COLOR_MANAGEMENT_LABEL,
            super::SOFT_PROOF_LABEL,
            super::MONITOR_PROFILE_LABEL,
        ] {
            assert!(MAIN.contains(label), "missing production feature entry point: {label}");
        }
''',
    '''        for entry in [
            "app_features::EXPORT_QUEUE_LABEL",
            "app_features::TIFF_INSPECTOR_LABEL",
            "Color Management / ICC Preview",
            super::SOFT_PROOF_LABEL,
            super::MONITOR_PROFILE_LABEL,
        ] {
            assert!(MAIN.contains(entry), "missing production feature entry point: {entry}");
        }
''',
    "central label wiring test",
)
p.write_text(t, encoding="utf-8")
