use crate::model::AdjustmentSnapshot;
use crate::*;
use eframe::egui;

pub(crate) fn ui_snapshots(app: &mut ShadeApp, ui: &mut egui::Ui) {
    let face_key = app
        .faces
        .get(app.current_face)
        .map(|face| face.path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let rows = app
        .project
        .snapshots
        .iter()
        .map(|snapshot| {
            let export = app
                .project
                .snapshot_export_for_face(snapshot.id, &face_key)
                .map(|record| (record.folder.clone(), record.exported_at_unix_ms));
            (
                snapshot.id,
                snapshot.name.clone(),
                snapshot.created_at_unix_ms,
                export,
            )
        })
        .collect::<Vec<_>>();

    let all_ids = rows.iter().map(|row| row.0).collect::<Vec<_>>();
    let all_latest_folder = rows
        .iter()
        .filter_map(|row| row.3.as_ref())
        .max_by_key(|(_, exported_at)| *exported_at)
        .map(|(folder, _)| folder.clone());
    let all_exported = !rows.is_empty() && rows.iter().all(|row| row.3.is_some());

    let mut new_snapshot = false;
    let mut export_all = false;
    let mut open_all_folder = false;
    ui.horizontal(|ui| {
        ui.small(format!("{} saved state(s)", rows.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if all_exported {
                open_all_folder = ui
                    .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                    .on_hover_text("Open latest Snapshot export folder")
                    .clicked();
            }
            export_all = ui
                .add_enabled(
                    app.job.is_none() && !all_ids.is_empty() && !app.faces.is_empty(),
                    VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                )
                .on_hover_text("Export all Snapshots for the active Face")
                .clicked();
            new_snapshot = ui.small_button("+ New").clicked();
        });
    });

    if new_snapshot {
        app.flush_history_now();
        app.sync_history_to_active_snapshot();
        let id = app.project.create_snapshot();
        if let Some(snapshot) = app.project.snapshots.iter().find(|snapshot| snapshot.id == id) {
            app.snapshot_rename_id = Some(id);
            app.snapshot_rename_buffer = snapshot.name.clone();
        }
        app.load_history_for_active_snapshot("Snapshot created");
        app.history_clear_backup = None;
        app.mark_project_dirty();
        app.cache_current_snapshot_preview_if_ready();
    }
    if export_all {
        export_snapshot_group_dialog(app, all_ids.clone(), "all snapshots".to_owned());
    }
    if open_all_folder {
        if let Some(folder) = all_latest_folder.as_deref() {
            app.open_export_folder(folder);
        }
    }

    let active_id = app.project.active_snapshot_id;
    let active_dirty = active_id.is_some() && !app.project.active_snapshot_matches();
    let mut groups: Vec<(String, Vec<(u64, String, i64, Option<(String, i64)>)>)> = Vec::new();
    for row in rows {
        let day = snapshot_day_time(row.2).0;
        if groups.last().map(|group| group.0.as_str()) != Some(day.as_str()) {
            groups.push((day, Vec::new()));
        }
        groups.last_mut().unwrap().1.push(row);
    }

    let mut requested_load = None;
    let mut requested_export = None;
    let mut requested_group_export: Option<(Vec<u64>, String)> = None;
    let mut requested_folder: Option<String> = None;

    for (day, day_rows) in groups {
        let day_ids = day_rows.iter().map(|row| row.0).collect::<Vec<_>>();
        let day_exported = !day_rows.is_empty() && day_rows.iter().all(|row| row.3.is_some());
        let day_latest_folder = day_rows
            .iter()
            .filter_map(|row| row.3.as_ref())
            .max_by_key(|(_, exported_at)| *exported_at)
            .map(|(folder, _)| folder.clone());
        let day_label = format!("{day}  ·  {}", day_rows.len());
        egui::CollapsingHeader::new(day_label)
            .id_salt(("snapshot-day-group", day.clone()))
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.small("Group actions");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if day_exported
                            && ui
                                .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                                .on_hover_text("Open latest export folder for this group")
                                .clicked()
                        {
                            requested_folder = day_latest_folder.clone();
                        }
                        if ui
                            .add_enabled(
                                app.job.is_none() && !day_ids.is_empty() && !app.faces.is_empty(),
                                VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                            )
                            .on_hover_text("Export this Snapshot group")
                            .clicked()
                        {
                            requested_group_export = Some((day_ids.clone(), day.clone()));
                        }
                    });
                });

                for (id, name, created_at, export_record) in &day_rows {
                    let (_, time) = snapshot_day_time(*created_at);
                    let selected = active_id == Some(*id);
                    let display_name = if selected && active_dirty {
                        format!("{name}  *")
                    } else {
                        name.clone()
                    };
                    let (row_response, export_clicked, folder_clicked) = snapshot_row_with_actions(
                        ui,
                        selected,
                        selected && active_dirty,
                        &display_name,
                        &time,
                        export_record.is_some(),
                        app.job.is_none() && !app.faces.is_empty(),
                    );
                    if export_clicked {
                        requested_export = Some(*id);
                    } else if folder_clicked {
                        requested_folder = export_record.as_ref().map(|record| record.0.clone());
                    } else if row_response.clicked() {
                        requested_load = Some(*id);
                    }
                }
            });
        ui.add_space(2.0);
    }

    if let Some(id) = requested_load {
        app.request_snapshot_load(id);
    }
    if let Some(id) = requested_export {
        export_snapshot_dialog(app, id);
    }
    if let Some((ids, label)) = requested_group_export {
        export_snapshot_group_dialog(app, ids, label);
    }
    if let Some(folder) = requested_folder {
        app.open_export_folder(&folder);
    }

    let Some(active_id) = app.project.active_snapshot_id else {
        return;
    };
    let Some(active_name) = app
        .project
        .snapshots
        .iter()
        .find(|snapshot| snapshot.id == active_id)
        .map(|snapshot| snapshot.name.clone())
    else {
        return;
    };
    if app.snapshot_rename_id != Some(active_id) {
        app.snapshot_rename_id = Some(active_id);
        app.snapshot_rename_buffer = active_name.clone();
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.small("Name");
        let rename_response = ui.add(
            egui::TextEdit::singleline(&mut app.snapshot_rename_buffer)
                .desired_width(f32::INFINITY),
        );
        let enter = rename_response.has_focus()
            && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if (rename_response.lost_focus() || enter)
            && app.snapshot_rename_buffer.trim() != active_name
        {
            let candidate = app.snapshot_rename_buffer.clone();
            match app.project.rename_snapshot(active_id, &candidate) {
                Ok(true) => {
                    app.snapshot_rename_buffer = candidate.trim().to_owned();
                    app.mark_project_dirty();
                    app.report_info("Snapshot renamed");
                }
                Ok(false) => {}
                Err(err) => app.report_error(err),
            }
        }
    });

    let exported = app
        .project
        .snapshots
        .iter()
        .find(|snapshot| snapshot.id == active_id)
        .is_some_and(|snapshot| !snapshot.exports.is_empty());
    let mut update = false;
    let mut delete = false;
    ui.horizontal(|ui| {
        update = ui.button("Update").clicked();
        delete = ui.button("Delete").clicked();
        if exported {
            ui.colored_label(egui::Color32::LIGHT_GREEN, "Exported")
                .on_hover_text("This Snapshot has at least one successful coded test export. Updating it is protected.");
        }
    });
    if update {
        workflow::update_active_snapshot(app);
    }
    if delete && app.project.delete_snapshot(active_id) {
        app.snapshot_preview_cache.remove_snapshot(active_id);
        app.snapshot_rename_id = None;
        app.snapshot_rename_buffer.clear();
        app.history
            .reset(&app.project.adjustments, "Snapshot deleted");
        app.history_clear_backup = None;
        app.mark_project_dirty();
        app.report_info("Snapshot deleted");
    }
}

fn export_snapshot_dialog(app: &mut ShadeApp, snapshot_id: u64) {
    if app.job.is_some() {
        return;
    }
    if !workflow::active_face_available(app) {
        app.report_error(
            "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
        );
        return;
    }
    let Some(face) = app.faces.get(app.current_face) else {
        return;
    };
    let Some(snapshot) = app
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
    let today = Local::now().format("%Y-%m-%d").to_string();
    let test_code = app.project.effective_test_code_text();
    let context = export_batch::ExportNameContext {
        shade_name: None,
        project_name: &app.project.name,
        snapshot_name: &snapshot.name,
        test_code: &test_code,
        face_number: app.current_face + 1,
        face_name: &stem,
        source_name: &stem,
        date: &today,
    };
    let suggested =
        export_batch::render_export_filename(&app.settings.snapshot_export_template, &context);
    let Some(mut destination) = rfd::FileDialog::new()
        .add_filter("TIFF image", &["tif", "tiff"])
        .set_file_name(suggested)
        .save_file()
    else {
        return;
    };

    let mut conflict_policy = export_batch::ConflictPolicy::Overwrite;
    if destination.exists() {
        match collision_choice(&destination, 1) {
            CollisionChoice::Overwrite => {}
            CollisionChoice::Incremental => {
                destination = incremental_destination(app, &destination);
                conflict_policy = export_batch::ConflictPolicy::AutoNumber;
            }
            CollisionChoice::Cancel => return,
        }
    }

    queue_snapshot_export(app, &snapshot, destination, conflict_policy);
}

fn queue_snapshot_export(
    app: &mut ShadeApp,
    snapshot: &AdjustmentSnapshot,
    destination: PathBuf,
    conflict_policy: export_batch::ConflictPolicy,
) {
    let Some(face) = app.faces.get(app.current_face) else {
        return;
    };
    let source = face.path.clone();
    let face_key = source.to_string_lossy().into_owned();
    let folder = destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut project = app.project.clone();
    project.adjustments = snapshot.adjustments.clone();
    project.active_snapshot_id = Some(snapshot.id);
    app.export.remind_after_export = app.snapshot_project_needs_save_reminder();
    if !app.enqueue_export(export_queue::ExportQueueSpec {
        label: format!("Face {} / {}", app.current_face + 1, snapshot.name),
        source,
        destination,
        recipe: export_recipe::ExportRecipe::from_snapshot_project(&project),
        default_dpi: app.settings.default_dpi,
        force_lzw: app.settings.lzw_compression,
        validate_after_export: app.settings.validate_after_export,
        conflict_policy,
        mark: Some(export_queue::ExportQueueMark {
            snapshot_id: snapshot.id,
            face_key,
            folder,
        }),
    }) {
        return;
    }
    app.export.show_queue = true;
    app.report_info("Snapshot test export added to queue");
}

fn export_snapshot_group_dialog(app: &mut ShadeApp, snapshot_ids: Vec<u64>, label: String) {
    if app.job.is_some() || snapshot_ids.is_empty() {
        return;
    }
    if !workflow::active_face_available(app) {
        app.report_error(
            "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
        );
        return;
    }
    let Some(face) = app.faces.get(app.current_face) else {
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
    let face_name = app
        .project
        .faces
        .get(app.current_face)
        .map(|face| face.label.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| source_name.clone());
    let snapshots = snapshot_ids
        .into_iter()
        .filter_map(|id| {
            app.project
                .snapshots
                .iter()
                .find(|snapshot| snapshot.id == id)
                .cloned()
        })
        .collect::<Vec<_>>();
    if snapshots.is_empty() {
        return;
    }
    let shade_name = app
        .project_path
        .as_ref()
        .and_then(|path| path.file_stem())
        .map(|value| value.to_string_lossy().into_owned());
    let project_name = app.project.name.clone();
    let date = Local::now().format("%Y-%m-%d").to_string();

    let mut candidates = Vec::new();
    let mut collision_count = 0usize;
    for snapshot in &snapshots {
        let mut project = app.project.clone();
        project.adjustments = snapshot.adjustments.clone();
        project.active_snapshot_id = Some(snapshot.id);
        let test_code = project.effective_test_code_text();
        let context = export_batch::ExportNameContext {
            shade_name: shade_name.as_deref(),
            project_name: &project_name,
            snapshot_name: &snapshot.name,
            test_code: &test_code,
            face_number: app.current_face + 1,
            face_name: &face_name,
            source_name: &source_name,
            date: &date,
        };
        let folder = export_batch::render_export_folder(
            &base_folder,
            &app.settings.export_folder_template,
            &context,
        );
        let filename = export_batch::render_export_filename(
            &app.settings.snapshot_export_template,
            &context,
        );
        let destination = folder.join(filename);
        collision_count += usize::from(destination.exists());
        candidates.push((snapshot.clone(), folder, destination));
    }

    let conflict_policy = if collision_count > 0 {
        match collision_choice(&base_folder, collision_count) {
            CollisionChoice::Overwrite => export_batch::ConflictPolicy::Overwrite,
            CollisionChoice::Incremental => export_batch::ConflictPolicy::AutoNumber,
            CollisionChoice::Cancel => return,
        }
    } else {
        export_batch::ConflictPolicy::Overwrite
    };

    let mut reserved = app.export.queue.reserved_destination_keys();
    let mut queued = 0usize;
    for (snapshot, folder, destination) in candidates {
        if let Err(err) = std::fs::create_dir_all(&folder) {
            app.report_error(format!("Cannot create export folder {}: {err}", folder.display()));
            return;
        }
        let destination = if conflict_policy == export_batch::ConflictPolicy::AutoNumber {
            let file_name = destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("snapshot.tif");
            match export_batch::resolve_destination_reserved(
                &folder,
                file_name,
                export_batch::ConflictPolicy::AutoNumber,
                &mut reserved,
            ) {
                export_batch::DestinationDecision::Write(path) => path,
                export_batch::DestinationDecision::Skip(_) => continue,
            }
        } else {
            destination
        };
        let mut project = app.project.clone();
        project.adjustments = snapshot.adjustments.clone();
        project.active_snapshot_id = Some(snapshot.id);
        if !app.enqueue_export(export_queue::ExportQueueSpec {
            label: format!("{face_name} / {}", snapshot.name),
            source: source.clone(),
            destination,
            recipe: export_recipe::ExportRecipe::from_snapshot_project(&project),
            default_dpi: app.settings.default_dpi,
            force_lzw: app.settings.lzw_compression,
            validate_after_export: app.settings.validate_after_export,
            conflict_policy,
            mark: Some(export_queue::ExportQueueMark {
                snapshot_id: snapshot.id,
                face_key: face_key.clone(),
                folder,
            }),
        }) {
            return;
        }
        queued += 1;
    }

    if queued > 0 {
        app.export.remind_after_export = app.snapshot_project_needs_save_reminder();
        app.export.show_queue = true;
        app.report_info(format!("Queued {queued} Snapshot test export(s) · {label}"));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollisionChoice {
    Overwrite,
    Incremental,
    Cancel,
}

fn collision_choice(path: &Path, count: usize) -> CollisionChoice {
    let description = if count == 1 {
        format!(
            "A Snapshot export already exists:\n{}\n\nYes = Overwrite existing file\nNo = Create incremental version\nCancel = Do nothing",
            path.display()
        )
    } else {
        format!(
            "{count} Snapshot export file(s) already exist under:\n{}\n\nYes = Overwrite colliding files\nNo = Create incremental versions\nCancel = Do nothing",
            path.display()
        )
    };
    match rfd::MessageDialog::new()
        .set_title("Snapshot export collision")
        .set_description(description)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show()
    {
        rfd::MessageDialogResult::Yes => CollisionChoice::Overwrite,
        rfd::MessageDialogResult::No => CollisionChoice::Incremental,
        _ => CollisionChoice::Cancel,
    }
}

fn incremental_destination(app: &ShadeApp, destination: &Path) -> PathBuf {
    let folder = destination.parent().unwrap_or_else(|| Path::new("."));
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("snapshot.tif");
    let mut reserved = app.export.queue.reserved_destination_keys();
    match export_batch::resolve_destination_reserved(
        folder,
        filename,
        export_batch::ConflictPolicy::AutoNumber,
        &mut reserved,
    ) {
        export_batch::DestinationDecision::Write(path) => path,
        export_batch::DestinationDecision::Skip(_) => destination.to_path_buf(),
    }
}
