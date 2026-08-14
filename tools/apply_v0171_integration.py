from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAIN = ROOT / "src" / "main.rs"


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one anchor, found {count}")
    return text.replace(old, new, 1)


def replace_section(text: str, start: str, end: str, new: str, label: str) -> str:
    i = text.find(start)
    if i < 0:
        raise RuntimeError(f"{label}: start anchor not found")
    j = text.find(end, i)
    if j < 0:
        raise RuntimeError(f"{label}: end anchor not found")
    return text[:i] + new + text[j:]


text = MAIN.read_text(encoding="utf-8")

text = replace_once(
    text,
    "mod export_batch;\nmod history;",
    "mod export_batch;\nmod export_queue;\nmod history;",
    "export queue module",
)
text = replace_once(
    text,
    "mod thumbnail;\nmod tiff_io;",
    "mod thumbnail;\nmod tiff_inspect;\nmod tiff_io;",
    "TIFF inspector module",
)
text = replace_once(
    text,
    "use std::collections::{BTreeMap, VecDeque};",
    "use std::collections::{BTreeMap, BTreeSet, VecDeque};",
    "BTreeSet import",
)

text = replace_once(
    text,
    "    original_texture: Option<egui::TextureHandle>,\n    generation: u64,",
    "    original_texture: Option<egui::TextureHandle>,\n    embedded_original_texture: Option<egui::TextureHandle>,\n    embedded_original_status: PreviewColorStatus,\n    generation: u64,",
    "RuntimeFace embedded source cache",
)
text = replace_once(
    text,
    "    rgba: Vec<u8>,\n    original_rgba: Vec<u8>,\n}",
    "    rgba: Vec<u8>,\n    original_rgba: Vec<u8>,\n    embedded_original_rgba: Option<Vec<u8>>,\n    embedded_original_status: Option<PreviewColorStatus>,\n}",
    "RenderResult embedded source cache",
)
text = replace_once(
    text,
    "    show_export_all: bool,\n    export_all_folder: String,",
    "    show_export_all: bool,\n    export_all_folder: String,\n    show_export_queue: bool,\n    export_queue: export_queue::ExportQueue,\n    export_queue_open_folder_after: Option<PathBuf>,\n    show_tiff_inspector: bool,\n    tiff_inspection: Option<tiff_inspect::TiffInspection>,\n    tiff_inspect_error: Option<String>,",
    "ShadeApp production tool state",
)
text = replace_once(
    text,
    "            show_export_all: false,\n            export_all_folder: String::new(),",
    "            show_export_all: false,\n            export_all_folder: String::new(),\n            show_export_queue: false,\n            export_queue: export_queue::ExportQueue::new(),\n            export_queue_open_folder_after: None,\n            show_tiff_inspector: false,\n            tiff_inspection: None,\n            tiff_inspect_error: None,",
    "ShadeApp production tool initialization",
)
text = replace_once(
    text,
    "            original_texture: None,\n            generation: 1,",
    "            original_texture: None,\n            embedded_original_texture: None,\n            embedded_original_status: PreviewColorStatus::Pending,\n            generation: 1,",
    "RuntimeFace cache initialization",
)

new_export_current = r'''    fn export_current_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let stem = face
            .path
            .file_stem()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("TIFF image", &["tif", "tiff"])
            .set_file_name(format!("{stem}-shade.tif"))
            .save_file()
        else {
            return;
        };
        let source = face.path.clone();
        let face_name = self
            .project
            .faces
            .get(self.current_face)
            .map(|face| face.label.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| stem.to_string());
        let state_name = self
            .project
            .active_snapshot_name()
            .unwrap_or("Working")
            .to_owned();
        self.remind_after_export = self.snapshot_project_needs_save_reminder();
        self.export_queue.enqueue(export_queue::ExportQueueSpec {
            label: format!("{face_name} / {state_name}"),
            source,
            destination,
            project: self.project.clone(),
            default_dpi: self.settings.default_dpi,
            force_lzw: self.settings.lzw_compression,
            validate_after_export: self.settings.validate_after_export,
            mark: None,
        });
        self.show_export_queue = true;
        self.report_info("Export added to queue");
    }

    fn poll_export_queue(&mut self) {
        let completions = self.export_queue.poll();
        if completions.is_empty() {
            return;
        }

        let mut completed = 0usize;
        let mut errors = Vec::new();
        for completion in completions {
            if let Some(mark) = completion.mark {
                self.project.record_snapshot_export(
                    mark.snapshot_id,
                    mark.face_key,
                    mark.folder.to_string_lossy().into_owned(),
                    unix_ms_now(),
                );
                self.project_dirty = true;
            }
            match completion.result {
                Ok(_) => completed += 1,
                Err(err) => errors.push(format!("Queue item #{}: {err}", completion.id)),
            }
        }

        if errors.is_empty() {
            self.report_info(format!("Export queue completed {completed} item(s)"));
        } else {
            self.report_error(errors.join(" | "));
        }

        if !self.export_queue.has_pending() {
            if self.remind_after_export {
                self.show_snapshot_save_reminder = true;
            }
            self.remind_after_export = false;
            if let Some(folder) = self.export_queue_open_folder_after.take() {
                if let Err(err) = open_folder(&folder) {
                    self.report_error(err);
                }
            }
        }
    }

    fn ui_export_queue_window(&mut self, ctx: &egui::Context) {
        if !self.show_export_queue {
            return;
        }
        let mut open = self.show_export_queue;
        let rows = self
            .export_queue
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
                )
            })
            .collect::<Vec<_>>();
        let pending = self.export_queue.pending_count();
        let mut cancel_id = None;
        let mut retry_id = None;
        let mut cancel_waiting = false;
        let mut clear_finished = false;

        egui::Window::new("Export Queue")
            .open(&mut open)
            .resizable(true)
            .default_size([760.0, 520.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Export Queue");
                    ui.label(format!("{pending} pending"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clear_finished = ui.button("Clear finished").clicked();
                        cancel_waiting = ui.button("Cancel waiting").clicked();
                    });
                });
                ui.small("Processing items finish their current atomic TIFF write safely. Cancel on a Processing item stops the queue after that file is safely committed.");
                ui.separator();

                if rows.is_empty() {
                    ui.label("No export jobs yet.");
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (id, label, destination, status, progress, detail, error) in &rows {
                            egui::Frame::new()
                                .inner_margin(8)
                                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                                .corner_radius(5)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.strong(label);
                                        ui.label(status.label());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            match status {
                                                export_queue::ExportQueueStatus::Waiting
                                                | export_queue::ExportQueueStatus::Processing => {
                                                    if ui.small_button("Cancel").clicked() {
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
                                        });
                                    });
                                    if matches!(
                                        status,
                                        export_queue::ExportQueueStatus::Waiting
                                            | export_queue::ExportQueueStatus::Processing
                                    ) {
                                        ui.add(
                                            egui::ProgressBar::new(*progress)
                                                .desired_width(f32::INFINITY)
                                                .text(if detail.trim().is_empty() {
                                                    status.label().to_owned()
                                                } else {
                                                    detail.clone()
                                                }),
                                        );
                                    } else if !detail.trim().is_empty() {
                                        ui.small(detail);
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

        self.show_export_queue = open;
        if let Some(id) = cancel_id {
            self.export_queue.cancel(id);
        }
        if let Some(id) = retry_id {
            self.export_queue.retry(id);
            self.show_export_queue = true;
        }
        if cancel_waiting {
            self.export_queue.cancel_all_waiting();
        }
        if clear_finished {
            self.export_queue.clear_finished();
        }
    }

    fn inspect_tiff_dialog(&mut self) {
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
        match tiff_inspect::inspect(&path, self.settings.default_dpi) {
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

    fn ui_tiff_inspector_window(&mut self, ctx: &egui::Context) {
        if !self.show_tiff_inspector {
            return;
        }
        let mut open = self.show_tiff_inspector;
        let report = self.tiff_inspection.as_ref().map(|item| item.report.clone());
        let path = self.tiff_inspection.as_ref().map(|item| item.path.clone());
        let error = self.tiff_inspect_error.clone();
        let mut copy_report = false;
        let mut reveal = false;
        let mut display_report = report.clone().unwrap_or_default();

        egui::Window::new("Inspect TIFF")
            .open(&mut open)
            .resizable(true)
            .default_size([820.0, 680.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("TIFF Production Diagnostics");
                    if ui.add_enabled(report.is_some(), egui::Button::new("Copy report")).clicked() {
                        copy_report = true;
                    }
                    if ui.add_enabled(path.is_some(), egui::Button::new("Reveal TIFF")).clicked() {
                        reveal = true;
                    }
                });
                if let Some(path) = path.as_ref() {
                    ui.small(path.display().to_string());
                }
                if let Some(error) = error.as_ref() {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if report.is_some() {
                    ui.separator();
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut display_report)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(34)
                                .interactive(false),
                        );
                    });
                }
            });

        self.show_tiff_inspector = open;
        if copy_report {
            if let Some(report) = report {
                ctx.copy_text(report);
                self.report_info("TIFF inspection report copied");
            }
        }
        if reveal {
            if let Some(path) = path {
                if let Err(err) = reveal_in_explorer(&path) {
                    self.report_error(err);
                }
            }
        }
    }

'''
text = replace_section(
    text,
    "    fn export_current_dialog(&mut self) {",
    "    fn validate_current_face_dialog(&mut self) {",
    new_export_current,
    "export current + queue + inspector integration",
)

new_start_export_all = r'''    fn start_export_all(&mut self) {
        if self.job.is_some() || self.faces.is_empty() {
            return;
        }
        let base_folder = PathBuf::from(self.export_all_folder.trim());
        if self.export_all_folder.trim().is_empty() {
            self.report_error("Choose an Export All folder first.");
            return;
        }
        if let Err(err) = std::fs::create_dir_all(&base_folder) {
            self.report_error(format!(
                "Cannot create Export All folder {}: {err}",
                base_folder.display()
            ));
            return;
        }
        if self.faces.iter().any(|face| !face.available) {
            self.report_error("Export all requires every Face source TIFF to be available. Relink missing Faces first.");
            return;
        }

        let sources = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        let face_names = self
            .project
            .faces
            .iter()
            .map(|face| face.label.clone())
            .collect::<Vec<_>>();
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let project_name = self.project.name.clone();
        let snapshot_code = self.project.effective_test_code_text();
        let template = self.settings.export_all_template.clone();
        let folder_template = self.settings.export_folder_template.clone();
        let conflict_policy = self.settings.export_all_conflict_policy;
        let open_after = self.settings.export_all_open_folder;
        let mut project = self.project.clone();
        project.test_code.enabled = self.settings.export_all_test_code;
        let date = Local::now().format("%Y-%m-%d").to_string();
        let mut reserved = BTreeSet::new();
        let mut queued = 0usize;
        let mut skipped = 0usize;

        for (index, source) in sources.iter().enumerate() {
            let face_name = face_names
                .get(index)
                .map(String::as_str)
                .filter(|name| !name.trim().is_empty())
                .or_else(|| source.file_stem().and_then(|value| value.to_str()))
                .unwrap_or("face");
            let source_name = source
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(face_name);
            let context = export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &project_name,
                snapshot_code: &snapshot_code,
                face_number: index + 1,
                face_name,
                source_name,
                date: &date,
            };
            let folder = export_batch::render_export_folder(
                &base_folder,
                &folder_template,
                &context,
            );
            if let Err(err) = std::fs::create_dir_all(&folder) {
                self.report_error(format!("Cannot create export folder {}: {err}", folder.display()));
                return;
            }
            let filename = export_batch::render_export_filename(&template, &context);
            let destination = match export_batch::resolve_destination_reserved(
                &folder,
                &filename,
                conflict_policy,
                &mut reserved,
            ) {
                export_batch::DestinationDecision::Write(path) => path,
                export_batch::DestinationDecision::Skip(_) => {
                    skipped += 1;
                    continue;
                }
            };
            self.export_queue.enqueue(export_queue::ExportQueueSpec {
                label: format!("{face_name} / {snapshot_code}"),
                source: source.clone(),
                destination,
                project: project.clone(),
                default_dpi: self.settings.default_dpi,
                force_lzw: self.settings.lzw_compression,
                validate_after_export: self.settings.validate_after_export,
                mark: None,
            });
            queued += 1;
        }

        self.show_export_all = false;
        self.show_export_queue = queued > 0;
        self.remind_after_export = queued > 0 && self.snapshot_project_needs_save_reminder();
        if open_after && queued > 0 {
            self.export_queue_open_folder_after = Some(base_folder.clone());
        }
        let _ = self.settings.save();
        if queued > 0 {
            self.report_info(if skipped > 0 {
                format!("Queued {queued} export(s) · skipped {skipped} existing file(s)")
            } else {
                format!("Queued {queued} export(s)")
            });
        } else if skipped > 0 {
            self.report_info(format!("No exports queued · skipped {skipped} existing file(s)"));
        }
    }

'''
text = replace_section(
    text,
    "    fn start_export_all(&mut self) {",
    "    fn ui_export_all_window(&mut self, ctx: &egui::Context) {",
    new_start_export_all,
    "Export All queue integration",
)

new_ui_export_all = r'''    fn ui_export_all_window(&mut self, ctx: &egui::Context) {
        if !self.show_export_all {
            return;
        }
        let mut open = self.show_export_all;
        let folder = PathBuf::from(self.export_all_folder.trim());
        let existing_tiffs = if folder.is_dir() {
            export_batch::folder_tiff_count(&folder)
        } else {
            0
        };
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let snapshot_code = self.project.effective_test_code_text();
        let first_source = self
            .faces
            .first()
            .map(|face| face.path.clone())
            .unwrap_or_else(|| PathBuf::from("source.tif"));
        let source_name = first_source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("source")
            .to_owned();
        let face_name = self
            .project
            .faces
            .first()
            .map(|face| face.label.as_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("face");
        let today = Local::now().format("%Y-%m-%d").to_string();
        let preview_context = export_batch::ExportNameContext {
            shade_name: shade_name.as_deref(),
            project_name: &self.project.name,
            snapshot_code: &snapshot_code,
            face_number: 1,
            face_name,
            source_name: &source_name,
            date: &today,
        };
        let preview_name = export_batch::render_export_filename(
            &self.settings.export_all_template,
            &preview_context,
        );
        let preview_folder = export_batch::render_export_folder(
            &folder,
            &self.settings.export_folder_template,
            &preview_context,
        );
        let mut browse = false;
        let mut reveal = false;
        let mut start = false;
        let mut cancel = false;
        let mut changed = false;

        egui::Window::new("Export All Faces")
            .open(&mut open)
            .resizable(true)
            .default_width(540.0)
            .min_width(480.0)
            .max_width(620.0)
            .show(ctx, |ui| {
                ui.strong("Export root folder");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.export_all_folder)
                            .desired_width(330.0),
                    );
                    browse = ui.button("Browse...").clicked();
                    reveal = ui
                        .add_enabled(folder.is_dir(), egui::Button::new("Reveal folder"))
                        .clicked();
                });
                if existing_tiffs > 0 {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!("Warning: this root already contains {existing_tiffs} TIFF file(s)."),
                    );
                }

                ui.add_space(8.0);
                ui.strong("File name template");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.settings.export_all_template)
                            .desired_width(500.0),
                    )
                    .changed();
                ui.small("Tokens: {project}, {face}, {snapshot}, {source}, {date}. Legacy tokens remain supported.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Preview:");
                    ui.monospace(&preview_name);
                });

                ui.add_space(8.0);
                ui.strong("Folder template");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.settings.export_folder_template)
                            .hint_text("Example: {project}/{date}/{snapshot}/")
                            .desired_width(500.0),
                    )
                    .changed();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Folder preview:");
                    ui.monospace(preview_folder.display().to_string());
                });

                ui.add_space(8.0);
                ui.strong("If a file already exists");
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::Overwrite,
                            "Overwrite",
                        )
                        .changed();
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::Skip,
                            "Skip",
                        )
                        .changed();
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::AutoNumber,
                            "Auto-number",
                        )
                        .changed();
                });

                ui.add_space(8.0);
                changed |= ui
                    .checkbox(
                        &mut self.settings.export_all_open_folder,
                        "Open root folder after queue finishes",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.export_all_test_code,
                        "Write Test Code on every exported Face",
                    )
                    .changed();

                ui.separator();
                ui.horizontal(|ui| {
                    start = ui
                        .add_enabled(
                            !self.export_all_folder.trim().is_empty()
                                && self.job.is_none()
                                && !self.faces.is_empty(),
                            egui::Button::new("Add all to Queue"),
                        )
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });

        if cancel {
            open = false;
        }
        self.show_export_all = open;
        if changed {
            self.settings.sanitize();
            if let Err(err) = self.settings.save() {
                self.log.error(&err);
            }
        }
        if browse {
            let mut dialog = rfd::FileDialog::new();
            if folder.is_dir() {
                dialog = dialog.set_directory(&folder);
            }
            if let Some(selected) = dialog.pick_folder() {
                self.export_all_folder = selected.to_string_lossy().into_owned();
            }
        }
        if reveal && folder.is_dir() {
            if let Err(err) = open_folder(&folder) {
                self.report_error(err);
            }
        }
        if start {
            self.start_export_all();
        }
    }

'''
text = replace_section(
    text,
    "    fn ui_export_all_window(&mut self, ctx: &egui::Context) {",
    "    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {",
    new_ui_export_all,
    "Export All template UI",
)

new_snapshot_export = r'''    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {
        if self.job.is_some() {
            return;
        }
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let Some(snapshot) = self
            .project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == snapshot_id)
            .cloned()
        else {
            return;
        };
        let stem = face
            .path
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "face".to_owned());
        let suggested = format!(
            "{}-{}.tif",
            sanitize_filename(&stem),
            sanitize_filename(&snapshot.name)
        );
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("TIFF image", &["tif", "tiff"])
            .set_file_name(suggested)
            .save_file()
        else {
            return;
        };
        let source = face.path.clone();
        let face_key = source.to_string_lossy().into_owned();
        let folder = destination
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut project = self.project.clone();
        project.adjustments = snapshot.adjustments.clone();
        project.active_snapshot_id = Some(snapshot.id);
        self.remind_after_export = self.snapshot_project_needs_save_reminder();
        self.export_queue.enqueue(export_queue::ExportQueueSpec {
            label: format!("Face {} / {}", self.current_face + 1, snapshot.name),
            source,
            destination,
            project,
            default_dpi: self.settings.default_dpi,
            force_lzw: self.settings.lzw_compression,
            validate_after_export: self.settings.validate_after_export,
            mark: Some(export_queue::ExportQueueMark {
                snapshot_id,
                face_key,
                folder,
            }),
        });
        self.show_export_queue = true;
        self.report_info("Snapshot export added to queue");
    }

    fn export_snapshot_group_dialog(&mut self, snapshot_ids: Vec<u64>, label: String) {
        if self.job.is_some() || snapshot_ids.is_empty() {
            return;
        }
        if !workflow::active_face_available(self) {
            self.report_error(
                "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
            );
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        let Some(base_folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let source = face.path.clone();
        let face_key = source.to_string_lossy().into_owned();
        let source_name = source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("face")
            .to_owned();
        let face_name = self
            .project
            .faces
            .get(self.current_face)
            .map(|face| face.label.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| source_name.clone());
        let snapshots = snapshot_ids
            .into_iter()
            .filter_map(|id| {
                self.project
                    .snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .cloned()
            })
            .collect::<Vec<_>>();
        if snapshots.is_empty() {
            return;
        }
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let project_name = self.project.name.clone();
        let date = Local::now().format("%Y-%m-%d").to_string();
        let mut reserved = BTreeSet::new();
        let mut queued = 0usize;
        let mut skipped = 0usize;

        for snapshot in snapshots {
            let mut project = self.project.clone();
            project.adjustments = snapshot.adjustments.clone();
            project.active_snapshot_id = Some(snapshot.id);
            let snapshot_code = project.effective_test_code_text();
            let context = export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &project_name,
                snapshot_code: &snapshot_code,
                face_number: self.current_face + 1,
                face_name: &face_name,
                source_name: &source_name,
                date: &date,
            };
            let folder = export_batch::render_export_folder(
                &base_folder,
                &self.settings.export_folder_template,
                &context,
            );
            if let Err(err) = std::fs::create_dir_all(&folder) {
                self.report_error(format!("Cannot create export folder {}: {err}", folder.display()));
                return;
            }
            let filename = export_batch::render_export_filename(
                &self.settings.export_all_template,
                &context,
            );
            let destination = match export_batch::resolve_destination_reserved(
                &folder,
                &filename,
                self.settings.export_all_conflict_policy,
                &mut reserved,
            ) {
                export_batch::DestinationDecision::Write(path) => path,
                export_batch::DestinationDecision::Skip(_) => {
                    skipped += 1;
                    continue;
                }
            };
            self.export_queue.enqueue(export_queue::ExportQueueSpec {
                label: format!("{face_name} / {}", snapshot.name),
                source: source.clone(),
                destination,
                project,
                default_dpi: self.settings.default_dpi,
                force_lzw: self.settings.lzw_compression,
                validate_after_export: self.settings.validate_after_export,
                mark: Some(export_queue::ExportQueueMark {
                    snapshot_id: snapshot.id,
                    face_key: face_key.clone(),
                    folder,
                }),
            });
            queued += 1;
        }

        if queued > 0 {
            self.remind_after_export = self.snapshot_project_needs_save_reminder();
            self.show_export_queue = true;
            self.report_info(if skipped > 0 {
                format!("Queued {queued} snapshot(s) ({label}) · skipped {skipped}")
            } else {
                format!("Queued {queued} snapshot(s) ({label})")
            });
        }
    }

'''
text = replace_section(
    text,
    "    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {",
    "    fn ensure_project_palette_for_model(&mut self, color_model: tiff_io::ColorModel) -> bool {",
    new_snapshot_export,
    "Snapshot queue integration",
)

new_poll_render = r'''    fn poll_render(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.render_rx.try_recv() {
            if self.render_busy == Some((result.face_index, result.generation)) {
                self.render_busy = None;
            }
            let Some(face) = self.faces.get_mut(result.face_index) else {
                continue;
            };
            if face.generation != result.generation {
                continue;
            }
            face.adjusted = result.adjusted;
            face.clipping = result.clipping;
            face.color_status = result.color_status;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.rgba,
            );
            let options = egui::TextureOptions::LINEAR;
            if let Some(texture) = &mut face.texture {
                texture.set(image, options);
            } else {
                face.texture = Some(ctx.load_texture(
                    format!("face-preview-{}", result.face_index),
                    image,
                    options,
                ));
            }
            let original_image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.original_rgba,
            );
            if let Some(texture) = &mut face.original_texture {
                texture.set(original_image, options);
            } else {
                face.original_texture = Some(ctx.load_texture(
                    format!("face-original-preview-{}", result.face_index),
                    original_image,
                    options,
                ));
            }
            if let Some(source_rgba) = result.embedded_original_rgba {
                let source_image = egui::ColorImage::from_rgba_unmultiplied(
                    [face.preview.width, face.preview.height],
                    &source_rgba,
                );
                if let Some(texture) = &mut face.embedded_original_texture {
                    texture.set(source_image, options);
                } else {
                    face.embedded_original_texture = Some(ctx.load_texture(
                        format!("face-embedded-source-preview-{}", result.face_index),
                        source_image,
                        options,
                    ));
                }
            }
            if let Some(status) = result.embedded_original_status {
                face.embedded_original_status = status;
            }
            face.rendered_generation = result.generation;
        }
    }

    fn start_render_if_needed(&mut self, ctx: &egui::Context) {
        if self.render_busy.is_some() || ctx.input(|input| input.pointer.any_down()) {
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        if !face.available {
            return;
        }
        if face.rendered_generation == face.generation {
            return;
        }
        let face_index = self.current_face;
        let generation = face.generation;
        let needs_embedded_original = face.embedded_original_texture.is_none();
        let preview = Arc::clone(&face.preview);
        let project = self.project.clone();
        let solo_channel = self.solo_channel;
        let color_config = PreviewColorConfig::from_project(&self.project);
        let tx = self.render_tx.clone();
        self.render_busy = Some((face_index, generation));
        std::thread::spawn(move || {
            let (adjusted, clipping) = render::adjusted_planes_with_stats(&preview, &project);
            let color =
                color_management::PreviewColorTransform::new(&preview.metadata, color_config);
            let rgba =
                render::rgba_from_planes_with_color(&preview, &adjusted, solo_channel, &color);
            let original_rgba = render::rgba_from_planes_with_color(
                &preview,
                &preview.channels,
                solo_channel,
                &color,
            );
            let color_status = color.status().clone();

            let (embedded_original_rgba, embedded_original_status) = if needs_embedded_original {
                let embedded_color = color_management::PreviewColorTransform::new(
                    &preview.metadata,
                    PreviewColorConfig {
                        enabled: true,
                        intent: PreviewRenderingIntent::Perceptual,
                        black_point_compensation: false,
                        assigned_profile_path: None,
                        soft_proof_enabled: false,
                        proof_profile_path: None,
                        proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
                    },
                );
                let status = embedded_color.status().clone();
                let source_rgba = render::rgba_from_planes_with_color(
                    &preview,
                    &preview.channels,
                    None,
                    &embedded_color,
                );
                (Some(source_rgba), Some(status))
            } else {
                (None, None)
            };

            let _ = tx.send(RenderResult {
                face_index,
                generation,
                adjusted,
                clipping,
                color_status,
                rgba,
                original_rgba,
                embedded_original_rgba,
                embedded_original_status,
            });
        });
    }

'''
text = replace_section(
    text,
    "    fn poll_render(&mut self, ctx: &egui::Context) {",
    "    fn select_channel(&mut self, channel: usize, isolate: bool) {",
    new_poll_render,
    "embedded source render cache",
)

new_toolbar = r'''    fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut dismiss_error = false;
        let mut inspect_requested = false;
        let mut queue_requested = false;
        ui.horizontal(|ui| {
            ui.horizontal_wrapped(|ui| {
                let enabled = self.job.is_none();
                ui.menu_button("File", |ui| {
                    if ui.add_enabled(enabled, egui::Button::new("New project")).clicked() {
                        self.new_project();
                    }
                    if ui.add_enabled(enabled, egui::Button::new("Open .shade...")).clicked() {
                        self.open_project_dialog();
                    }
                    if ui.add_enabled(enabled, egui::Button::new("Add TIFF faces...")).clicked() {
                        self.add_faces_dialog();
                    }
                    ui.separator();
                    if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save")).clicked() {
                        self.save_project(false);
                    }
                    if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save As...")).clicked() {
                        self.save_project(true);
                    }
                    ui.separator();
                    if ui.button("Inspect TIFF...").clicked() {
                        inspect_requested = true;
                    }
                    if ui.button("Export Queue").clicked() {
                        queue_requested = true;
                    }
                });
                if ui.add_enabled(enabled, egui::Button::new("New")).clicked() { self.new_project(); }
                if ui.add_enabled(enabled, egui::Button::new("Open .shade")).clicked() { self.open_project_dialog(); }
                if ui.button("Project View").clicked() { self.show_previous_shades = true; }
                if ui.add_enabled(enabled, egui::Button::new("Add TIFF faces")).clicked() { self.add_faces_dialog(); }
                ui.separator();
                if self.project_path.is_none() && ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Quick Save")).on_hover_text("Create the first .shade project beside the source TIFF files without opening a Save dialog").clicked() { self.quick_save_project(); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save")).clicked() { self.save_project(false); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save As")).clicked() { self.save_project(true); }
                ui.separator();
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Export face")).clicked() { self.export_current_dialog(); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Export all")).clicked() { self.export_all_dialog(); }
                let queue_label = format!("Queue ({})", self.export_queue.pending_count());
                if ui.button(queue_label).clicked() { self.show_export_queue = true; }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Validate face")).on_hover_text("Run a no-adjustment export through the production TIFF backend, re-decode it, and compare pixels plus critical Photoshop/TIFF metadata.").clicked() { self.validate_current_face_dialog(); }
                ui.separator();
                if ui.button("Settings").clicked() { self.show_settings = true; }
                if ui.button("About").clicked() { self.show_about = true; }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.ui_operation_progress(ui);
                if ui.small_button("Logs").clicked() { self.log_cache = self.log.read(); self.show_logs = true; }
                self.ui_update_compact(ui);
                if let Some(toast) = &self.toast {
                    ui.horizontal(|ui| {
                        dismiss_error = ui.small_button("x").on_hover_text("Dismiss error").clicked();
                        let full = toast.message.clone();
                        let mut compact = full.chars().take(56).collect::<String>();
                        if full.chars().count() > 56 { compact.push('…'); }
                        ui.label(egui::RichText::new(compact).color(egui::Color32::LIGHT_RED).small()).on_hover_text(full);
                    });
                }
            });
        });
        if inspect_requested {
            self.inspect_tiff_dialog();
        }
        if queue_requested {
            self.show_export_queue = true;
        }
        if dismiss_error {
            self.toast = None;
            if self.status_message == "Error - see Logs" {
                self.status_message = "Ready".to_owned();
            }
        }
    }

'''
text = replace_section(
    text,
    "    fn ui_toolbar(&mut self, ui: &mut egui::Ui) {",
    "    fn ui_operation_progress(&self, ui: &mut egui::Ui) {",
    new_toolbar,
    "File menu and Queue toolbar integration",
)

text = replace_once(
    text,
    "        if self.render_busy.is_some() {\n            ui.add(\n                egui::ProgressBar::new(0.45)",
    "        if let Some((value, text)) = self.export_queue.active_summary() {\n            ui.add(\n                egui::ProgressBar::new(value)\n                    .desired_width(300.0)\n                    .text(text),\n            );\n            return;\n        }\n        if self.render_busy.is_some() {\n            ui.add(\n                egui::ProgressBar::new(0.45)",
    "queue progress in toolbar",
)

new_viewport = r'''    fn ui_viewport(&mut self, ui: &mut egui::Ui) {
        if workflow::ui_missing_viewport(self, ui) {
            return;
        }

        let Some(face) = self.faces.get(self.current_face) else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Shade Editor");
                    ui.label("Open a .shade project or add TIFF faces.");
                    if ui.button("Add TIFF faces").clicked() {
                        self.add_faces_dialog();
                    }
                });
            });
            return;
        };

        let file_name = face
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| face.path.display().to_string());
        let meta = face.preview.metadata.clone();
        let dpi_info = face.dpi;
        let color_status = face.color_status.clone();
        let embedded_original_status = face.embedded_original_status.clone();
        let texture = face.texture.clone();
        let original_texture = face.original_texture.clone();
        let embedded_original_texture = face.embedded_original_texture.clone();
        let (width_cm, height_cm) = physical_dimensions_cm(meta.width, meta.height, dpi_info);
        ui.horizontal_wrapped(|ui| {
            ui.strong(file_name);
            ui.separator();
            ui.label(format!("{}-bit", meta.bit_depth));
            ui.label(format!(
                "{} x {} cm",
                format_cm_value(width_cm),
                format_cm_value(height_cm)
            ));
            ui.label(format!("{} x {} px", meta.width, meta.height));
            if dpi_info.has_physical_resolution {
                ui.label(format!("{:.0} x {:.0} DPI", dpi_info.dpi_x, dpi_info.dpi_y));
            } else {
                ui.label(format!(
                    "{:.0} x {:.0} DPI (default)",
                    dpi_info.dpi_x, dpi_info.dpi_y
                ));
            }
            ui.label(meta.color_model.title());
            ui.label(format!("{} channels", meta.samples_per_pixel));
            let profile_text = if color_status.is_problem() {
                egui::RichText::new(color_status.button_label()).color(egui::Color32::YELLOW)
            } else if color_status.is_managed() {
                egui::RichText::new(color_status.button_label()).color(egui::Color32::LIGHT_GREEN)
            } else {
                egui::RichText::new(color_status.button_label())
            };
            let icc_response = ui.small_button(profile_text);
            let open_color_management = icc_response.clicked();
            icc_response.on_hover_text(format!(
                "{}\nClick to manage source ICC and Printer/RIP Soft Proof.",
                color_status.detail()
            ));
            if open_color_management {
                self.show_color_management = true;
                self.icc_profile_selected =
                    self.project.preview_color.assigned_profile_path.clone();
            }
        });
        ui.small("Hold right mouse: BEFORE adjustments with current color-management setup · Hold middle mouse: original TIFF samples with Embedded ICC only (cached, no assigned profile / RIP Soft Proof).");
        ui.separator();

        let Some(texture) = texture else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
            return;
        };

        let visible = ui.available_size().max(egui::vec2(1.0, 1.0));
        if self.fit_requested {
            let natural = texture.size_vec2();
            if natural.x > 0.0 && natural.y > 0.0 {
                self.zoom = ((visible.x - 30.0).max(1.0) / natural.x)
                    .min((visible.y - 30.0).max(1.0) / natural.y)
                    .clamp(0.05, 8.0);
            }
            self.fit_requested = false;
            self.viewport_recenter = true;
        }

        let image_size = texture.size_vec2() * self.zoom;
        let recenter = self.viewport_recenter;
        let output = egui::ScrollArea::both()
            .id_salt("image-viewport")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show_viewport(ui, |ui, viewport| {
                let canvas_size = egui::vec2(
                    (image_size.x + VIEWPORT_OVERSCROLL * 2.0)
                        .max(viewport.width() + VIEWPORT_OVERSCROLL * 2.0),
                    (image_size.y + VIEWPORT_OVERSCROLL * 2.0)
                        .max(viewport.height() + VIEWPORT_OVERSCROLL * 2.0),
                );
                let (canvas_rect, _) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
                let image_rect = egui::Rect::from_center_size(canvas_rect.center(), image_size);
                let pointer_over = ui.rect_contains_pointer(image_rect);
                let show_embedded_source = ui.input(|input| input.pointer.middle_down()) && pointer_over;
                let show_before = !show_embedded_source
                    && ui.input(|input| input.pointer.secondary_down())
                    && pointer_over;
                let display_texture = if show_embedded_source {
                    embedded_original_texture
                        .as_ref()
                        .or(original_texture.as_ref())
                        .unwrap_or(&texture)
                } else if show_before {
                    original_texture.as_ref().unwrap_or(&texture)
                } else {
                    &texture
                };
                ui.put(
                    image_rect,
                    egui::Image::from_texture(display_texture).fit_to_exact_size(image_size),
                );
                if show_embedded_source {
                    let source_label = if embedded_original_texture.is_none() {
                        "SOURCE · PREPARING"
                    } else if embedded_original_status.is_managed() {
                        "SOURCE · EMBEDDED ICC"
                    } else if embedded_original_status.is_problem() {
                        "SOURCE · ICC FALLBACK"
                    } else {
                        "SOURCE · NO EMBEDDED ICC"
                    };
                    ui.painter().text(
                        image_rect.left_top() + egui::vec2(10.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        source_label,
                        egui::FontId::proportional(13.0),
                        egui::Color32::WHITE,
                    );
                } else if show_before {
                    ui.painter().text(
                        image_rect.left_top() + egui::vec2(10.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        "BEFORE",
                        egui::FontId::proportional(13.0),
                        egui::Color32::WHITE,
                    );
                }
                if recenter {
                    ui.scroll_to_rect(image_rect, Some(egui::Align::Center));
                }
            });
        let _ = output;
        if recenter {
            self.viewport_recenter = false;
        }
    }

'''
text = replace_section(
    text,
    "    fn ui_viewport(&mut self, ui: &mut egui::Ui) {",
    "    fn ui_status(&mut self, ui: &mut egui::Ui) {",
    new_viewport,
    "middle mouse embedded source viewport",
)

new_color_management = r'''    fn ui_color_management_window(&mut self, ctx: &egui::Context) {
        if !self.show_color_management {
            return;
        }
        if !self.icc_profile_scan_done {
            self.refresh_icc_profile_catalog();
        }

        let Some(active_face) = self.faces.get(self.current_face) else {
            self.show_color_management = false;
            return;
        };
        let active_model = active_face.preview.metadata.color_model;
        let embedded_name =
            color_management::embedded_profile_description(&active_face.preview.metadata)
                .unwrap_or_else(|| "No embedded ICC".to_owned());
        let profiles = self.icc_profiles.clone();
        let scan_error = self.icc_profile_scan_error.clone();
        let current_status = active_face.color_status.clone();

        let original_query = self.icc_profile_query.clone();
        let mut query = original_query.clone();
        let mut selected = self
            .icc_profile_selected
            .clone()
            .or_else(|| self.project.preview_color.assigned_profile_path.clone());
        let mut enabled = self.project.preview_color.enabled;
        let mut intent = self.project.preview_color.rendering_intent;
        let mut bpc = self.project.preview_color.black_point_compensation;
        let mut soft_proof_enabled = self.project.preview_color.soft_proof_enabled;
        let mut proofing_intent = self.project.preview_color.proofing_intent;
        let proof_path = self.project.preview_color.proof_profile_path.clone();
        let mut show_incompatible = self.icc_show_incompatible;
        let mut requested_profile: Option<Option<PathBuf>> = None;
        let mut requested_proof: Option<Option<PathBuf>> = None;
        let mut browse_requested = false;
        let mut browse_proof_requested = false;
        let mut refresh_requested = false;
        let mut open = self.show_color_management;

        egui::Window::new("Color Management / ICC Preview")
            .open(&mut open)
            .resizable(true)
            .default_size([820.0, 760.0])
            .show(ctx, |ui| {
                ui.heading("Color Management / ICC Preview");
                ui.small("Source-profile assignment and Printer/RIP Soft Proof are display-only. TIFF samples, embedded ICC and Photoshop resources remain untouched by these preview settings.");
                ui.add_space(5.0);
                egui::Grid::new("preview-profile-current")
                    .num_columns(2)
                    .striped(true)
                    .spacing([14.0, 5.0])
                    .show(ui, |ui| {
                        ui.strong("Active preview");
                        ui.label(current_status.button_label())
                            .on_hover_text(current_status.detail());
                        ui.end_row();
                        ui.strong("TIFF base model");
                        ui.label(active_model.title());
                        ui.end_row();
                        ui.strong("Embedded profile");
                        ui.label(&embedded_name);
                        ui.end_row();
                        ui.strong("Assigned source profile");
                        ui.label(
                            self.project
                                .preview_color
                                .assigned_profile_path
                                .as_deref()
                                .unwrap_or("Embedded profile"),
                        );
                        ui.end_row();
                        ui.strong("Printer / RIP proof");
                        ui.label(proof_path.as_deref().unwrap_or("Not selected"));
                        ui.end_row();
                    });

                ui.separator();
                ui.heading("Document / source ICC");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Use embedded profile").clicked() {
                        requested_profile = Some(None);
                    }
                    if ui.button("Browse source ICC / ICM...").clicked() {
                        browse_requested = true;
                    }
                    if ui.button("Refresh system profiles").clicked() {
                        refresh_requested = true;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut enabled, "Enable color-managed preview");
                    ui.checkbox(&mut bpc, "Black point compensation");
                });
                egui::ComboBox::from_label("Source rendering intent")
                    .selected_text(intent.label())
                    .show_ui(ui, |ui| {
                        for value in [
                            PreviewRenderingIntent::Perceptual,
                            PreviewRenderingIntent::RelativeColorimetric,
                            PreviewRenderingIntent::Saturation,
                            PreviewRenderingIntent::AbsoluteColorimetric,
                        ] {
                            ui.selectable_value(&mut intent, value, value.label());
                        }
                    });

                ui.horizontal(|ui| {
                    ui.label("Search source profiles");
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut query)
                            .hint_text("Profile name, filename, RGB/CMYK/Gray, path")
                            .desired_width(430.0),
                    );
                    if !search.has_focus() && !ctx.wants_keyboard_input() {
                        let typed = ctx.input(|input| {
                            input
                                .events
                                .iter()
                                .filter_map(|event| match event {
                                    egui::Event::Text(text) if !text.chars().all(char::is_control) => {
                                        Some(text.as_str())
                                    }
                                    _ => None,
                                })
                                .collect::<String>()
                        });
                        if !typed.is_empty() {
                            query.push_str(&typed);
                            search.request_focus();
                        }
                    }
                    ui.checkbox(&mut show_incompatible, "Show incompatible");
                });

                let visible = profiles
                    .iter()
                    .filter(|profile| profile.matches_query(&query))
                    .filter(|profile| show_incompatible || profile.compatible_with(active_model))
                    .collect::<Vec<_>>();
                let compatible_paths = visible
                    .iter()
                    .filter(|profile| profile.compatible_with(active_model))
                    .map(|profile| profile.path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                if query != original_query {
                    selected = compatible_paths.first().cloned();
                }
                let current_position = selected
                    .as_deref()
                    .and_then(|path| compatible_paths.iter().position(|item| item == path));
                let (up, down, enter) = ctx.input(|input| {
                    (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                        input.key_pressed(egui::Key::Enter),
                    )
                });
                if !compatible_paths.is_empty() && (up || down) {
                    let next = match (current_position, up, down) {
                        (Some(position), true, _) => position.saturating_sub(1),
                        (Some(position), _, true) => (position + 1).min(compatible_paths.len() - 1),
                        (None, _, true) => 0,
                        (None, true, _) => compatible_paths.len() - 1,
                        _ => 0,
                    };
                    selected = compatible_paths.get(next).cloned();
                }
                if enter {
                    if let Some(path) = selected.as_ref() {
                        requested_profile = Some(Some(PathBuf::from(path)));
                    }
                }

                if let Some(error) = scan_error.as_ref() {
                    ui.colored_label(egui::Color32::YELLOW, error);
                }
                egui::ScrollArea::vertical()
                    .id_salt("icc-profile-list")
                    .auto_shrink([false, false])
                    .max_height(210.0)
                    .show(ui, |ui| {
                        for profile in visible {
                            let path_text = profile.path.to_string_lossy().into_owned();
                            let compatible = profile.compatible_with(active_model);
                            let label = format!(
                                "{} · {} · {}",
                                profile.description,
                                profile.color_space_label(),
                                profile.filename()
                            );
                            let response = ui
                                .add_enabled(
                                    compatible,
                                    egui::Button::new(label)
                                        .selected(selected.as_deref() == Some(path_text.as_str())),
                                )
                                .on_hover_text(format!(
                                    "{}\nClass: {}",
                                    profile.path.display(),
                                    profile.device_class_label(),
                                ));
                            if response.clicked() {
                                selected = Some(path_text.clone());
                            }
                            if response.double_clicked() && compatible {
                                requested_profile = Some(Some(profile.path.clone()));
                            }
                        }
                    });
                let can_assign = selected.as_deref().is_some_and(|path| {
                    profiles.iter().any(|profile| {
                        profile.path.to_string_lossy() == path
                            && profile.compatible_with(active_model)
                    })
                });
                if ui
                    .add_enabled(can_assign, egui::Button::new("Assign selected source profile"))
                    .clicked()
                {
                    requested_profile = selected
                        .as_ref()
                        .map(|path| Some(PathBuf::from(path)));
                }

                ui.separator();
                ui.heading("Printer / RIP Soft Proof");
                ui.small("Select an Output-class printer/RIP ICC. Shade Editor uses a LittleCMS proofing transform only for the viewport and project thumbnail; export remains separation/sample preserving.");
                let output_profiles = profiles
                    .iter()
                    .filter(|profile| profile.is_output_profile())
                    .collect::<Vec<_>>();
                let proof_selected_text = proof_path
                    .as_deref()
                    .and_then(|path| {
                        output_profiles
                            .iter()
                            .find(|profile| profile.path.to_string_lossy() == path)
                            .map(|profile| profile.description.clone())
                    })
                    .or_else(|| proof_path.clone())
                    .unwrap_or_else(|| "Choose printer/RIP output profile".to_owned());
                egui::ComboBox::from_label("Installed output profile")
                    .selected_text(proof_selected_text)
                    .width(520.0)
                    .show_ui(ui, |ui| {
                        for profile in &output_profiles {
                            let selected_now = proof_path.as_deref()
                                == Some(profile.path.to_string_lossy().as_ref());
                            if ui
                                .selectable_label(
                                    selected_now,
                                    format!(
                                        "{} · {} · {}",
                                        profile.description,
                                        profile.color_space_label(),
                                        profile.filename()
                                    ),
                                )
                                .clicked()
                            {
                                requested_proof = Some(Some(profile.path.clone()));
                            }
                        }
                    });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Browse Printer/RIP ICC...").clicked() {
                        browse_proof_requested = true;
                    }
                    if ui
                        .add_enabled(proof_path.is_some(), egui::Button::new("Clear proof profile"))
                        .clicked()
                    {
                        requested_proof = Some(None);
                        soft_proof_enabled = false;
                    }
                    ui.checkbox(&mut soft_proof_enabled, "Enable Soft Proof");
                });
                egui::ComboBox::from_label("Proof rendering intent")
                    .selected_text(proofing_intent.label())
                    .show_ui(ui, |ui| {
                        for value in [
                            PreviewRenderingIntent::Perceptual,
                            PreviewRenderingIntent::RelativeColorimetric,
                            PreviewRenderingIntent::Saturation,
                            PreviewRenderingIntent::AbsoluteColorimetric,
                        ] {
                            ui.selectable_value(&mut proofing_intent, value, value.label());
                        }
                    });
                if soft_proof_enabled && proof_path.is_none() && requested_proof.is_none() {
                    ui.colored_label(egui::Color32::YELLOW, "Soft Proof is enabled but no printer/RIP profile is selected.");
                }
                ui.small("Middle-mouse source preview deliberately bypasses assigned source profiles and Soft Proof and uses only the TIFF's embedded ICC.");
            });

        self.show_color_management = open;
        self.icc_profile_query = query;
        self.icc_profile_selected = selected;
        self.icc_show_incompatible = show_incompatible;

        if refresh_requested {
            self.icc_profile_scan_done = false;
            self.refresh_icc_profile_catalog();
        }
        if browse_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_profile = Some(Some(path));
            }
        }
        if browse_proof_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_proof = Some(Some(path));
            }
        }

        let mut changed = false;
        if self.project.preview_color.enabled != enabled {
            self.project.preview_color.enabled = enabled;
            changed = true;
        }
        if self.project.preview_color.rendering_intent != intent {
            self.project.preview_color.rendering_intent = intent;
            changed = true;
        }
        if self.project.preview_color.black_point_compensation != bpc {
            self.project.preview_color.black_point_compensation = bpc;
            changed = true;
        }
        if self.project.preview_color.soft_proof_enabled != soft_proof_enabled {
            self.project.preview_color.soft_proof_enabled = soft_proof_enabled;
            changed = true;
        }
        if self.project.preview_color.proofing_intent != proofing_intent {
            self.project.preview_color.proofing_intent = proofing_intent;
            changed = true;
        }

        if let Some(requested) = requested_profile {
            match requested {
                None => {
                    if self.project.preview_color.assigned_profile_path.is_some() {
                        self.project.preview_color.assigned_profile_path = None;
                        self.icc_profile_selected = None;
                        changed = true;
                    }
                }
                Some(path) => match color_management::inspect_profile(&path) {
                    Ok(profile) if profile.compatible_with(active_model) => {
                        let path_text = path.to_string_lossy().into_owned();
                        if self.project.preview_color.assigned_profile_path.as_deref()
                            != Some(path_text.as_str())
                        {
                            self.project.preview_color.assigned_profile_path =
                                Some(path_text.clone());
                            self.icc_profile_selected = Some(path_text);
                            changed = true;
                        }
                    }
                    Ok(profile) => self.report_error(format!(
                        "Cannot assign '{}': profile color space {} does not match active TIFF {}.",
                        profile.description,
                        profile.color_space_label(),
                        active_model.title(),
                    )),
                    Err(err) => self.report_error(err),
                },
            }
        }

        if let Some(requested) = requested_proof {
            match requested {
                None => {
                    if self.project.preview_color.proof_profile_path.is_some() {
                        self.project.preview_color.proof_profile_path = None;
                        self.project.preview_color.soft_proof_enabled = false;
                        changed = true;
                    }
                }
                Some(path) => match color_management::inspect_profile(&path) {
                    Ok(profile) if profile.is_output_profile() => {
                        let path_text = path.to_string_lossy().into_owned();
                        if self.project.preview_color.proof_profile_path.as_deref()
                            != Some(path_text.as_str())
                        {
                            self.project.preview_color.proof_profile_path = Some(path_text);
                            changed = true;
                        }
                        if !self.project.preview_color.soft_proof_enabled {
                            self.project.preview_color.soft_proof_enabled = true;
                            changed = true;
                        }
                    }
                    Ok(profile) => self.report_error(format!(
                        "Cannot use '{}' for Soft Proof: profile class is {}, not Output/Printer.",
                        profile.description,
                        profile.device_class_label(),
                    )),
                    Err(err) => self.report_error(err),
                },
            }
        }

        if changed {
            self.project_dirty = true;
            self.invalidate_display_previews();
        }
    }

'''
text = replace_section(
    text,
    "    fn ui_color_management_window(&mut self, ctx: &egui::Context) {",
    "    fn ui_settings_window(&mut self, ctx: &egui::Context) {",
    new_color_management,
    "Printer/RIP Soft Proof UI",
)

text = replace_once(
    text,
    "        self.poll_job();\n        self.poll_render(ui.ctx());",
    "        self.poll_job();\n        self.poll_export_queue();\n        self.poll_render(ui.ctx());",
    "queue polling",
)
text = replace_once(
    text,
    "        self.ui_export_all_window(ui.ctx());\n        self.ui_recovery_window(ui.ctx());",
    "        self.ui_export_all_window(ui.ctx());\n        self.ui_export_queue_window(ui.ctx());\n        self.ui_tiff_inspector_window(ui.ctx());\n        self.ui_recovery_window(ui.ctx());",
    "production windows",
)

MAIN.write_text(text, encoding="utf-8")

cargo = ROOT / "Cargo.toml"
cargo_text = cargo.read_text(encoding="utf-8")
cargo_text = replace_once(
    cargo_text,
    'version = "0.17.0"',
    'version = "0.17.1"',
    "version bump",
)
cargo.write_text(cargo_text, encoding="utf-8")

notes = ROOT / "RELEASE_NOTES.md"
notes_text = notes.read_text(encoding="utf-8")
header = """# Shade Editor 0.17.1\n\n- Wire the previously-added Export Queue and TIFF Inspector backends into the actual application UI and export workflows.\n- Expose Printer/RIP Soft Proof in Color Management with Output-class profile selection, proof intent and persistent enable state.\n- Add `File > Inspect TIFF` plus a report window with Copy report and Explorer reveal.\n- Route Face, Export All and Snapshot batch exports through the non-blocking queue with Cancel/Retry and safe atomic completion semantics.\n- Expose filename and folder templates in Export All, including `{project}`, `{face}`, `{snapshot}`, `{source}` and `{date}`.\n- Add a cached middle-mouse SOURCE preview: original TIFF samples rendered only through the TIFF embedded ICC, bypassing edits, assigned source ICC and Printer/RIP Soft Proof. Right mouse remains BEFORE using the current preview color-management setup.\n\n"""
if not notes_text.startswith("# Shade Editor 0.17.1"):
    notes.write_text(header + notes_text, encoding="utf-8")

readme = ROOT / "README.md"
readme_text = readme.read_text(encoding="utf-8")
needle = "- True printer/RIP **Soft Proof** using an output-device ICC proofing transform; proof settings remain preview-only.\n"
if needle in readme_text and "middle-mouse" not in readme_text.lower():
    readme_text = readme_text.replace(
        needle,
        needle + "- Hold the middle mouse button over the viewport for a cached original-source preview using only the TIFF embedded ICC; right mouse remains the current-color-management BEFORE preview.\n",
        1,
    )
    readme.write_text(readme_text, encoding="utf-8")

print("v0.17.1 integration migration applied")
