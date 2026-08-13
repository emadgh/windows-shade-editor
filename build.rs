use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn replace_once(text: String, old: &str, new: &str, label: &str) -> String {
    let count = text.matches(old).count();
    assert_eq!(count, 1, "{label}: expected one match, found {count}");
    text.replacen(old, new, 1)
}

fn replace_n(text: String, old: &str, new: &str, expected: usize, label: &str) -> String {
    let count = text.matches(old).count();
    assert_eq!(count, expected, "{label}: expected {expected} matches, found {count}");
    text.replace(old, new)
}

fn replace_section(mut text: String, start: &str, end: &str, replacement: &str, label: &str) -> String {
    let start_pos = text.find(start).unwrap_or_else(|| panic!("{label}: start marker not found"));
    let end_pos = text[start_pos..]
        .find(end)
        .map(|offset| start_pos + offset)
        .unwrap_or_else(|| panic!("{label}: end marker not found"));
    text.replace_range(start_pos..end_pos, replacement);
    text
}

fn path_attr(path: &Path, module: &str) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    format!("#[path = \"{value}\"]\nmod {module};")
}

fn write_file(path: &Path, text: &str) {
    fs::write(path, text).unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
}

fn main() {
    println!("cargo:rerun-if-changed=src/app_main.rs");
    println!("cargo:rerun-if-changed=src/settings_v6.rs");
    println!("cargo:rerun-if-changed=src/validation.rs");
    println!("cargo:rerun-if-changed=src/workflow_v0103.rs");

    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = root.join("src");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let generated_settings = out.join("settings_v0103.rs");
    let generated_validation = out.join("validation_v0103.rs");
    let generated_main = out.join("app_main_v0103.rs");

    let mut settings = fs::read_to_string(src.join("settings_v6.rs")).unwrap();
    settings = replace_once(
        settings,
        "    pub compact_curve_controls: bool,\n    pub default_dpi: f64,",
        "    pub compact_curve_controls: bool,\n    pub validate_after_export: bool,\n    pub default_dpi: f64,",
        "settings field",
    );
    settings = replace_once(
        settings,
        "            compact_curve_controls: false,\n            default_dpi: DEFAULT_DPI,",
        "            compact_curve_controls: false,\n            validate_after_export: false,\n            default_dpi: DEFAULT_DPI,",
        "settings default",
    );
    settings = replace_once(
        settings,
        "    #[test]\n    fn builtins_are_always_available()",
        "    #[test]\n    fn post_export_validation_defaults_off() {\n        assert!(!AppSettings::default().validate_after_export);\n    }\n\n    #[test]\n    fn builtins_are_always_available()",
        "settings test",
    );
    write_file(&generated_settings, &settings);

    let mut validation = fs::read_to_string(src.join("validation.rs")).unwrap();
    let transport = r#"pub fn validate_export_transport(source: &Path, exported: &Path) -> Result<String, String> {
    let source_info = tiff_io::stream_info(source)
        .map_err(|err| format!("Cannot inspect source TIFF for post-export validation: {err}"))?;
    let output_info = tiff_io::stream_info(exported)
        .map_err(|err| format!("Post-export TIFF validation failed while opening output: {err}"))?;
    let source_meta = &source_info.metadata;
    let output_meta = &output_info.metadata;
    let mut mismatches = Vec::new();

    if (source_meta.width, source_meta.height) != (output_meta.width, output_meta.height) {
        mismatches.push("dimensions changed".to_owned());
    }
    if source_meta.bit_depth != output_meta.bit_depth {
        mismatches.push("bit depth changed".to_owned());
    }
    if source_meta.color_model != output_meta.color_model {
        mismatches.push("color model changed".to_owned());
    }
    if source_meta.samples_per_pixel != output_meta.samples_per_pixel
        || source_meta.base_channel_count != output_meta.base_channel_count
    {
        mismatches.push("channel layout changed".to_owned());
    }
    if source_meta.channel_names != output_meta.channel_names {
        mismatches.push("channel names/order changed".to_owned());
    }
    if source_meta.icc_profile != output_meta.icc_profile {
        mismatches.push("ICC profile changed".to_owned());
    }
    if source_meta.photoshop_resources != output_meta.photoshop_resources {
        mismatches.push("Photoshop Image Resources 34377 changed".to_owned());
    }
    if source_meta.photoshop_image_source_data != output_meta.photoshop_image_source_data {
        mismatches.push("Photoshop ImageSourceData 37724 changed".to_owned());
    }
    if source_meta.orientation != output_meta.orientation {
        mismatches.push("orientation changed".to_owned());
    }
    let expected_compression = expected_export_compression(source_meta.compression);
    if output_meta.compression != expected_compression {
        mismatches.push(format!(
            "compression expected {:?}, got {:?}",
            expected_compression, output_meta.compression
        ));
    }
    if source_meta.predictor == Some(2)
        && source_meta.samples_per_pixel == source_meta.base_channel_count
    {
        if output_meta.predictor != Some(2) {
            mismatches.push(format!(
                "horizontal predictor expected Some(2), got {:?}",
                output_meta.predictor
            ));
        }
    } else if output_meta.predictor == Some(2)
        && output_meta.samples_per_pixel > output_meta.base_channel_count
    {
        mismatches.push("unsafe horizontal predictor remained enabled with ExtraSamples".to_owned());
    }
    if !mismatches.is_empty() {
        return Err(format!(
            "Post-export TIFF metadata validation failed: {}",
            mismatches.join("; ")
        ));
    }

    let mut next_row = 0u32;
    let mut decoded_samples = 0u64;
    tiff_io::for_each_decoded_strip(exported, &output_info, |row_start, row_count, samples| {
        if row_start != next_row {
            return Err(format!(
                "Post-export TIFF strip order is invalid: expected row {next_row}, got {row_start}."
            ));
        }
        let expected = u64::from(output_meta.width)
            .checked_mul(u64::from(row_count))
            .and_then(|value| value.checked_mul(output_meta.samples_per_pixel as u64))
            .ok_or_else(|| "Post-export TIFF sample count overflow.".to_owned())?;
        if samples.len() as u64 != expected {
            return Err(format!(
                "Post-export TIFF strip sample count mismatch: decoded {}, expected {expected}.",
                samples.len()
            ));
        }
        decoded_samples = decoded_samples.saturating_add(expected);
        next_row = next_row.saturating_add(row_count);
        Ok(())
    })?;
    let expected_samples = u64::from(output_meta.width)
        .checked_mul(u64::from(output_meta.height))
        .and_then(|value| value.checked_mul(output_meta.samples_per_pixel as u64))
        .ok_or_else(|| "Post-export TIFF sample count overflow.".to_owned())?;
    if next_row != output_meta.height || decoded_samples != expected_samples {
        return Err(format!(
            "Post-export TIFF decode incomplete: rows {next_row}/{}, samples {decoded_samples}/{expected_samples}.",
            output_meta.height
        ));
    }

    Ok(format!(
        "validation PASS · {} channel(s) · compression {:?} · predictor {:?}",
        output_meta.samples_per_pixel, output_meta.compression, output_meta.predictor
    ))
}

"#;
    validation = replace_once(
        validation,
        "fn push_check(",
        &format!("{transport}fn push_check("),
        "transport validator insertion",
    );
    validation = replace_once(
        validation,
        "        assert!(artifacts.markdown_path.is_file());\n\n        let _ = fs::remove_dir_all(folder);",
        "        assert!(artifacts.markdown_path.is_file());\n        let exported = PathBuf::from(&artifacts.report.exported_tiff);\n        let transport = validate_export_transport(&source, &exported)\n            .expect(\"post-export transport validation should pass\");\n        assert!(transport.contains(\"validation PASS\"));\n\n        let _ = fs::remove_dir_all(folder);",
        "transport validator regression",
    );
    write_file(&generated_validation, &validation);

    let mut app = fs::read_to_string(src.join("app_main.rs")).unwrap();
    app = app
        .strip_prefix("#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]\n\n")
        .expect("app_main crate attribute changed")
        .to_owned();

    let module_block = r#"mod app_log;
mod dpi;
#[path = "export_v6.rs"]
mod export;
mod history;
#[path = "model_v6.rs"]
mod model;
mod palette;
mod recovery;
mod render;
#[path = "settings_v6.rs"]
mod settings;
mod thumbnail;
#[path = "tiff_io.rs"]
mod tiff_io;
#[path = "update_v4.rs"]
mod update;
mod validation;"#;
    let replacement_modules = [
        path_attr(&src.join("app_log.rs"), "app_log"),
        path_attr(&src.join("dpi.rs"), "dpi"),
        path_attr(&src.join("export_v6.rs"), "export"),
        path_attr(&src.join("history.rs"), "history"),
        path_attr(&src.join("model_v6.rs"), "model"),
        path_attr(&src.join("palette.rs"), "palette"),
        path_attr(&src.join("recovery.rs"), "recovery"),
        path_attr(&src.join("render.rs"), "render"),
        path_attr(&generated_settings, "settings"),
        path_attr(&src.join("thumbnail.rs"), "thumbnail"),
        path_attr(&src.join("tiff_io.rs"), "tiff_io"),
        path_attr(&src.join("update_v4.rs"), "update"),
        path_attr(&generated_validation, "validation"),
        path_attr(&src.join("workflow_v0103.rs"), "workflow_v0103"),
    ]
    .join("\n");
    app = replace_once(app, module_block, &replacement_modules, "module block");

    app = replace_once(
        app,
        "struct RuntimeFace {\n    path: PathBuf,\n    preview: Arc<PreviewFace>,",
        "struct RuntimeFace {\n    path: PathBuf,\n    available: bool,\n    preview: Arc<PreviewFace>,",
        "runtime availability",
    );
    app = replace_once(
        app,
        "struct LoadedFace {\n    path: PathBuf,\n    preview: PreviewFace,",
        "struct LoadedFace {\n    path: PathBuf,\n    available: bool,\n    preview: PreviewFace,",
        "loaded availability",
    );
    app = replace_once(
        app,
        "    RebuildPreviews(Result<Vec<LoadedFace>, String>),\n    Open(Result<OpenPayload, String>),",
        "    RebuildPreviews(Result<Vec<LoadedFace>, String>),\n    RelinkFace { index: usize, result: Result<LoadedFace, String> },\n    RelinkFolder { faces: Vec<(usize, LoadedFace)>, errors: Vec<String> },\n    Open(Result<OpenPayload, String>),",
        "relink job variants",
    );
    app = replace_once(
        app,
        "        RuntimeFace {\n            path: item.path,\n            preview: Arc::new(item.preview),",
        "        RuntimeFace {\n            path: item.path,\n            available: item.available,\n            preview: Arc::new(item.preview),",
        "runtime face construction",
    );
    app = replace_once(
        app,
        "                    Ok(preview) => faces.push(LoadedFace {\n                        dpi: dpi::read_dpi(&path, default_dpi),\n                        path,\n                        preview,\n                    }),",
        "                    Ok(preview) => faces.push(LoadedFace {\n                        dpi: dpi::read_dpi(&path, default_dpi),\n                        path,\n                        available: true,\n                        preview,\n                    }),",
        "add face availability",
    );

    app = replace_section(
        app,
        "    fn rebuild_previews(&mut self) {",
        "    fn open_project_dialog(&mut self) {",
        "    fn rebuild_previews(&mut self) {\n        workflow_v0103::rebuild_previews(self);\n    }\n\n",
        "rebuild preview delegation",
    );

    let open_load = r#"                    match tiff_io::load_preview(&source, max_dimension) {
                        Ok(preview) => {
                            project.ensure_channels(&preview.metadata.channel_names);
                            faces.push(LoadedFace {
                                dpi: dpi::read_dpi(&source, default_dpi),
                                path: source,
                                preview,
                            });
                        }
                        Err(err) => errors.push(format!("{}: {err}", source.display())),
                    }"#;
    let placeholder_load = r#"                    let expected = project
                        .file_metadata
                        .as_ref()
                        .and_then(|metadata| metadata.faces.get(index))
                        .cloned();
                    match tiff_io::load_preview(&source, max_dimension) {
                        Ok(preview) => {
                            project.ensure_channels(&preview.metadata.channel_names);
                            faces.push(LoadedFace {
                                dpi: dpi::read_dpi(&source, default_dpi),
                                path: source,
                                available: true,
                                preview,
                            });
                        }
                        Err(err) => {
                            errors.push(format!("{}: {err}", source.display()));
                            faces.push(workflow_v0103::placeholder_loaded_face(
                                source,
                                expected.as_ref(),
                                default_dpi,
                            ));
                        }
                    }"#;
    app = replace_n(app, open_load, placeholder_load, 2, "open/recovery placeholders");

    app = replace_once(
        app,
        "        let thumbnail_face = self\n            .faces\n            .get(self.current_face)\n            .map(|face| Arc::clone(&face.preview));",
        "        let thumbnail_face = self\n            .faces\n            .get(self.current_face)\n            .filter(|face| face.available)\n            .or_else(|| self.faces.iter().find(|face| face.available))\n            .map(|face| Arc::clone(&face.preview));",
        "save thumbnail availability",
    );

    for (needle, message) in [
        (
            "    fn export_current_dialog(&mut self) {\n        if self.job.is_some() {\n            return;\n        }",
            "The active Face source TIFF is missing. Relink it before exporting.",
        ),
        (
            "    fn validate_current_face_dialog(&mut self) {\n        if self.job.is_some() {\n            return;\n        }",
            "The active Face source TIFF is missing. Relink it before validation.",
        ),
        (
            "    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {\n        if self.job.is_some() {\n            return;\n        }",
            "The active Face source TIFF is missing. Relink it before exporting Snapshots.",
        ),
    ] {
        let replacement = format!(
            "{needle}\n        if !workflow_v0103::active_face_available(self) {{\n            self.report_error(\"{message}\");\n            return;\n        }}"
        );
        app = replace_once(app, needle, &replacement, "active face operation guard");
    }
    app = replace_once(
        app,
        "    fn export_snapshot_group_dialog(&mut self, snapshot_ids: Vec<u64>, label: String) {\n        if self.job.is_some() || snapshot_ids.is_empty() {\n            return;\n        }",
        "    fn export_snapshot_group_dialog(&mut self, snapshot_ids: Vec<u64>, label: String) {\n        if self.job.is_some() || snapshot_ids.is_empty() {\n            return;\n        }\n        if !workflow_v0103::active_face_available(self) {\n            self.report_error(\"The active Face source TIFF is missing. Relink it before exporting Snapshots.\");\n            return;\n        }",
        "snapshot group guard",
    );
    app = replace_once(
        app,
        "    fn export_all_dialog(&mut self) {\n        if self.job.is_some() || self.faces.is_empty() {\n            return;\n        }",
        "    fn export_all_dialog(&mut self) {\n        if self.job.is_some() || self.faces.is_empty() {\n            return;\n        }\n        if self.faces.iter().any(|face| !face.available) {\n            self.report_error(\"Export all requires every Face source TIFF to be available. Relink missing Faces first.\");\n            return;\n        }",
        "export all guard",
    );

    app = replace_once(
        app,
        "        let project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        self.launch_job(\"Exporting TIFF\", move |progress| {",
        "        let project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        let validate_after_export = self.settings.validate_after_export;\n        self.launch_job(\"Exporting TIFF\", move |progress| {",
        "current export validation setting",
    );
    let current_export = r#"            let result = export::export_face_with_progress(
                &source,
                &destination,
                &project,
                default_dpi,
                |fraction, detail| {
                    Self::set_progress(&progress, Some(fraction), "Exporting TIFF", detail);
                },
            )
            .map(|_| format!("Exported {}", destination.display()));"#;
    let current_verified = r#"            let result = export::export_face_with_progress(
                &source,
                &destination,
                &project,
                default_dpi,
                |fraction, detail| {
                    let fraction = if validate_after_export { fraction * 0.88 } else { fraction };
                    Self::set_progress(&progress, Some(fraction), "Exporting TIFF", detail);
                },
            )
            .and_then(|_| {
                if validate_after_export {
                    Self::set_progress(
                        &progress,
                        Some(0.92),
                        "Validating exported TIFF",
                        "Decoding strips and checking production metadata",
                    );
                    let verified = validation::validate_export_transport(&source, &destination)?;
                    Ok(format!("Exported {} · {verified}", destination.display()))
                } else {
                    Ok(format!("Exported {}", destination.display()))
                }
            });"#;
    app = replace_once(app, current_export, current_verified, "current verified export");

    app = replace_once(
        app,
        "        let project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        self.launch_job(\"Exporting faces\", move |progress| {",
        "        let project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        let validate_after_export = self.settings.validate_after_export;\n        self.launch_job(\"Exporting faces\", move |progress| {",
        "all export validation setting",
    );
    let all_export = r#"                    export::export_face_with_progress(
                        source,
                        &destination,
                        &project,
                        default_dpi,
                        |inner, detail| {
                            let overall = (index as f32 + inner) / total as f32;
                            Self::set_progress(&progress, Some(overall), "Exporting faces", detail);
                        },
                    )?;
                }
                Ok(format!("Exported {total} face(s) to {}", folder.display()))"#;
    let all_verified = r#"                    export::export_face_with_progress(
                        source,
                        &destination,
                        &project,
                        default_dpi,
                        |inner, detail| {
                            let phase = if validate_after_export { inner * 0.88 } else { inner };
                            let overall = (index as f32 + phase) / total as f32;
                            Self::set_progress(&progress, Some(overall), "Exporting faces", detail);
                        },
                    )?;
                    if validate_after_export {
                        let overall = (index as f32 + 0.92) / total as f32;
                        Self::set_progress(
                            &progress,
                            Some(overall),
                            "Validating exported TIFF",
                            &destination.display().to_string(),
                        );
                        validation::validate_export_transport(source, &destination)?;
                    }
                }
                if validate_after_export {
                    Ok(format!("Exported and verified {total} face(s) to {}", folder.display()))
                } else {
                    Ok(format!("Exported {total} face(s) to {}", folder.display()))
                }"#;
    app = replace_once(app, all_export, all_verified, "all verified export");

    let relink_poll = r#"            JobResult::RelinkFace { index, result } => {
                workflow_v0103::apply_relinked_face(self, index, result);
            }
            JobResult::RelinkFolder { faces, errors } => {
                workflow_v0103::apply_relinked_folder(self, faces, errors);
            }
"#;
    app = replace_once(
        app,
        "            JobResult::Open(result) => match result {",
        &format!("{relink_poll}            JobResult::Open(result) => match result {{"),
        "poll relink results",
    );

    app = replace_once(
        app,
        "        let Some(face) = self.faces.get(self.current_face) else {\n            return;\n        };\n        if face.rendered_generation == face.generation {",
        "        let Some(face) = self.faces.get(self.current_face) else {\n            return;\n        };\n        if !face.available {\n            return;\n        }\n        if face.rendered_generation == face.generation {",
        "skip placeholder rendering",
    );

    app = replace_section(
        app,
        "    fn ui_faces(&mut self, ui: &mut egui::Ui) {",
        "    fn ui_snapshots(&mut self, ui: &mut egui::Ui) {",
        "    fn ui_faces(&mut self, ui: &mut egui::Ui) {\n        workflow_v0103::ui_faces(self, ui);\n    }\n",
        "faces UI delegation",
    );

    app = replace_once(
        app,
        "            .get(self.current_face)\n            .map(|face| face.preview.metadata.channel_names.clone())\n            .unwrap_or_default();",
        "            .get(self.current_face)\n            .filter(|face| face.available)\n            .map(|face| face.preview.metadata.channel_names.clone())\n            .unwrap_or_default();",
        "test code missing face channels",
    );
    app = replace_once(
        app,
        "        let Some(face) = self.faces.get(self.current_face) else {\n            ui.heading(\"Channels\");\n            ui.label(\"No active face\");\n            return;\n        };\n        let channel_names = face.preview.metadata.channel_names.clone();",
        "        let Some(face) = self.faces.get(self.current_face) else {\n            ui.heading(\"Channels\");\n            ui.label(\"No active face\");\n            return;\n        };\n        if !face.available {\n            ui.heading(\"Channels\");\n            ui.label(\"Source TIFF missing. Relink this Face to inspect channels and histograms.\");\n            return;\n        }\n        let channel_names = face.preview.metadata.channel_names.clone();",
        "channels missing state",
    );
    app = replace_once(
        app,
        "        let Some(face) = self.faces.get(self.current_face) else {\n            ui.heading(\"Adjustments\");\n            ui.label(\"No active face\");\n            return;\n        };\n        let channel_names = face.preview.metadata.channel_names.clone();",
        "        let Some(face) = self.faces.get(self.current_face) else {\n            ui.heading(\"Adjustments\");\n            ui.label(\"No active face\");\n            return;\n        };\n        if !face.available {\n            ui.heading(\"Adjustments\");\n            ui.label(\"Source TIFF missing. Relink this Face before editing its channels.\");\n            return;\n        }\n        let channel_names = face.preview.metadata.channel_names.clone();",
        "adjustments missing state",
    );

    app = replace_once(
        app,
        "        let file_name = face\n            .path\n            .file_name()",
        "        if workflow_v0103::ui_missing_viewport(self, ui) {\n            return;\n        }\n\n        let file_name = face\n            .path\n            .file_name()",
        "viewport missing state",
    );

    let preview_help = "                ui.small(\"The max dimension is used when TIFF previews are loaded. Use Rebuild previews to apply a changed value to Faces already open in this project.\");\n";
    let validation_setting = format!(
        "{preview_help}                changed |= ui\n                    .checkbox(\n                        &mut self.settings.validate_after_export,\n                        \"Validate TIFF after normal Export face / Export all\",\n                    )\n                    .changed();\n                ui.small(\"When enabled, Shade Editor immediately re-decodes every exported TIFF and verifies channel layout/names, ICC/Photoshop resources, compression/predictor policy and complete strip decoding.\");\n"
    );
    app = replace_once(app, preview_help, &validation_setting, "validation settings UI");

    app = replace_once(
        app,
        "        self.poll_autosave();\n        self.handle_history_shortcuts(ui.ctx());",
        "        self.poll_autosave();\n        workflow_v0103::handle_shortcuts(self, ui.ctx());\n        self.handle_history_shortcuts(ui.ctx());",
        "workflow shortcut dispatch",
    );
    app = replace_once(
        app,
        "        if update && self.project.update_snapshot(active_id) {\n            self.project_dirty = true;\n            self.report_info(\"Snapshot updated\");\n        }",
        "        if update {\n            workflow_v0103::update_active_snapshot(self);\n        }",
        "snapshot update shortcut reuse",
    );
    app = replace_once(
        app,
        "                ui.label(\"Update controls are located on the right side of the main toolbar.\");",
        "                ui.label(\"Update controls are located on the right side of the main toolbar.\");\n                ui.separator();\n                ui.label(\"Shortcuts: Ctrl+S Save · Ctrl+Shift+S Save As · F Fit · 1-9 channel · S Solo · Ctrl+Enter Update Snapshot · Curve arrows nudge; Shift+Arrow uses larger steps.\");",
        "about shortcut help",
    );

    let curve_start = app.find("fn curve_editor_graph(").expect("curve editor start");
    let curve_end = app[curve_start..]
        .find("fn curve_point_fields(")
        .map(|offset| curve_start + offset)
        .expect("curve editor end");
    let mut curve = app[curve_start..curve_end].to_owned();
    curve = replace_once(
        curve,
        "    let mut changed = false;\n    let mut midpoint_removed_this_frame = false;",
        "    let mut changed = false;\n    if graph_response.clicked() {\n        graph_response.request_focus();\n    }\n    let mut midpoint_removed_this_frame = false;",
        "curve focus",
    );
    curve = replace_once(
        curve,
        "        if response.clicked() || response.drag_started() {\n            selected = point;\n            ui.data_mut(|data| data.insert_temp(selection_id, point));\n        }",
        "        if response.clicked() || response.drag_started() {\n            selected = point;\n            ui.data_mut(|data| data.insert_temp(selection_id, point));\n            graph_response.request_focus();\n        }",
        "curve point focus",
    );
    let nudge = r#"    if graph_response.has_focus() {
        let (left, right, up, down, shift) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.modifiers.shift,
            )
        });
        if left || right || up || down {
            let step = if shift { 10.0 / 255.0 } else { 1.0 / 255.0 };
            let (mut input_value, mut output_value) = curve_point_xy(*curve, selected);
            if left { input_value -= step; }
            if right { input_value += step; }
            if up { output_value += step; }
            if down { output_value -= step; }
            set_curve_point(curve, selected, input_value, output_value);
            changed = true;
        }
    }

"#;
    curve = replace_once(
        curve,
        "    let painter = ui.painter_at(rect);\n",
        &format!("{nudge}    let painter = ui.painter_at(rect);\n"),
        "curve nudge",
    );
    app.replace_range(curve_start..curve_end, &curve);

    app.insert_str(0, "// @generated by build.rs from src/app_main.rs for Shade Editor 0.10.3.\n");
    write_file(&generated_main, &app);
}
