from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8", newline="\n")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def function_span(text: str, signature: str) -> tuple[int, int]:
    start = text.find(signature)
    if start < 0:
        raise RuntimeError(f"Function not found: {signature}")
    brace = text.find("{", start)
    if brace < 0:
        raise RuntimeError(f"Opening brace not found: {signature}")
    depth = 0
    for index in range(brace, len(text)):
        ch = text[index]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return start, index + 1
    raise RuntimeError(f"Closing brace not found: {signature}")


def replace_function(text: str, signature: str, new_function: str) -> str:
    start, end = function_span(text, signature)
    return text[:start] + new_function.rstrip() + text[end:]


def replace_inside_function(text: str, signature: str, old: str, new: str, label: str) -> str:
    start, end = function_span(text, signature)
    body = text[start:end]
    body = replace_once(body, old, new, label)
    return text[:start] + body + text[end:]


# ---------------------------------------------------------------------------
# 1) Short deterministic atomic TIFF/spool temporary names.
# ---------------------------------------------------------------------------
export_rs = read("src/export.rs")
export_rs = replace_once(
    export_rs,
    '    let temporary = temporary_export_path(destination)?;\n    let result = export_face_direct_with_progress(',
    '    let temporary = temporary_export_path(destination)?;\n    remove_stale_temp(&temporary, "temporary export TIFF")?;\n    let result = export_face_direct_with_progress(',
    "prepare short atomic temp",
)
export_rs = replace_once(
    export_rs,
    '    let spool_path = temporary_spool_path(destination)?;\n\n    let result = (|| -> Result<(), String> {',
    '    let spool_path = temporary_spool_path(destination)?;\n    remove_stale_temp(&spool_path, "temporary export spool")?;\n\n    let result = (|| -> Result<(), String> {',
    "prepare short spool temp",
)
old_temp_functions = '''fn temporary_spool_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export.tif");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(
            ".{file_name}.shade-editor-spool-{}-{stamp}-{attempt}.raw",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Cannot allocate a temporary export spool beside the destination.".to_owned())
}

fn temporary_export_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export.tif");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(
            ".{file_name}.shade-editor-{}-{stamp}-{attempt}.tmp",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Cannot allocate a temporary export file beside the destination.".to_owned())
}
'''
new_temp_functions = '''fn temporary_spool_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export.tif".to_owned());
    let final_name = file_name.strip_suffix(".tmp").unwrap_or(&file_name);
    Ok(parent.join(format!("{final_name}.spool.tmp")))
}

fn temporary_export_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export.tif".to_owned());
    Ok(parent.join(format!("{file_name}.tmp")))
}

fn remove_stale_temp(path: &Path, label: &str) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(path)
        .map_err(|err| format!("Cannot remove stale {label} {}: {err}", path.display()))
}
'''
export_rs = replace_once(export_rs, old_temp_functions, new_temp_functions, "replace temp naming")
insert_before_export_tests = '''    #[test]
    fn temporary_export_and_spool_names_stay_short_and_deterministic() {
        let destination = Path::new("Fabia_Gray_S8-E6_2026-08-15.tif");
        let temporary = temporary_export_path(destination).unwrap();
        assert_eq!(
            temporary.file_name().unwrap().to_string_lossy(),
            "Fabia_Gray_S8-E6_2026-08-15.tif.tmp"
        );
        let spool = temporary_spool_path(&temporary).unwrap();
        assert_eq!(
            spool.file_name().unwrap().to_string_lossy(),
            "Fabia_Gray_S8-E6_2026-08-15.tif.spool.tmp"
        );
        assert!(!spool.to_string_lossy().contains("shade-editor-spool"));
    }

'''
marker = "#[cfg(test)]\nmod tests {\n"
export_rs = replace_once(export_rs, marker, marker + "    use super::*;\n\n" if "mod tests {\n    use super::*;" not in export_rs else marker, "export test module") if False else export_rs
# Insert into the existing test module after its opening without duplicating imports.
module_at = export_rs.find("#[cfg(test)]\nmod tests {")
if module_at < 0:
    raise RuntimeError("export tests module not found")
brace_at = export_rs.find("{", module_at)
insert_at = export_rs.find("\n", brace_at) + 1
export_rs = export_rs[:insert_at] + insert_before_export_tests + export_rs[insert_at:]
write("src/export.rs", export_rs)


# ---------------------------------------------------------------------------
# 2) Persist queue recipes, but restored work requires explicit operator resume.
# ---------------------------------------------------------------------------
queue_rs = read("src/export_queue.rs")
queue_rs = replace_once(
    queue_rs,
    '    pub error: Option<String>,\n    spec: QueuedExportSpec,',
    '    pub error: Option<String>,\n    /// True when this row came from a previous application session.\n    pub restored: bool,\n    /// Restored Waiting/Processing work never starts until the operator explicitly resumes it.\n    pub requires_resume: bool,\n    spec: QueuedExportSpec,',
    "queue runtime restore fields",
)
queue_rs = replace_once(
    queue_rs,
    '            let recovered_processing = saved.status == ExportQueueStatus::Processing;\n            let status = if recovered_processing {',
    '            let recovered_processing = saved.status == ExportQueueStatus::Processing;\n            let requires_resume = matches!(\n                saved.status,\n                ExportQueueStatus::Waiting | ExportQueueStatus::Processing\n            );\n            let status = if recovered_processing {',
    "queue restored resume flag",
)
queue_rs = replace_once(
    queue_rs,
    '''                detail: if recovered_processing {
                    "Recovered after restart · ready to resume".to_owned()
                } else {
                    String::new()
                },
                error: saved.error,
                spec,
''',
    '''                detail: if requires_resume {
                    "Recovered from previous session · paused until you resume it".to_owned()
                } else {
                    String::new()
                },
                error: saved.error,
                restored: true,
                requires_resume,
                spec,
''',
    "queue restored row state",
)
# Both enqueue constructors get non-restored runtime state.
needle = '''            detail: String::new(),
            error: None,
            spec: QueuedExportSpec {
'''
if queue_rs.count(needle) != 2:
    raise RuntimeError(f"queue enqueue constructor count changed: {queue_rs.count(needle)}")
queue_rs = queue_rs.replace(
    needle,
    '''            detail: String::new(),
            error: None,
            restored: false,
            requires_resume: false,
            spec: QueuedExportSpec {
''',
)
queue_rs = replace_once(
    queue_rs,
    '''    pub fn items(&self) -> &[ExportQueueItem] {
        &self.items
    }

    pub fn pending_count(&self) -> usize {
''',
    '''    pub fn items(&self) -> &[ExportQueueItem] {
        &self.items
    }

    pub fn restored_count(&self) -> usize {
        self.items.iter().filter(|item| item.restored).count()
    }

    pub fn recovered_waiting_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == ExportQueueStatus::Waiting && item.requires_resume)
            .count()
    }

    pub fn resume(&mut self, id: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if item.status != ExportQueueStatus::Waiting || !item.requires_resume {
            return false;
        }
        item.requires_resume = false;
        item.detail.clear();
        self.persist();
        true
    }

    pub fn resume_recovered(&mut self) -> usize {
        let mut resumed = 0usize;
        for item in &mut self.items {
            if item.status == ExportQueueStatus::Waiting && item.requires_resume {
                item.requires_resume = false;
                item.detail.clear();
                resumed += 1;
            }
        }
        if resumed > 0 {
            self.persist();
        }
        resumed
    }

    pub fn pending_count(&self) -> usize {
''',
    "queue resume API",
)
queue_rs = replace_once(
    queue_rs,
    '''            .filter(|item| {
                matches!(
                    item.status,
                    ExportQueueStatus::Waiting | ExportQueueStatus::Processing
                )
            })
            .count()
''',
    '''            .filter(|item| {
                item.status == ExportQueueStatus::Processing
                    || (item.status == ExportQueueStatus::Waiting && !item.requires_resume)
            })
            .count()
''',
    "pending excludes paused restored jobs",
)
queue_rs = replace_once(
    queue_rs,
    '''            ExportQueueStatus::Waiting => {
                item.status = ExportQueueStatus::Cancelled;
                item.detail = "Cancelled before processing".to_owned();
                true
            }
''',
    '''            ExportQueueStatus::Waiting => {
                item.status = ExportQueueStatus::Cancelled;
                item.requires_resume = false;
                item.detail = "Cancelled before processing".to_owned();
                true
            }
''',
    "cancel recovered waiting",
)
queue_rs = replace_once(
    queue_rs,
    '''        item.status = ExportQueueStatus::Waiting;
        item.progress = 0.0;
        item.detail.clear();
''',
    '''        item.status = ExportQueueStatus::Waiting;
        item.requires_resume = false;
        item.progress = 0.0;
        item.detail.clear();
''',
    "retry is explicit resume",
)
queue_rs = replace_once(
    queue_rs,
    '''            if item.status == ExportQueueStatus::Waiting {
                item.status = ExportQueueStatus::Cancelled;
                item.detail = "Cancelled before processing".to_owned();
''',
    '''            if item.status == ExportQueueStatus::Waiting {
                item.status = ExportQueueStatus::Cancelled;
                item.requires_resume = false;
                item.detail = "Cancelled before processing".to_owned();
''',
    "cancel all recovered waiting",
)
queue_rs = replace_once(
    queue_rs,
    '''            .position(|item| item.status == ExportQueueStatus::Waiting)
''',
    '''            .position(|item| {
                item.status == ExportQueueStatus::Waiting && !item.requires_resume
            })
''',
    "do not auto-start restored queue",
)
queue_rs = replace_once(
    queue_rs,
    '''        self.items[index].status = ExportQueueStatus::Processing;
        self.items[index].progress = 0.0;
''',
    '''        self.items[index].status = ExportQueueStatus::Processing;
        self.items[index].requires_resume = false;
        self.items[index].progress = 0.0;
''',
    "processing clears resume gate",
)
queue_rs = replace_once(
    queue_rs,
    '''        assert_eq!(restored.items[0].status, ExportQueueStatus::Waiting);
        assert!(restored.items[0].spec.export.mark.is_none());
''',
    '''        assert_eq!(restored.items[0].status, ExportQueueStatus::Waiting);
        assert!(restored.items[0].restored);
        assert!(restored.items[0].requires_resume);
        assert_eq!(restored.pending_count(), 0);
        assert_eq!(restored.recovered_waiting_count(), 1);
        assert!(restored.items[0].spec.export.mark.is_none());
''',
    "queue persistence test resume assertions",
)
queue_test_insert = '''
    #[test]
    fn restored_waiting_work_requires_explicit_resume() {
        let folder = temp_folder("paused-restore");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.tif");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = folder.join("out.tif");
        let path = folder.join("queue.json");
        let mut queue = ExportQueue::empty(Some(path.clone()));
        let mut queued = spec(destination.to_str().unwrap());
        queued.source = source.clone();
        let id = queue
            .enqueue_for_project(queued, vec![source], 55)
            .unwrap();
        queue.persist();
        drop(queue);

        let mut restored = ExportQueue::load_from_path(path).unwrap();
        assert_eq!(restored.pending_count(), 0);
        assert!(restored.active_id.is_none());
        assert!(restored.poll().is_empty());
        assert!(restored.active_id.is_none());
        assert!(restored.resume(id));
        assert_eq!(restored.pending_count(), 1);
        let _ = std::fs::remove_dir_all(folder);
    }
'''
last_module_brace = queue_rs.rfind("}\n")
if last_module_brace < 0:
    raise RuntimeError("queue test module end not found")
queue_rs = queue_rs[:last_module_brace] + queue_test_insert + queue_rs[last_module_brace:]
write("src/export_queue.rs", queue_rs)


# ---------------------------------------------------------------------------
# 3) Separate Snapshot/Test filename template from Export All.
# ---------------------------------------------------------------------------
settings_rs = read("src/settings.rs")
settings_rs = replace_once(
    settings_rs,
    '    pub export_all_template: String,\n    pub export_folder_template: String,',
    '    pub export_all_template: String,\n    pub snapshot_export_template: String,\n    pub export_folder_template: String,',
    "settings snapshot template field",
)
settings_rs = replace_once(
    settings_rs,
    '            export_all_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),\n            export_folder_template: DEFAULT_FOLDER_TEMPLATE.to_owned(),',
    '            export_all_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),\n            snapshot_export_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),\n            export_folder_template: DEFAULT_FOLDER_TEMPLATE.to_owned(),',
    "settings snapshot template default",
)
settings_rs = replace_once(
    settings_rs,
    '''        if self.export_all_template.trim().is_empty() {
            self.export_all_template = DEFAULT_EXPORT_TEMPLATE.to_owned();
        }
        if self
''',
    '''        if self.export_all_template.trim().is_empty() {
            self.export_all_template = DEFAULT_EXPORT_TEMPLATE.to_owned();
        }
        if self.snapshot_export_template.trim().is_empty() {
            self.snapshot_export_template = DEFAULT_EXPORT_TEMPLATE.to_owned();
        }
        if self
''',
    "sanitize snapshot template",
)
settings_rs = replace_once(
    settings_rs,
    '        assert_eq!(settings.export_all_template, DEFAULT_EXPORT_TEMPLATE);\n        assert_eq!(settings.export_folder_template, DEFAULT_FOLDER_TEMPLATE);',
    '        assert_eq!(settings.export_all_template, DEFAULT_EXPORT_TEMPLATE);\n        assert_eq!(settings.snapshot_export_template, DEFAULT_EXPORT_TEMPLATE);\n        assert_eq!(settings.export_folder_template, DEFAULT_FOLDER_TEMPLATE);',
    "snapshot template default test",
)
write("src/settings.rs", settings_rs)


# ---------------------------------------------------------------------------
# 4/5) Move History left; improve queue UI, status placement, coloring and reveal.
# ---------------------------------------------------------------------------
workflow_rs = read("src/workflow.rs")
workflow_rs = replace_once(
    workflow_rs,
    '''    app.ui_test_code(ui);
}

pub(super) fn ui_missing_viewport''',
    '''    app.ui_test_code(ui);
    ui.separator();
    app.ui_history(ui);
}

pub(super) fn ui_missing_viewport''',
    "history below test code",
)
write("src/workflow.rs", workflow_rs)

controller_rs = read("src/app_controllers.rs")
controller_rs = replace_once(
    controller_rs,
    '''    pub fn new(queue: ExportQueue) -> Self {
        Self {
            show_all: false,
            all_folder: String::new(),
            show_queue: false,
            queue,
''',
    '''    pub fn new(queue: ExportQueue) -> Self {
        let show_queue = queue.restored_count() > 0;
        Self {
            show_all: false,
            all_folder: String::new(),
            show_queue,
            queue,
''',
    "show restored queue on startup",
)
write("src/app_controllers.rs", controller_rs)

main_rs = read("src/main.rs")
main_rs = replace_inside_function(
    main_rs,
    "    fn export_snapshot_dialog(&mut self, snapshot_id: u64)",
    "self.settings.export_all_template",
    "self.settings.snapshot_export_template",
    "single snapshot template",
)
main_rs = replace_inside_function(
    main_rs,
    "    fn export_snapshot_group_dialog(&mut self, snapshot_ids: Vec<u64>, label: String)",
    "self.settings.export_all_template",
    "self.settings.snapshot_export_template",
    "snapshot group template",
)
main_rs = replace_once(
    main_rs,
    '''                ui.small("Off by default: Export all writes clean Face TIFFs without Test Code. Enable this only when every Face in Export all should receive the current Test Code configuration.");
                let old_default_dpi = self.settings.default_dpi;
''',
    '''                ui.small("Off by default: Export all writes clean Face TIFFs without Test Code. Enable this only when every Face in Export all should receive the current Test Code configuration.");
                ui.add_space(8.0);
                ui.strong("Snapshot / Test export filename template");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.settings.snapshot_export_template)
                            .desired_width(520.0),
                    )
                    .changed();
                ui.small("Used only by Snapshot/Test exports. Tokens: {project}, {face}, {snapshot}, {testcode}, {source}, {date}. Export Face keeps its manual Save As filename, and Export All keeps the template in its own window.");
                let old_default_dpi = self.settings.default_dpi;
''',
    "snapshot template settings UI",
)
main_rs = replace_once(
    main_rs,
    '''                    .show(&mut columns[0], |ui| {
                        self.ui_channels_histogram(ui);
                        ui.separator();
                        self.ui_history(ui);
                    });
''',
    '''                    .show(&mut columns[0], |ui| self.ui_channels_histogram(ui));
''',
    "remove history from two-column tools",
)
main_rs = replace_once(
    main_rs,
    '''            egui::ScrollArea::vertical().show(ui, |ui| {
                self.ui_channels_histogram(ui);
                ui.separator();
                self.ui_history(ui);
                ui.separator();
                self.ui_adjustments(ui);
            });
''',
    '''            egui::ScrollArea::vertical().show(ui, |ui| {
                self.ui_channels_histogram(ui);
                ui.separator();
                self.ui_adjustments(ui);
            });
''',
    "remove history from single-column tools",
)
# Toolbar makes recovered paused work visible even though it is not pending/runnable.
main_rs = replace_once(
    main_rs,
    '                let queue_label = format!("Queue ({})", self.export.queue.pending_count());\n                if ui.button(queue_label).clicked() { self.export.show_queue = true; }',
    '''                let queue_pending = self.export.queue.pending_count();
                let queue_recovered = self.export.queue.recovered_waiting_count();
                let queue_label = if queue_recovered > 0 {
                    format!("Queue ({queue_pending} + {queue_recovered} recovered)")
                } else {
                    format!("Queue ({queue_pending})")
                };
                if ui.button(queue_label).clicked() { self.export.show_queue = true; }''',
    "toolbar recovered queue count",
)

new_queue_ui = r'''    fn ui_export_queue_window(&mut self, ctx: &egui::Context) {
        if !self.export.show_queue {
            return;
        }
        let mut open = self.export.show_queue;
        let rows = self
            .export
            .queue
            .items()
            .iter()
            .map(|item| {
                (
                    item.id,
                    item.label.clone(),
                    item.destination.clone(),
                    item.status,
                    item.progress,
                    item.detail.clone(),
                    item.error.clone(),
                    item.restored,
                    item.requires_resume,
                )
            })
            .collect::<Vec<_>>();
        let pending = self.export.queue.pending_count();
        let recovered_waiting = self.export.queue.recovered_waiting_count();
        let mut cancel_id = None;
        let mut resume_id = None;
        let mut retry_id = None;
        let mut reveal_folder = None;
        let mut resume_recovered = false;
        let mut cancel_waiting = false;
        let mut clear_finished = false;

        egui::Window::new("Export Queue")
            .open(&mut open)
            .resizable(true)
            .default_size([820.0, 540.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Export Queue");
                    ui.label(format!("{pending} pending"));
                    if recovered_waiting > 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(225, 175, 70),
                            format!("{recovered_waiting} recovered · paused"),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clear_finished = ui.button("Clear finished").clicked();
                        cancel_waiting = ui.button("Cancel waiting").clicked();
                        if recovered_waiting > 0 {
                            resume_recovered = ui.button("Resume recovered").clicked();
                        }
                    });
                });
                if recovered_waiting > 0 {
                    ui.small("Recovered exports are never started automatically. Resume individual rows or use Resume recovered when you want them to run.");
                } else {
                    ui.small("Waiting items can be cancelled immediately. Processing items use Stop after current so the current atomic TIFF finishes safely.");
                }
                ui.separator();

                if rows.is_empty() {
                    ui.label("No export jobs yet.");
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (id, label, destination, status, progress, detail, error, restored, requires_resume) in &rows {
                            let (fill, status_color) = match status {
                                export_queue::ExportQueueStatus::Waiting => (
                                    egui::Color32::from_rgba_unmultiplied(135, 95, 20, 34),
                                    egui::Color32::from_rgb(230, 180, 70),
                                ),
                                export_queue::ExportQueueStatus::Processing => (
                                    egui::Color32::from_rgba_unmultiplied(25, 90, 165, 38),
                                    egui::Color32::from_rgb(90, 165, 255),
                                ),
                                export_queue::ExportQueueStatus::Done => (
                                    egui::Color32::from_rgba_unmultiplied(30, 115, 60, 32),
                                    egui::Color32::from_rgb(90, 205, 125),
                                ),
                                export_queue::ExportQueueStatus::Failed => (
                                    egui::Color32::from_rgba_unmultiplied(155, 35, 35, 40),
                                    egui::Color32::from_rgb(245, 105, 105),
                                ),
                                export_queue::ExportQueueStatus::Cancelled => (
                                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 26),
                                    egui::Color32::from_rgb(165, 165, 165),
                                ),
                            };
                            let status_text = if *requires_resume {
                                "Waiting · paused"
                            } else {
                                status.label()
                            };
                            egui::Frame::new()
                                .inner_margin(8)
                                .fill(fill)
                                .stroke(egui::Stroke::new(1.0, status_color))
                                .corner_radius(5)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.strong(label);
                                        if *restored && !*requires_resume {
                                            ui.small("restored");
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(status_text)
                                                        .color(status_color)
                                                        .strong(),
                                                );
                                                if ui.small_button("Reveal folder").clicked() {
                                                    reveal_folder = Some(
                                                        destination
                                                            .parent()
                                                            .unwrap_or_else(|| Path::new("."))
                                                            .to_path_buf(),
                                                    );
                                                }
                                                if *requires_resume {
                                                    if ui.small_button("Resume").clicked() {
                                                        resume_id = Some(*id);
                                                    }
                                                    if ui.small_button("Cancel").clicked() {
                                                        cancel_id = Some(*id);
                                                    }
                                                } else {
                                                    match status {
                                                        export_queue::ExportQueueStatus::Waiting => {
                                                            if ui.small_button("Cancel").clicked() {
                                                                cancel_id = Some(*id);
                                                            }
                                                        }
                                                        export_queue::ExportQueueStatus::Processing => {
                                                            if ui.small_button("Stop after current").clicked() {
                                                                cancel_id = Some(*id);
                                                            }
                                                        }
                                                        export_queue::ExportQueueStatus::Failed
                                                        | export_queue::ExportQueueStatus::Cancelled => {
                                                            if ui.small_button("Retry").clicked() {
                                                                retry_id = Some(*id);
                                                            }
                                                        }
                                                        export_queue::ExportQueueStatus::Done => {}
                                                    }
                                                }
                                            },
                                        );
                                    });

                                    if *status == export_queue::ExportQueueStatus::Processing {
                                        ui.add(
                                            egui::ProgressBar::new(*progress)
                                                .desired_width(f32::INFINITY)
                                                .text(if detail.trim().is_empty() {
                                                    "Processing".to_owned()
                                                } else {
                                                    detail.clone()
                                                }),
                                        );
                                    } else {
                                        let detail = detail.trim();
                                        if !detail.is_empty()
                                            && detail != status.label()
                                            && detail != "Done"
                                        {
                                            let detail = detail.strip_prefix("Done · ").unwrap_or(detail);
                                            if !detail.is_empty() {
                                                ui.small(detail);
                                            }
                                        }
                                    }
                                    ui.small(destination.display().to_string());
                                    if let Some(error) = error {
                                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                                    }
                                });
                            ui.add_space(5.0);
                        }
                    });
                }
            });
        self.export.show_queue = open;

        if resume_recovered {
            let count = self.export.queue.resume_recovered();
            if count > 0 {
                self.report_info(format!("Resumed {count} recovered export(s)"));
            }
        }
        if cancel_waiting {
            self.export.queue.cancel_all_waiting();
        }
        if clear_finished {
            self.export.queue.clear_finished();
        }
        if let Some(id) = resume_id {
            self.export.queue.resume(id);
        }
        if let Some(id) = cancel_id {
            self.export.queue.cancel(id);
        }
        if let Some(id) = retry_id {
            self.export.queue.retry(id);
        }
        if let Some(folder) = reveal_folder {
            if let Err(err) = open_folder(&folder) {
                self.report_error(err);
            }
        }
    }
'''
main_rs = replace_function(
    main_rs,
    "    fn ui_export_queue_window(&mut self, ctx: &egui::Context)",
    new_queue_ui,
)
write("src/main.rs", main_rs)


# ---------------------------------------------------------------------------
# Version + release notes.
# ---------------------------------------------------------------------------
cargo = read("Cargo.toml")
cargo = replace_once(cargo, 'version = "0.18.0"', 'version = "0.18.1"', "Cargo version")
write("Cargo.toml", cargo)
write("VERSION", "0.18.1\n")

notes = read("RELEASE_NOTES.md")
header = '''# Shade Editor 0.18.1

- Shorten production export staging paths to deterministic sibling names: `final.tif.tmp` and `final.tif.spool.tmp`, avoiding path-length failures from nested timestamp/PID suffixes.
- Keep persisted Export Queue rows visible after restart but pause every recovered Waiting/Processing job until the operator explicitly resumes it; recovered work can be resumed or cancelled per row or as a group.
- Separate Snapshot/Test export filename templating from Export All. Export Face remains a manual Save As filename, Export All keeps its editable template window, and Snapshot/Test exports use a dedicated Settings template.
- Move History from the right Tools sidebar to the left sidebar directly below Test Code.
- Color-code Export Queue rows by Waiting/Processing/Done/Failed/Cancelled state, move status to the right side, remove duplicate Done text, and add Reveal folder per row.

'''
if not notes.startswith("# Shade Editor 0.18.1"):
    notes = header + notes
write("RELEASE_NOTES.md", notes)

print("Applied v0.18.1 operator feedback patch")
