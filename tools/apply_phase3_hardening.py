from pathlib import Path
import re


def once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1, got {count}")
    return text.replace(old, new, 1)

# Recovery: consume legacy v1 once, immediately rewrite as verified v2.
p = Path("src/recovery.rs")
t = p.read_text(encoding="utf-8")
t = once(t, '''pub fn load() -> Result<Option<RecoveryFile>, String> {
    load_from_paths(&recovery_paths())
}
''', '''pub fn load() -> Result<Option<RecoveryFile>, String> {
    let loaded = load_from_paths(&recovery_paths())?;
    let Some(recovery) = loaded else {
        return Ok(None);
    };
    if recovery.format_version == LEGACY_RECOVERY_FORMAT_VERSION {
        let upgraded = stamped_recovery(&recovery)?;
        write(&upgraded)?;
        return Ok(Some(upgraded));
    }
    Ok(Some(recovery))
}
''', "legacy migration load")
t = once(t, '''    #[test]
    fn legacy_v1_recovery_remains_readable() {
''', '''    #[test]
    fn legacy_v1_can_be_upgraded_to_checksummed_v2() {
        let mut recovery = RecoveryFile::new(ShadeProject::default(), vec![], None);
        recovery.format_version = LEGACY_RECOVERY_FORMAT_VERSION;
        recovery.checksum_sha256.clear();
        let upgraded = stamped_recovery(&recovery).unwrap();
        assert_eq!(upgraded.format_version, RECOVERY_FORMAT_VERSION);
        assert!(!upgraded.checksum_sha256.is_empty());
        verify_recovery_checksum(&upgraded).unwrap();
    }

    #[test]
    fn legacy_v1_recovery_remains_readable() {
''', "legacy upgrade test")
p.write_text(t, encoding="utf-8")

# Main: async inspector and verified backup restore UI.
p = Path("src/main.rs")
t = p.read_text(encoding="utf-8")

# JobResult variant.
t = once(t, '''    Save {
        path: PathBuf,
        result: Result<(), String>,
    },
    Export(SnapshotExportBatchResult),
''', '''    Save {
        path: PathBuf,
        result: Result<(), String>,
    },
    InspectTiff(Result<tiff_inspect::TiffInspection, String>),
    Export(SnapshotExportBatchResult),
''', "inspect job result")

# Backup candidate type after ErrorToast.
t = once(t, '''struct ErrorToast {
    message: String,
    created: Instant,
}

struct ShadeApp {
''', '''struct ErrorToast {
    message: String,
    created: Instant,
}

#[derive(Clone)]
struct BackupRestoreCandidate {
    primary_path: PathBuf,
    backup_path: PathBuf,
    primary_error: String,
}

struct ShadeApp {
''', "backup candidate type")

# Fields.
t = once(t, '''    show_tiff_inspector: bool,
    tiff_inspection: Option<tiff_inspect::TiffInspection>,
    tiff_inspect_error: Option<String>,
    remind_after_export: bool,
''', '''    show_tiff_inspector: bool,
    tiff_inspection: Option<tiff_inspect::TiffInspection>,
    tiff_inspect_error: Option<String>,
    opening_project_path: Option<PathBuf>,
    backup_restore_candidate: Option<BackupRestoreCandidate>,
    remind_after_export: bool,
''', "phase3 fields")
t = once(t, '''            show_tiff_inspector: false,
            tiff_inspection: None,
            tiff_inspect_error: None,
            remind_after_export: false,
''', '''            show_tiff_inspector: false,
            tiff_inspection: None,
            tiff_inspect_error: None,
            opening_project_path: None,
            backup_restore_candidate: None,
            remind_after_export: false,
''', "phase3 defaults")

# Track requested open path before background job.
t = once(t, '''    fn open_project_path(&mut self, path: PathBuf) {
        if self.job.is_some() {
            return;
        }
        self.recovery_candidate = None;
        let max_dimension = self.settings.max_preview_dimension;
''', '''    fn open_project_path(&mut self, path: PathBuf) {
        if self.job.is_some() {
            return;
        }
        self.recovery_candidate = None;
        self.backup_restore_candidate = None;
        self.opening_project_path = Some(path.clone());
        let max_dimension = self.settings.max_preview_dimension;
''', "track opening path")

# Open poll result: clear path on success; inspect validated backup on failure.
t = once(t, '''            JobResult::Open(result) => match result {
                Ok(payload) => {
                    self.project = payload.project;
''', '''            JobResult::Open(result) => match result {
                Ok(payload) => {
                    self.opening_project_path = None;
                    self.backup_restore_candidate = None;
                    self.project = payload.project;
''', "open success clear")
t = once(t, '''                Err(err) => self.report_error(err),
            },
            JobResult::Recover(result) => match result {
''', '''                Err(err) => {
                    let primary_path = self.opening_project_path.take();
                    if let Some(primary_path) = primary_path {
                        let backup_path = safe_fs::backup_path(&primary_path);
                        if backup_path.is_file() && ShadeProject::load(&backup_path).is_ok() {
                            self.backup_restore_candidate = Some(BackupRestoreCandidate {
                                primary_path,
                                backup_path,
                                primary_error: err.clone(),
                            });
                        }
                    }
                    self.report_error(err);
                }
            },
            JobResult::Recover(result) => match result {
''', "open failure backup detection")

# Inspect job poll branch before Export.
t = once(t, '''            JobResult::Export(payload) => {
''', '''            JobResult::InspectTiff(result) => {
                match result {
                    Ok(report) => {
                        self.tiff_inspection = Some(report);
                        self.tiff_inspect_error = None;
                    }
                    Err(err) => {
                        self.tiff_inspection = None;
                        self.tiff_inspect_error = Some(err);
                    }
                }
                self.show_tiff_inspector = true;
            }
            JobResult::Export(payload) => {
''', "inspect poll branch")

# Async inspect dialog.
pattern = re.compile(r'''    fn inspect_tiff_dialog\(&mut self\) \{.*?\n    \}\n\n    fn ui_tiff_inspector_window''', re.S)
match = pattern.search(t)
if not match:
    raise RuntimeError("inspect_tiff_dialog block not found")
replacement = '''    fn inspect_tiff_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        let mut dialog = rfd::FileDialog::new().add_filter("TIFF image", &["tif", "tiff"]);
        if let Some(parent) = self
            .faces
            .get(self.current_face)
            .and_then(|face| face.path.parent())
        {
            dialog = dialog.set_directory(parent);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Inspecting TIFF", move |progress| {
            Self::set_progress(&progress, Some(0.1), "Inspecting TIFF", "Reading bounded TIFF metadata");
            let result = tiff_inspect::inspect(&path, default_dpi);
            Self::set_progress(&progress, Some(1.0), "Inspecting TIFF", "Complete");
            JobResult::InspectTiff(result)
        });
    }

    fn ui_tiff_inspector_window'''
t = t[:match.start()] + replacement + t[match.end():]

# Backup restore window inserted before TIFF inspector window.
marker = '''    fn ui_tiff_inspector_window(&mut self, ctx: &egui::Context) {
'''
backup_window = '''    fn ui_backup_restore_window(&mut self, ctx: &egui::Context) {
        let Some(candidate) = self.backup_restore_candidate.clone() else {
            return;
        };
        let mut restore = false;
        let mut cancel = false;
        egui::Window::new("Project backup available")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.strong("The selected .shade file could not be opened, but its .bak backup is valid.");
                ui.label(format!("Primary: {}", candidate.primary_path.display()));
                ui.label(format!("Backup: {}", candidate.backup_path.display()));
                ui.colored_label(egui::Color32::LIGHT_RED, &candidate.primary_error);
                ui.small("Restore keeps a copy of the failed primary as .corrupt before atomically replacing it with the validated backup.");
                ui.horizontal(|ui| {
                    restore = ui.button("Restore validated backup").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if cancel {
            self.backup_restore_candidate = None;
        } else if restore {
            let corrupt = append_path_suffix(&candidate.primary_path, ".corrupt");
            let result = (|| -> Result<(), String> {
                if candidate.primary_path.is_file() {
                    safe_fs::atomic_copy(&candidate.primary_path, &corrupt)?;
                }
                safe_fs::atomic_copy(&candidate.backup_path, &candidate.primary_path)?;
                ShadeProject::load(&candidate.primary_path)
                    .map(|_| ())
                    .map_err(|err| format!("Restored backup did not validate: {err}"))
            })();
            match result {
                Ok(()) => {
                    self.backup_restore_candidate = None;
                    self.open_project_path(candidate.primary_path);
                }
                Err(err) => self.report_error(format!("Backup restore failed: {err}")),
            }
        }
    }

'''
t = once(t, marker, backup_window + marker, "backup restore window insertion")

# Call window in update.
t = once(t, '''        self.ui_tiff_inspector_window(ui.ctx());
        self.ui_recovery_window(ui.ctx());
''', '''        self.ui_tiff_inspector_window(ui.ctx());
        self.ui_backup_restore_window(ui.ctx());
        self.ui_recovery_window(ui.ctx());
''', "backup window call")

# Path suffix helper near project_name_for_path.
t = once(t, '''fn project_name_for_path(current: &str, path: &Path) -> String {
''', '''fn append_path_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

fn project_name_for_path(current: &str, path: &Path) -> String {
''', "path suffix helper")

p.write_text(t, encoding="utf-8")
