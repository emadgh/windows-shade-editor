use super::*;

pub(super) fn active_face_available(app: &ShadeApp) -> bool {
    app.faces
        .get(app.current_face)
        .is_some_and(|face| face.available)
}

fn next_test_code(current: &str) -> String {
    let current = current.trim();
    if current.is_empty() {
        return "Test-2".to_owned();
    }
    let split = current
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    let (prefix, digits) = current.split_at(split);
    if !digits.is_empty() {
        if let Ok(number) = digits.parse::<u64>() {
            return format!("{prefix}{}", number.saturating_add(1));
        }
    }
    format!("{current}-2")
}

pub(super) fn update_active_snapshot(app: &mut ShadeApp) {
    let Some(active_id) = app.project.active_snapshot_id else {
        return;
    };
    let exported = app
        .project
        .snapshots
        .iter()
        .find(|snapshot| snapshot.id == active_id)
        .is_some_and(|snapshot| !snapshot.exports.is_empty());
    let dirty = !app.project.active_snapshot_matches();
    let mut reusing_exported_code = false;

    if exported && dirty {
        let snapshot_name = app
            .project
            .active_snapshot_name()
            .unwrap_or("Snapshot")
            .to_owned();
        let current_code = app.project.effective_test_code_text();
        let description = format!(
            "'{snapshot_name}' has already been exported with Test Code '{current_code}'.\n\nYes = Create a NEW Snapshot + Test Code (recommended)\nNo = Reuse the SAME exported Test Code and update this Snapshot\nCancel = Keep the exported Snapshot unchanged\n\nReusing the same code does not bypass the separate file overwrite confirmation during the next export."
        );
        match rfd::MessageDialog::new()
            .set_title("Exported Snapshot / Test Code")
            .set_description(description)
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::YesNoCancel)
            .show()
        {
            rfd::MessageDialogResult::Yes => {
                app.flush_history_now();
                let new_code = next_test_code(&current_code);
                app.project.test_code.enabled = true;
                app.project.test_code.text = new_code.clone();
                let new_id = app.project.create_snapshot();
                app.sync_history_to_active_snapshot();
                if let Some(snapshot) = app
                    .project
                    .snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == new_id)
                {
                    app.snapshot_rename_id = Some(new_id);
                    app.snapshot_rename_buffer = snapshot.name.clone();
                }
                app.history_clear_backup = None;
                app.snapshot_preview_cache.remove_snapshot(new_id);
                app.cache_current_snapshot_preview_if_ready();
                app.mark_project_dirty();
                app.report_info(format!(
                    "Preserved exported Snapshot '{snapshot_name}' and created a new test state with code '{new_code}'"
                ));
                return;
            }
            rfd::MessageDialogResult::No => {
                reusing_exported_code = true;
            }
            _ => return,
        }
    }

    app.flush_history_now();
    app.sync_history_to_active_snapshot();
    if app.project.update_snapshot(active_id) {
        app.snapshot_preview_cache.remove_snapshot(active_id);
        app.cache_current_snapshot_preview_if_ready();
        app.mark_project_dirty();
        if reusing_exported_code {
            app.report_info("Snapshot updated with the previously exported Test Code; disk overwrite protection still applies on export");
        } else {
            app.report_info("Snapshot updated · preview cache refreshed");
        }
    }
}

pub(super) fn handle_shortcuts(app: &mut ShadeApp, ctx: &egui::Context) {
    let curve_graph_focused = ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new("shade-editor-curve-graph-focused"))
            .unwrap_or(false)
    });
    let modal_active = app.lifecycle.pending.is_some()
        || app.lifecycle.after_save.is_some()
        || app.lifecycle.backup_restore.is_some()
        || app.pending_snapshot_action.is_some()
        || app.recovery_candidate.is_some();
    let input_context = input_router::classify(
        ctx.wants_keyboard_input(),
        curve_graph_focused,
        modal_active,
        app.project_view.open,
    );

    let (new_project, save, save_as, export_face, export_all, update_snapshot) =
        ctx.input(|input| {
            (
                input.key_pressed(egui::Key::N)
                    && input.modifiers.ctrl
                    && !input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::S)
                    && input.modifiers.ctrl
                    && !input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::S)
                    && input.modifiers.ctrl
                    && input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::E)
                    && input.modifiers.ctrl
                    && !input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::E)
                    && input.modifiers.ctrl
                    && input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::Enter) && input.modifiers.ctrl && !input.modifiers.alt,
            )
        });

    if input_context.allows_save_shortcuts() {
        if save_as {
            app.save_project(true);
        } else if save {
            app.save_project(false);
        }
    }
    if input_context.allows_project_commands() {
        if new_project {
            app.project_view.open = false;
            app.new_project();
        }
        if export_all {
            app.export_all_dialog();
        } else if export_face {
            app.export_current_dialog();
        }
        if update_snapshot {
            update_active_snapshot(app);
        }
    }

    if !input_context.allows_editor_shortcuts() {
        return;
    }

    let (channel, all_channels, settings, fit, solo) = ctx.input(|input| {
        let no_modifiers = !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.shift;
        let keys = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        let channel = no_modifiers
            .then(|| keys.iter().position(|key| input.key_pressed(*key)))
            .flatten();
        // Backtick is the logical key for both ` and Shift+` (~) in egui.
        let all_channels =
            !input.modifiers.ctrl && !input.modifiers.alt && input.key_pressed(egui::Key::Backtick);
        (
            channel,
            all_channels,
            no_modifiers && input.key_pressed(egui::Key::G),
            no_modifiers && input.key_pressed(egui::Key::F),
            no_modifiers && input.key_pressed(egui::Key::S),
        )
    });

    if settings {
        app.show_settings = true;
    }
    if all_channels {
        select_all_channels_shortcut(app);
    }
    if fit {
        app.fit_requested = true;
        app.viewport_recenter = true;
    }
    if let Some(channel) = channel {
        select_channel_shortcut(app, channel);
    }
    if solo && active_face_available(app) {
        let previous = app.solo_channel;
        app.solo_channel = if app.solo_channel == Some(app.selected_channel) {
            None
        } else {
            Some(app.selected_channel)
        };
        if app.solo_channel != previous {
            app.mark_current_preview_dirty();
        }
    }
}

fn select_all_channels_shortcut(app: &mut ShadeApp) {
    let previous_solo = app.solo_channel;
    app.adjustment_scope = if app.adjustment_scope == AdjustmentScope::All {
        AdjustmentScope::Selected
    } else {
        AdjustmentScope::All
    };
    app.solo_channel = None;
    if previous_solo.is_some() {
        app.mark_current_preview_dirty();
    }
}

fn select_channel_shortcut(app: &mut ShadeApp, channel: usize) {
    if app
        .faces
        .get(app.current_face)
        .filter(|face| face.available)
        .is_some_and(|face| channel < face.preview.metadata.channel_names.len())
    {
        app.select_channel(channel, false);
    }
}

pub(super) fn rebuild_previews(app: &mut ShadeApp) {
    if app.job.is_some() || app.faces.is_empty() {
        return;
    }
    let items = app
        .faces
        .iter()
        .map(|face| {
            (
                face.path.clone(),
                face.available,
                (*face.preview).clone(),
                face.dpi,
            )
        })
        .collect::<Vec<_>>();
    let max_dimension = app.settings.max_preview_dimension;
    let default_dpi = app.settings.default_dpi;
    app.launch_job("Rebuilding previews", move |progress| {
        let result = (|| -> Result<Vec<LoadedFace>, String> {
            let total = items.len().max(1);
            let mut faces = Vec::with_capacity(items.len());
            for (index, (path, available, old_preview, old_dpi)) in items.into_iter().enumerate() {
                ShadeApp::set_progress(
                    &progress,
                    Some(index as f32 / total as f32),
                    "Rebuilding previews",
                    &path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                if !available || !path.is_file() {
                    faces.push(LoadedFace {
                        path,
                        available: false,
                        preview: old_preview,
                        dpi: old_dpi,
                    });
                    continue;
                }
                let preview = tiff_io::load_preview(&path, max_dimension)
                    .map_err(|err| format!("{}: {err}", path.display()))?;
                faces.push(LoadedFace {
                    dpi: dpi::read_dpi(&path, default_dpi),
                    path,
                    available: true,
                    preview,
                });
            }
            ShadeApp::set_progress(&progress, Some(1.0), "Rebuilding previews", "Complete");
            Ok(faces)
        })();
        JobResult::RebuildPreviews(result)
    });
}

pub(super) fn relink_current_face_dialog(app: &mut ShadeApp) {
    if app.job.is_some() {
        return;
    }
    let index = app.current_face;
    let Some(face) = app.faces.get(index) else {
        return;
    };
    if face.available {
        return;
    }
    let current_path = face.path.clone();
    let expected = app
        .project
        .file_metadata
        .as_ref()
        .and_then(|metadata| metadata.faces.get(index))
        .cloned();
    let mut dialog = rfd::FileDialog::new().add_filter("TIFF images", &["tif", "tiff"]);
    if let Some(parent) = current_path.parent() {
        dialog = dialog.set_directory(parent);
    }
    if let Some(name) = current_path.file_name().and_then(|name| name.to_str()) {
        dialog = dialog.set_file_name(name);
    }
    let Some(path) = dialog.pick_file() else {
        return;
    };
    let max_dimension = app.settings.max_preview_dimension;
    let default_dpi = app.settings.default_dpi;
    app.launch_job("Relinking Face", move |progress| {
        ShadeApp::set_progress(&progress, Some(0.15), "Relinking Face", "Verifying TIFF");
        let result = load_relink_candidate(path, max_dimension, default_dpi, expected.as_ref());
        ShadeApp::set_progress(&progress, Some(1.0), "Relinking Face", "Complete");
        JobResult::RelinkFace { index, result }
    });
}

pub(super) fn relink_missing_faces_folder_dialog(app: &mut ShadeApp) {
    if app.job.is_some() {
        return;
    }
    let requests = app
        .faces
        .iter()
        .enumerate()
        .filter(|(_, face)| !face.available)
        .filter_map(|(index, face)| {
            let file_name = face.path.file_name()?.to_string_lossy().into_owned();
            let expected = app
                .project
                .file_metadata
                .as_ref()
                .and_then(|metadata| metadata.faces.get(index))
                .cloned();
            Some((index, file_name, expected))
        })
        .collect::<Vec<_>>();
    if requests.is_empty() {
        return;
    }
    let Some(folder) = rfd::FileDialog::new().pick_folder() else {
        return;
    };
    let max_dimension = app.settings.max_preview_dimension;
    let default_dpi = app.settings.default_dpi;
    app.launch_job("Relinking missing Faces", move |progress| {
        let total = requests.len().max(1);
        let mut faces = Vec::new();
        let mut errors = Vec::new();
        for (position, (index, file_name, expected)) in requests.into_iter().enumerate() {
            ShadeApp::set_progress(
                &progress,
                Some(position as f32 / total as f32),
                "Relinking missing Faces",
                &file_name,
            );
            let Some(path) = find_named_file_recursive(&folder, &file_name) else {
                errors.push(format!("{file_name}: not found under {}", folder.display()));
                continue;
            };
            match load_relink_candidate(path, max_dimension, default_dpi, expected.as_ref()) {
                Ok(face) => faces.push((index, face)),
                Err(err) => errors.push(format!("{file_name}: {err}")),
            }
        }
        ShadeApp::set_progress(&progress, Some(1.0), "Relinking missing Faces", "Complete");
        JobResult::RelinkFolder { faces, errors }
    });
}

pub(super) fn apply_relinked_face(
    app: &mut ShadeApp,
    index: usize,
    result: Result<LoadedFace, String>,
) {
    match result {
        Ok(item) => {
            if index < app.faces.len() && index < app.project.faces.len() {
                app.project
                    .ensure_channels(&item.preview.metadata.channel_names);
                app.snapshot_preview_cache.clear();
                app.project.faces[index].path = item.path.to_string_lossy().into_owned();
                app.faces[index] = ShadeApp::make_runtime_face(item);
                app.current_face = index;
                app.selected_channel = 0;
                app.solo_channel = None;
                app.fit_requested = true;
                app.viewport_recenter = true;
                app.mark_project_dirty();
                app.report_info("Face relinked and verified");
            }
        }
        Err(err) => app.report_error(format!("Relink failed: {err}")),
    }
}

pub(super) fn apply_relinked_folder(
    app: &mut ShadeApp,
    faces: Vec<(usize, LoadedFace)>,
    errors: Vec<String>,
) {
    let mut relinked = 0usize;
    for (index, item) in faces {
        if index < app.faces.len() && index < app.project.faces.len() {
            app.project
                .ensure_channels(&item.preview.metadata.channel_names);
            app.project.faces[index].path = item.path.to_string_lossy().into_owned();
            app.faces[index] = ShadeApp::make_runtime_face(item);
            relinked += 1;
        }
    }
    if relinked > 0 {
        app.snapshot_preview_cache.clear();
        app.mark_project_dirty();
        app.fit_requested = true;
        app.viewport_recenter = true;
        app.report_info(format!("Relinked and verified {relinked} missing Face(s)"));
    }
    if !errors.is_empty() {
        app.report_error(format!(
            "Some Faces were not relinked: {}",
            errors.join(" | ")
        ));
    }
}

pub(super) fn ui_missing_viewport(app: &mut ShadeApp, ui: &mut egui::Ui) -> bool {
    let Some(face) = app.faces.get(app.current_face) else {
        return false;
    };
    if face.available {
        return false;
    }
    let missing_path = face.path.clone();
    let mut locate_file = false;
    let mut locate_folder = false;
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.heading("Face source TIFF is missing");
            ui.label(missing_path.display().to_string());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                locate_file = ui.button("Locate file").clicked();
                locate_folder = ui.button("Locate folder").clicked();
            });
        });
    });
    if locate_file {
        relink_current_face_dialog(app);
    } else if locate_folder {
        relink_missing_faces_folder_dialog(app);
    }
    true
}

pub(super) fn placeholder_loaded_face(
    path: PathBuf,
    expected: Option<&model::FaceFileMetadata>,
    default_dpi: f64,
) -> LoadedFace {
    LoadedFace {
        path,
        available: false,
        preview: missing_face_preview(expected),
        dpi: missing_face_dpi(expected, default_dpi),
    }
}

fn color_model_from_cached(value: &str) -> tiff_io::ColorModel {
    match value.trim().to_ascii_uppercase().as_str() {
        "RGB" => tiff_io::ColorModel::Rgb,
        "CMYK" => tiff_io::ColorModel::Cmyk,
        "GRAY" | "GREY" => tiff_io::ColorModel::Gray,
        _ => tiff_io::ColorModel::Other,
    }
}

fn missing_face_preview(expected: Option<&model::FaceFileMetadata>) -> PreviewFace {
    let channel_names = expected
        .map(|metadata| metadata.channel_names.clone())
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| vec!["Missing source".to_owned()]);
    let samples_per_pixel = expected
        .map(|metadata| metadata.channel_count)
        .unwrap_or(channel_names.len())
        .max(channel_names.len())
        .max(1);
    let mut names = channel_names;
    while names.len() < samples_per_pixel {
        names.push(format!("Channel {}", names.len() + 1));
    }
    names.truncate(samples_per_pixel);
    let base_channel_count = expected
        .map(|metadata| metadata.base_channel_count)
        .unwrap_or(1)
        .clamp(1, samples_per_pixel);
    let metadata = tiff_io::TiffMetadata {
        width: expected.map(|m| m.width).unwrap_or(1).max(1),
        height: expected.map(|m| m.height).unwrap_or(1).max(1),
        bit_depth: expected.map(|m| m.bit_depth).unwrap_or(8),
        samples_per_pixel,
        base_channel_count,
        color_model: expected
            .map(|m| color_model_from_cached(&m.color_model))
            .unwrap_or(tiff_io::ColorModel::Other),
        non_cmyk_separated: false,
        channel_names: names,
        channel_display_info: vec![None; samples_per_pixel],
        compression: None,
        predictor: None,
        orientation: None,
        icc_profile: None,
        photoshop_resources: None,
        photoshop_image_source_data: None,
    };
    PreviewFace {
        metadata,
        width: 1,
        height: 1,
        channels: (0..samples_per_pixel).map(|_| vec![0u16]).collect(),
        histograms: vec![[0u32; 256]; samples_per_pixel],
    }
}

fn missing_face_dpi(expected: Option<&model::FaceFileMetadata>, default_dpi: f64) -> dpi::DpiInfo {
    let Some(expected) = expected else {
        return dpi::DpiInfo::with_default(default_dpi);
    };
    if !expected.dpi_from_source || expected.dpi_x <= 0.0 || expected.dpi_y <= 0.0 {
        return dpi::DpiInfo::with_default(default_dpi);
    }
    let unit = if matches!(expected.resolution_unit, 2 | 3) {
        expected.resolution_unit
    } else {
        2
    };
    dpi::DpiInfo {
        dpi_x: expected.dpi_x,
        dpi_y: expected.dpi_y,
        raw_x: Some(if unit == 3 {
            expected.dpi_x / 2.54
        } else {
            expected.dpi_x
        }),
        raw_y: Some(if unit == 3 {
            expected.dpi_y / 2.54
        } else {
            expected.dpi_y
        }),
        unit,
        has_physical_resolution: true,
        used_default: false,
    }
}

fn verify_relink_metadata(
    metadata: &tiff_io::TiffMetadata,
    expected: Option<&model::FaceFileMetadata>,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let mut bad = Vec::new();
    if (metadata.width, metadata.height) != (expected.width, expected.height) {
        bad.push(format!(
            "dimensions expected {}x{}, got {}x{}",
            expected.width, expected.height, metadata.width, metadata.height
        ));
    }
    if metadata.bit_depth != expected.bit_depth {
        bad.push(format!(
            "bit depth expected {}, got {}",
            expected.bit_depth, metadata.bit_depth
        ));
    }
    if metadata.color_model.title() != expected.color_model {
        bad.push(format!(
            "color model expected {}, got {}",
            expected.color_model,
            metadata.color_model.title()
        ));
    }
    if metadata.samples_per_pixel != expected.channel_count
        || metadata.base_channel_count != expected.base_channel_count
    {
        bad.push(format!(
            "channel layout expected {}/{} base, got {}/{} base",
            expected.channel_count,
            expected.base_channel_count,
            metadata.samples_per_pixel,
            metadata.base_channel_count
        ));
    }
    if metadata.channel_names != expected.channel_names {
        bad.push(format!(
            "channel names/order expected {:?}, got {:?}",
            expected.channel_names, metadata.channel_names
        ));
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Replacement TIFF does not match cached Face metadata: {}",
            bad.join("; ")
        ))
    }
}

fn load_relink_candidate(
    path: PathBuf,
    max_dimension: u32,
    default_dpi: f64,
    expected: Option<&model::FaceFileMetadata>,
) -> Result<LoadedFace, String> {
    let preview = tiff_io::load_preview(&path, max_dimension)
        .map_err(|err| format!("Cannot load replacement {}: {err}", path.display()))?;
    verify_relink_metadata(&preview.metadata, expected)?;
    Ok(LoadedFace {
        dpi: dpi::read_dpi(&path, default_dpi),
        path,
        available: true,
        preview,
    })
}

fn find_named_file_recursive(root: &Path, file_name: &str) -> Option<PathBuf> {
    let target = file_name.to_ascii_lowercase();
    let mut stack = vec![root.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path
                .file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase() == target)
                .unwrap_or(false)
            {
                return Some(path);
            }
        }
    }
    None
}
