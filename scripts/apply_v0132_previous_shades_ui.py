from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_count(text: str, old: str, new: str, expected: int, label: str) -> str:
    count = text.count(old)
    if count != expected:
        raise RuntimeError(f"{label}: expected {expected} matches, found {count}")
    return text.replace(old, new)


def replace_between(text: str, start: str, end: str, replacement: str, label: str) -> str:
    start_index = text.find(start)
    if start_index < 0:
        raise RuntimeError(f"{label}: start marker not found")
    end_index = text.find(end, start_index)
    if end_index < 0:
        raise RuntimeError(f"{label}: end marker not found")
    return text[:start_index] + replacement + text[end_index:]


root = Path('.')

# ---------------- thumbnail.rs ----------------
thumbnail_path = root / 'src' / 'thumbnail.rs'
thumbnail = thumbnail_path.read_text(encoding='utf-8')
thumbnail = replace_once(
    thumbnail,
    'const THUMBNAIL_MAX_DIMENSION: usize = 256;',
    'const THUMBNAIL_MAX_DIMENSION: usize = 512;',
    'thumbnail dimension',
)
thumbnail = replace_between(
    thumbnail,
    'fn resize_rgba(',
    'fn encode_png(',
    '''pub(crate) fn resize_rgba(
    width: usize,
    height: usize,
    rgba: &[u8],
    max_dimension: usize,
) -> Result<(usize, usize, Vec<u8>), String> {
    if width == 0 || height == 0 {
        return Err("Cannot create thumbnail for an empty preview.".to_owned());
    }
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Thumbnail dimensions overflow.".to_owned())?;
    if rgba.len() < expected {
        return Err("Preview RGBA data is incomplete.".to_owned());
    }

    let scale = (max_dimension as f64 / width.max(height) as f64).min(1.0);
    let out_width = ((width as f64 * scale).round() as usize).max(1);
    let out_height = ((height as f64 * scale).round() as usize).max(1);
    if out_width == width && out_height == height {
        return Ok((width, height, rgba[..expected].to_vec()));
    }

    // Bilinear filtering removes the blocky nearest-neighbour look that was
    // especially visible in Explorer and Previous Shades thumbnails.
    let mut output = vec![0u8; out_width * out_height * 4];
    let x_scale = width as f64 / out_width as f64;
    let y_scale = height as f64 / out_height as f64;
    for y in 0..out_height {
        let source_y = ((y as f64 + 0.5) * y_scale - 0.5).clamp(0.0, (height - 1) as f64);
        let y0 = source_y.floor() as usize;
        let y1 = (y0 + 1).min(height - 1);
        let fy = source_y - y0 as f64;
        for x in 0..out_width {
            let source_x = ((x as f64 + 0.5) * x_scale - 0.5).clamp(0.0, (width - 1) as f64);
            let x0 = source_x.floor() as usize;
            let x1 = (x0 + 1).min(width - 1);
            let fx = source_x - x0 as f64;
            let target = (y * out_width + x) * 4;
            for channel in 0..4 {
                let p00 = rgba[(y0 * width + x0) * 4 + channel] as f64;
                let p10 = rgba[(y0 * width + x1) * 4 + channel] as f64;
                let p01 = rgba[(y1 * width + x0) * 4 + channel] as f64;
                let p11 = rgba[(y1 * width + x1) * 4 + channel] as f64;
                let top = p00 + (p10 - p00) * fx;
                let bottom = p01 + (p11 - p01) * fx;
                output[target + channel] = (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok((out_width, out_height, output))
}

''',
    'thumbnail resampler',
)
thumbnail = replace_between(
    thumbnail,
    'fn encode_png(',
    '#[cfg(test)]',
    '''pub(crate) fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = width as usize * height as usize * 4;
    if rgba.len() < expected {
        return Err("Thumbnail RGBA data is incomplete.".to_owned());
    }
    let opaque = rgba[..expected].chunks_exact(4).all(|pixel| pixel[3] == 255);
    let rgb = opaque.then(|| {
        let mut bytes = Vec::with_capacity(width as usize * height as usize * 3);
        for pixel in rgba[..expected].chunks_exact(4) {
            bytes.extend_from_slice(&pixel[..3]);
        }
        bytes
    });

    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(if opaque { png::ColorType::Rgb } else { png::ColorType::Rgba });
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::High);
        let mut writer = encoder
            .write_header()
            .map_err(|err| format!("Cannot initialize project thumbnail PNG: {err}"))?;
        if let Some(rgb) = rgb.as_ref() {
            writer
                .write_image_data(rgb)
                .map_err(|err| format!("Cannot encode project thumbnail PNG: {err}"))?;
        } else {
            writer
                .write_image_data(&rgba[..expected])
                .map_err(|err| format!("Cannot encode project thumbnail PNG: {err}"))?;
        }
    }
    Ok(bytes)
}

''',
    'thumbnail PNG encoder',
)
thumbnail = replace_once(
    thumbnail,
    'let rgba = vec![255u8; 400 * 200 * 4];\n        let (width, height, output) = resize_rgba(400, 200, &rgba, 256).unwrap();\n        assert_eq!((width, height), (256, 128));\n        assert_eq!(output.len(), 256 * 128 * 4);',
    'let rgba = vec![255u8; 1024 * 512 * 4];\n        let (width, height, output) = resize_rgba(1024, 512, &rgba, 512).unwrap();\n        assert_eq!((width, height), (512, 256));\n        assert_eq!(output.len(), 512 * 256 * 4);',
    'thumbnail resize test',
)
thumbnail_path.write_text(thumbnail, encoding='utf-8')

# ---------------- settings_v6.rs ----------------
settings_path = root / 'src' / 'settings_v6.rs'
settings = settings_path.read_text(encoding='utf-8')
settings = replace_once(
    settings,
    '    pub validate_after_export: bool,\n    pub lzw_compression: bool,',
    '    pub validate_after_export: bool,\n    pub export_all_test_code: bool,\n    pub lzw_compression: bool,',
    'settings field',
)
settings = replace_once(
    settings,
    '            validate_after_export: false,\n            lzw_compression: true,',
    '            validate_after_export: false,\n            export_all_test_code: false,\n            lzw_compression: true,',
    'settings default',
)
settings = replace_once(
    settings,
    '    fn lzw_compression_defaults_on() {\n        assert!(AppSettings::default().lzw_compression);\n    }',
    '    fn lzw_compression_defaults_on() {\n        assert!(AppSettings::default().lzw_compression);\n    }\n\n    #[test]\n    fn export_all_test_code_defaults_off() {\n        assert!(!AppSettings::default().export_all_test_code);\n    }',
    'settings default test',
)
settings_path.write_text(settings, encoding='utf-8')

# ---------------- previous_shades.rs ----------------
previous_path = root / 'src' / 'previous_shades.rs'
previous = previous_path.read_text(encoding='utf-8')
previous = replace_once(
    previous,
    'use crate::model::{FaceFileMetadata, ProjectThumbnail, ShadeProject};',
    'use crate::model::{FaceFileMetadata, ProjectThumbnail, ShadeProject};\nuse crate::thumbnail;',
    'previous shades thumbnail import',
)
previous = replace_once(previous, 'const SNAPSHOT_CACHE_VERSION: u32 = 1;', 'const SNAPSHOT_CACHE_VERSION: u32 = 2;', 'cache version')
previous = replace_once(
    previous,
    '    pub snapshots: Vec<CachedSnapshot>,\n    pub test_code_text: String,',
    '    pub snapshots: Vec<CachedSnapshot>,\n    pub test_code_text: String,\n    pub face_count: usize,\n    pub total_source_bytes: u64,\n    pub thumbnail: Option<ProjectThumbnail>,',
    'cache fields',
)
previous = replace_once(
    previous,
    '            snapshots: Vec::new(),\n            test_code_text: String::new(),',
    '            snapshots: Vec::new(),\n            test_code_text: String::new(),\n            face_count: 0,\n            total_source_bytes: 0,\n            thumbnail: None,',
    'cache field defaults',
)
previous = replace_once(
    previous,
    '    pub active_snapshot_name: Option<String>,\n    pub test_code_enabled: bool,',
    '    pub active_snapshot_name: Option<String>,\n    pub snapshots: Vec<CachedSnapshot>,\n    pub test_code_enabled: bool,',
    'inspection snapshots',
)
previous = replace_between(
    previous,
    '    fn refresh_from_project(&mut self, project: &ShadeProject) {',
    '    pub fn matches_query(&self, query_lower: &str) -> bool {',
    '''    fn refresh_from_project(&mut self, project: &ShadeProject) {
        self.project_name = project_display_name(&project.name, Path::new(&self.path));
        self.test_code_text = project.test_code.text.trim().to_owned();
        self.snapshots = snapshot_cache_from_project(project);
        self.face_count = project
            .file_metadata
            .as_ref()
            .map(|metadata| metadata.face_count)
            .filter(|count| *count > 0)
            .unwrap_or(project.faces.len());
        self.total_source_bytes = project
            .file_metadata
            .as_ref()
            .map(|metadata| metadata.total_source_bytes)
            .unwrap_or(0);
        self.thumbnail = project
            .thumbnail
            .as_ref()
            .and_then(build_cached_list_thumbnail);
        self.snapshot_cache_version = SNAPSHOT_CACHE_VERSION;
    }

    pub fn display_name(&self) -> String {
        project_display_name(&self.project_name, Path::new(&self.path))
    }

''',
    'refresh project cache',
)
previous = replace_once(
    previous,
    '        contains_case_insensitive(&self.project_name, query)\n            || contains_case_insensitive(&self.path, query)',
    '        contains_case_insensitive(&self.display_name(), query)\n            || contains_case_insensitive(&self.path, query)',
    'search display name',
)
previous = replace_once(
    previous,
    '        let cached_name = loaded_project\n            .as_ref()\n            .map(|project| project.name.trim())\n            .filter(|name| !name.is_empty())\n            .unwrap_or_else(|| project_name.trim())\n            .to_owned();',
    '        let cached_name = loaded_project\n            .as_ref()\n            .map(|project| project_display_name(&project.name, path))\n            .unwrap_or_else(|| project_display_name(project_name, path));',
    'cached display name',
)
previous = replace_once(
    previous,
    '                    existing.snapshots = entry.snapshots.clone();\n                    existing.test_code_text = entry.test_code_text.clone();',
    '                    existing.snapshots = entry.snapshots.clone();\n                    existing.test_code_text = entry.test_code_text.clone();\n                    existing.face_count = entry.face_count;\n                    existing.total_source_bytes = entry.total_source_bytes;\n                    existing.thumbnail = entry.thumbnail.clone();',
    'sanitize new cache fields newest',
)
previous = replace_once(
    previous,
    '                    existing.snapshots = entry.snapshots.clone();\n                    existing.test_code_text = entry.test_code_text.clone();\n                }\n                existing.open_count',
    '                    existing.snapshots = entry.snapshots.clone();\n                    existing.test_code_text = entry.test_code_text.clone();\n                    existing.face_count = entry.face_count;\n                    existing.total_source_bytes = entry.total_source_bytes;\n                    existing.thumbnail = entry.thumbnail.clone();\n                }\n                existing.open_count',
    'sanitize new cache fields version',
)
previous = replace_once(
    previous,
    '        project_name: project.name.clone(),\n        snapshot_count: project.snapshots.len(),\n        active_snapshot_name: project.active_snapshot_name().map(str::to_owned),\n        test_code_enabled: project.test_code.enabled,',
    '        project_name: project_display_name(&project.name, path),\n        snapshot_count: project.snapshots.len(),\n        active_snapshot_name: project.active_snapshot_name().map(str::to_owned),\n        snapshots: snapshot_cache_from_project(&project),\n        test_code_enabled: project.test_code.enabled,',
    'inspection display/snapshots',
)
previous = replace_between(
    previous,
    'fn decode_thumbnail(thumbnail: &ProjectThumbnail) -> Result<DecodedThumbnail, String> {',
    'fn history_path() -> PathBuf {',
    '''fn decode_thumbnail(thumbnail: &ProjectThumbnail) -> Result<DecodedThumbnail, String> {
    if !thumbnail.mime_type.eq_ignore_ascii_case("image/png") {
        return Err(format!(
            "Unsupported project thumbnail type: {}",
            thumbnail.mime_type
        ));
    }
    let bytes = BASE64_STANDARD
        .decode(thumbnail.data_base64.as_bytes())
        .map_err(|err| format!("Invalid project thumbnail base64: {err}"))?;
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|err| format!("Cannot read project thumbnail PNG: {err}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| "Project thumbnail PNG exceeds decoder limits.".to_owned())?;
    let mut buffer = vec![0u8; size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|err| format!("Cannot decode project thumbnail PNG: {err}"))?;
    let pixels = &buffer[..info.buffer_size()];
    if info.bit_depth != png::BitDepth::Eight {
        return Err("Project thumbnail PNG is not 8-bit.".to_owned());
    }
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for pixel in pixels.chunks_exact(3) {
                rgba.extend_from_slice(pixel);
                rgba.push(255);
            }
            rgba
        }
        _ => return Err("Project thumbnail PNG must be RGB or RGBA.".to_owned()),
    };
    Ok(DecodedThumbnail {
        width: info.width as usize,
        height: info.height as usize,
        rgba,
    })
}

pub fn decode_cached_thumbnail(entry: &PreviousShadeEntry) -> Result<Option<DecodedThumbnail>, String> {
    entry.thumbnail.as_ref().map(decode_thumbnail).transpose()
}

fn project_display_name(project_name: &str, path: &Path) -> String {
    let trimmed = project_name.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("Untitled Shade") {
        return trimmed.to_owned();
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Shade project".to_owned())
}

fn snapshot_cache_from_project(project: &ShadeProject) -> Vec<CachedSnapshot> {
    let explicit_code = project.test_code.text.trim();
    project
        .snapshots
        .iter()
        .map(|snapshot| {
            let name = snapshot.name.trim().to_owned();
            CachedSnapshot {
                id: snapshot.id,
                code: if explicit_code.is_empty() {
                    name.clone()
                } else {
                    explicit_code.to_owned()
                },
                name,
                created_at_unix_ms: snapshot.created_at_unix_ms,
            }
        })
        .collect()
}

fn build_cached_list_thumbnail(source: &ProjectThumbnail) -> Option<ProjectThumbnail> {
    let decoded = decode_thumbnail(source).ok()?;
    let (width, height, rgba) = thumbnail::resize_rgba(
        decoded.width,
        decoded.height,
        &decoded.rgba,
        72,
    )
    .ok()?;
    let png = thumbnail::encode_png(width as u32, height as u32, &rgba).ok()?;
    Some(ProjectThumbnail {
        mime_type: "image/png".to_owned(),
        width: width as u32,
        height: height as u32,
        data_base64: BASE64_STANDARD.encode(png),
    })
}

''',
    'thumbnail decode/cache helpers',
)
previous = replace_once(
    previous,
    '        assert_eq!(store.entries()[0].open_count, 2);',
    '        assert_eq!(store.entries()[0].open_count, 2);\n        assert_eq!(store.entries()[0].display_name(), "example");',
    'history display name test',
)
previous_path.write_text(previous, encoding='utf-8')

# ---------------- app_main.rs ----------------
app_path = root / 'src' / 'app_main.rs'
app = app_path.read_text(encoding='utf-8')
app = replace_once(
    app,
    '    previous_shade_texture: Option<egui::TextureHandle>,\n    remind_after_export: bool,',
    '    previous_shade_texture: Option<egui::TextureHandle>,\n    previous_shade_list_textures: BTreeMap<String, egui::TextureHandle>,\n    remind_after_export: bool,',
    'list texture field',
)
app = replace_once(
    app,
    '            previous_shade_texture: None,\n            remind_after_export: false,',
    '            previous_shade_texture: None,\n            previous_shade_list_textures: BTreeMap::new(),\n            remind_after_export: false,',
    'list texture init',
)
app = replace_once(
    app,
    '            adjustment_scope: AdjustmentScope::Selected,',
    '            adjustment_scope: AdjustmentScope::All,',
    'default all scope',
)
app = replace_count(
    app,
    '        self.adjustment_scope = AdjustmentScope::Selected;',
    '        self.adjustment_scope = AdjustmentScope::All;',
    3,
    'reset all scope',
)
app = replace_once(
    app,
    '        let mut project = self.project.clone();\n        project.file_metadata = Some(build_project_file_metadata(',
    '        let mut project = self.project.clone();\n        project.name = project_name_for_path(&project.name, &path);\n        project.file_metadata = Some(build_project_file_metadata(',
    'save project name normalization',
)
app = replace_once(
    app,
    '                Ok(()) => {\n                    self.project_path = Some(path.clone());',
    '                Ok(()) => {\n                    self.project.name = project_name_for_path(&self.project.name, &path);\n                    self.project_path = Some(path.clone());',
    'runtime saved project name',
)
app = replace_between(
    app,
    '    fn export_all_dialog(&mut self) {',
    '    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {',
    '''    fn export_all_dialog(&mut self) {
        if self.job.is_some() || self.faces.is_empty() {
            return;
        }
        if self.faces.iter().any(|face| !face.available) {
            self.report_error("Export all requires every Face source TIFF to be available. Relink missing Faces first.");
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let sources = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        let mut project = self.project.clone();
        project.test_code.enabled = self.settings.export_all_test_code;
        let default_dpi = self.settings.default_dpi;
        let force_lzw = self.settings.lzw_compression;
        let validate_after_export = self.settings.validate_after_export;
        self.remind_after_export = self.snapshot_project_needs_save_reminder();
        self.launch_job("Exporting faces", move |progress| {
            let total = sources.len().max(1);
            let result = (|| -> Result<String, String> {
                for (index, source) in sources.iter().enumerate() {
                    let stem = source
                        .file_stem()
                        .map(|value| value.to_string_lossy())
                        .unwrap_or_default();
                    let destination = folder.join(format!("{stem}-shade.tif"));
                    export::export_face_with_progress_options(
                        source,
                        &destination,
                        &project,
                        default_dpi,
                        export::ExportOptions { force_lzw },
                        |inner, detail| {
                            let phase = if validate_after_export {
                                inner * 0.88
                            } else {
                                inner
                            };
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
                        validation::validate_export_transport_with_options(
                            source,
                            &destination,
                            force_lzw,
                        )?;
                    }
                }
                let code_note = if project.test_code.enabled { " with Test Code" } else { " without Test Code" };
                if validate_after_export {
                    Ok(format!(
                        "Exported and verified {total} face(s){code_note} to {}",
                        folder.display()
                    ))
                } else {
                    Ok(format!("Exported {total} face(s){code_note} to {}", folder.display()))
                }
            })();
            JobResult::Export(SnapshotExportBatchResult {
                result,
                marks: Vec::new(),
            })
        });
    }

''',
    'export all policy',
)
app = replace_once(
    app,
    '    fn remember_previous_shade(&mut self, path: &Path) {\n        self.previous_shades.record_open(path, &self.project.name);',
    '    fn remember_previous_shade(&mut self, path: &Path) {\n        self.previous_shade_list_textures.clear();\n        self.previous_shades.record_open(path, &self.project.name);',
    'clear cached list textures',
)
app = replace_between(
    app,
    '    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {',
    '    fn ui_selected_adjustment(',
    '''    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {
        let adjustments_before = self.project.adjustments.clone();
        let Some(face) = self.faces.get(self.current_face) else {
            ui.heading("Adjustments");
            ui.label("No active face");
            return;
        };
        if !face.available {
            ui.heading("Adjustments");
            ui.label("Source TIFF missing. Relink this Face before editing its channels.");
            return;
        }
        let channel_names = face.preview.metadata.channel_names.clone();
        if channel_names.is_empty() {
            return;
        }
        self.selected_channel = self.selected_channel.min(channel_names.len() - 1);
        let output_name = channel_names[self.selected_channel].clone();
        let palette = self.project.channel_palette.clone();
        let output_display =
            channel_display_name(palette.as_ref(), &output_name, self.selected_channel);
        let all_adjusted_histograms = face
            .adjusted
            .iter()
            .map(|values| render::histogram(values))
            .collect::<Vec<_>>();
        let active_histogram = all_adjusted_histograms.get(self.selected_channel).copied();
        let control_accent = self
            .settings
            .colorize_adjustments
            .then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));
        let panel_accent = (self.adjustment_scope == AdjustmentScope::Selected)
            .then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));

        ui.horizontal_wrapped(|ui| {
            ui.heading("Adjustments");
            let mut all_channels = self.adjustment_scope == AdjustmentScope::All;
            if ui.checkbox(&mut all_channels, "All channels").changed() {
                self.adjustment_scope = if all_channels {
                    AdjustmentScope::All
                } else {
                    AdjustmentScope::Selected
                };
            }
            let selected = self.adjustment_scope == AdjustmentScope::Selected;
            let response = with_accent(ui, control_accent, |ui| {
                ui.add(egui::Button::new(output_display).selected(selected))
            });
            if response.clicked() {
                self.adjustment_scope = AdjustmentScope::Selected;
            }
            let layout_label = if self.settings.adjustment_tabs {
                "Tabs"
            } else {
                "Stacked"
            };
            if ui.small_button(layout_label).clicked() {
                self.settings.adjustment_tabs = !self.settings.adjustment_tabs;
                self.save_settings_quietly();
            }
        });

        let mut frame = egui::Frame::new().inner_margin(8).corner_radius(6);
        if let Some(color) = panel_accent {
            frame = frame.stroke(egui::Stroke::new(1.5, color.gamma_multiply(0.72)));
        } else {
            frame = frame.stroke(ui.visuals().widgets.noninteractive.bg_stroke);
        }
        let changed = frame
            .show(ui, |ui| {
                if let Some(color) = panel_accent {
                    ui.visuals_mut().widgets.noninteractive.bg_stroke.color =
                        color.gamma_multiply(0.52);
                }
                let mut header_changed = false;
                let reset_all = ui
                    .horizontal(|ui| {
                        match self.adjustment_scope {
                            AdjustmentScope::Selected => {
                                if let Some(color) = panel_accent {
                                    ui.colored_label(color, format!("Editing: {output_display}"));
                                } else {
                                    ui.strong(format!("Editing: {output_display}"));
                                }
                                let enabled = &mut self
                                    .project
                                    .adjustments
                                    .entry(output_name.clone())
                                    .or_default()
                                    .enabled;
                                header_changed |= ui.checkbox(enabled, "Enabled").changed();
                            }
                            AdjustmentScope::All => {
                                ui.strong("Editing: All channels");
                                let mut all_enabled = channel_names.iter().all(|name| {
                                    self.project
                                        .adjustments
                                        .get(name)
                                        .map(|adjustment| adjustment.enabled)
                                        .unwrap_or(true)
                                });
                                if ui.checkbox(&mut all_enabled, "Enabled").changed() {
                                    for name in &channel_names {
                                        self.project
                                            .adjustments
                                            .entry(name.clone())
                                            .or_default()
                                            .enabled = all_enabled;
                                    }
                                    header_changed = true;
                                }
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.small_button("Reset all").clicked()
                        })
                        .inner
                    })
                    .inner;
                if reset_all {
                    self.project.reset_adjustments(&channel_names);
                    self.mark_all_previews_dirty();
                    self.report_info("All adjustments reset to defaults");
                }
                let body_changed = match self.adjustment_scope {
                    AdjustmentScope::Selected => self.ui_selected_adjustment(
                        ui,
                        &output_name,
                        &channel_names,
                        active_histogram.as_ref(),
                        control_accent,
                        palette.as_ref(),
                    ),
                    AdjustmentScope::All => self.ui_all_adjustments(
                        ui,
                        &output_name,
                        &channel_names,
                        &all_adjusted_histograms,
                        control_accent,
                        palette.as_ref(),
                    ),
                };
                header_changed || body_changed
            })
            .inner;
        if changed {
            self.mark_all_previews_dirty();
        }
        if self.project.adjustments != adjustments_before {
            self.queue_adjustment_history(&adjustments_before);
        }
    }

''',
    'adjustments header/layout',
)
app = replace_between(
    app,
    '    fn ui_selected_adjustment(',
    '    fn ui_all_adjustments(',
    '''    fn ui_selected_adjustment(
        &mut self,
        ui: &mut egui::Ui,
        output_name: &str,
        channel_names: &[String],
        histogram: Option<&[u32; 256]>,
        accent: Option<egui::Color32>,
        palette: Option<&ChannelPalette>,
    ) -> bool {
        let mut changed = false;
        let compact_curve_controls = self.settings.compact_curve_controls;
        let adjustment = self
            .project
            .adjustments
            .entry(output_name.to_owned())
            .or_default();
        ui.add_enabled_ui(adjustment.enabled, |ui| {
            if self.settings.adjustment_tabs {
                let reset_tool = adjustment_tab_bar(ui, &mut self.tool);
                if reset_tool {
                    match self.tool {
                        ToolPanel::Levels => adjustment.levels = model::Levels::default(),
                        ToolPanel::Curves => adjustment.curve = model::Curve::default(),
                        ToolPanel::Mixer => reset_mixer_row(adjustment, output_name, channel_names),
                    }
                    changed = true;
                }
                changed |= match self.tool {
                    ToolPanel::Levels => levels_ui(ui, adjustment, accent),
                    ToolPanel::Curves => curves_ui(
                        ui,
                        adjustment,
                        histogram.filter(|_| self.settings.show_curve_histogram),
                        accent,
                        compact_curve_controls,
                    ),
                    ToolPanel::Mixer => {
                        mixer_ui(ui, adjustment, output_name, channel_names, accent, palette)
                    }
                };
            } else {
                let (body_changed, reset) = adjustment_foldout(
                    ui,
                    format!("selected-levels-{output_name}"),
                    "Levels",
                    true,
                    |ui| levels_ui(ui, adjustment, accent),
                );
                changed |= body_changed.unwrap_or(false);
                if reset {
                    adjustment.levels = model::Levels::default();
                    changed = true;
                }

                ui.add_space(4.0);
                let (body_changed, reset) = adjustment_foldout(
                    ui,
                    format!("selected-mixer-{output_name}"),
                    "Channel Mixer",
                    true,
                    |ui| mixer_ui(ui, adjustment, output_name, channel_names, accent, palette),
                );
                changed |= body_changed.unwrap_or(false);
                if reset {
                    reset_mixer_row(adjustment, output_name, channel_names);
                    changed = true;
                }

                ui.add_space(4.0);
                let (body_changed, reset) = adjustment_foldout(
                    ui,
                    format!("selected-curve-{output_name}"),
                    "Curve",
                    true,
                    |ui| {
                        curves_ui(
                            ui,
                            adjustment,
                            histogram.filter(|_| self.settings.show_curve_histogram),
                            accent,
                            compact_curve_controls,
                        )
                    },
                );
                changed |= body_changed.unwrap_or(false);
                if reset {
                    adjustment.curve = model::Curve::default();
                    changed = true;
                }
            }
        });
        changed
    }

''',
    'selected adjustment controls',
)
app = replace_between(
    app,
    '    fn ui_all_adjustments(',
    '    fn ui_tools(&mut self, ui: &mut egui::Ui) {',
    '''    fn ui_all_adjustments(
        &mut self,
        ui: &mut egui::Ui,
        template_name: &str,
        channel_names: &[String],
        histograms: &[[u32; 256]],
        accent: Option<egui::Color32>,
        palette: Option<&ChannelPalette>,
    ) -> bool {
        let mut changed = false;
        let compact_curve_controls = self.settings.compact_curve_controls;
        ui.small(
            "Levels broadcasts to every channel. Mixer output rows remain independent. Curve keeps one Broadcast control plus independent per-channel foldouts.",
        );

        if self.settings.adjustment_tabs {
            let reset_tool = adjustment_tab_bar(ui, &mut self.tool);
            if reset_tool {
                match self.tool {
                    ToolPanel::Levels => {
                        reset_all_levels(&mut self.project.adjustments, channel_names)
                    }
                    ToolPanel::Curves => {
                        reset_all_curves(&mut self.project.adjustments, channel_names)
                    }
                    ToolPanel::Mixer => {
                        reset_all_mixers(&mut self.project.adjustments, channel_names)
                    }
                }
                changed = true;
            }
            changed |= match self.tool {
                ToolPanel::Levels => broadcast_levels_ui(
                    ui,
                    &mut self.project.adjustments,
                    template_name,
                    channel_names,
                    accent,
                ),
                ToolPanel::Curves => all_curves_ui(
                    ui,
                    &mut self.project.adjustments,
                    template_name,
                    channel_names,
                    histograms,
                    self.settings.colorize_adjustments,
                    self.settings.show_curve_histogram,
                    compact_curve_controls,
                    palette,
                ),
                ToolPanel::Mixer => all_mixers_ui(
                    ui,
                    &mut self.project.adjustments,
                    channel_names,
                    self.settings.colorize_adjustments,
                    palette,
                ),
            };
        } else {
            let (body_changed, reset) = adjustment_foldout(
                ui,
                "all-levels-section",
                "Levels - all channels",
                true,
                |ui| {
                    broadcast_levels_ui(
                        ui,
                        &mut self.project.adjustments,
                        template_name,
                        channel_names,
                        accent,
                    )
                },
            );
            changed |= body_changed.unwrap_or(false);
            if reset {
                reset_all_levels(&mut self.project.adjustments, channel_names);
                changed = true;
            }

            ui.add_space(4.0);
            let (body_changed, reset) = adjustment_foldout(
                ui,
                "all-mixers-section",
                "Channel Mixer - all output rows",
                true,
                |ui| {
                    all_mixers_ui(
                        ui,
                        &mut self.project.adjustments,
                        channel_names,
                        self.settings.colorize_adjustments,
                        palette,
                    )
                },
            );
            changed |= body_changed.unwrap_or(false);
            if reset {
                reset_all_mixers(&mut self.project.adjustments, channel_names);
                changed = true;
            }

            ui.add_space(4.0);
            let (body_changed, reset) = adjustment_foldout(
                ui,
                "all-curves-section",
                "Curve - broadcast + per channel",
                true,
                |ui| {
                    all_curves_ui(
                        ui,
                        &mut self.project.adjustments,
                        template_name,
                        channel_names,
                        histograms,
                        self.settings.colorize_adjustments,
                        self.settings.show_curve_histogram,
                        compact_curve_controls,
                        palette,
                    )
                },
            );
            changed |= body_changed.unwrap_or(false);
            if reset {
                reset_all_curves(&mut self.project.adjustments, channel_names);
                changed = true;
            }
        }
        changed
    }

''',
    'all adjustment controls',
)
app = replace_between(
    app,
    '    fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {',
    '    fn ui_snapshot_save_reminder(&mut self, ctx: &egui::Context) {',
    '''    fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {
        if !self.show_previous_shades {
            return;
        }
        let mut open = self.show_previous_shades;
        let mut requested_select: Option<String> = None;
        let mut requested_open: Option<String> = None;
        let mut requested_folder: Option<PathBuf> = None;
        let mut clear_selection = false;
        egui::Window::new("Previous shades")
            .open(&mut open)
            .resizable(true)
            .default_size([1060.0, 680.0])
            .show(ctx, |ui| {
                let query_before = self.previous_shades_query.clone();
                ui.horizontal(|ui| {
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.previous_shades_query)
                            .hint_text("Search project, path, snapshot name or test code...")
                            .desired_width(360.0),
                    );
                    if !search.has_focus() {
                        let typed = ui.input(|input| {
                            if input.modifiers.ctrl || input.modifiers.alt || input.modifiers.command {
                                String::new()
                            } else {
                                input
                                    .events
                                    .iter()
                                    .filter_map(|event| match event {
                                        egui::Event::Text(text) => Some(text.as_str()),
                                        _ => None,
                                    })
                                    .collect::<String>()
                            }
                        });
                        if !typed.is_empty() {
                            self.previous_shades_query.push_str(&typed);
                            search.request_focus();
                        }
                    }
                    egui::ComboBox::from_id_salt("previous-shades-sort")
                        .selected_text(self.previous_shades_sort.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.previous_shades_sort,
                                previous_shades::PreviousShadesSort::LastOpened,
                                "Last opened",
                            );
                            ui.selectable_value(
                                &mut self.previous_shades_sort,
                                previous_shades::PreviousShadesSort::ProjectName,
                                "Project name",
                            );
                            ui.selectable_value(
                                &mut self.previous_shades_sort,
                                previous_shades::PreviousShadesSort::SavedAt,
                                "Saved time",
                            );
                            ui.selectable_value(
                                &mut self.previous_shades_sort,
                                previous_shades::PreviousShadesSort::Path,
                                "Path",
                            );
                        });
                    ui.add_enabled(false, egui::Button::new("Load all shades from system"))
                        .on_hover_text("Reserved for the Everything Search integration. History already uses the same importable entry model.");
                });
                ui.small("Type to search. Up/Down moves through results even while Search has focus; Enter opens the selected project.");
                ui.separator();

                let query_changed = self.previous_shades_query != query_before;
                let query = self.previous_shades_query.trim().to_lowercase();
                let mut rows = self.previous_shades.entries().to_vec();
                if !query.is_empty() {
                    rows.retain(|entry| entry.matches_query(&query));
                }
                match self.previous_shades_sort {
                    previous_shades::PreviousShadesSort::LastOpened => {
                        rows.sort_by(|a, b| b.last_opened_unix_ms.cmp(&a.last_opened_unix_ms));
                    }
                    previous_shades::PreviousShadesSort::ProjectName => {
                        rows.sort_by(|a, b| {
                            a.display_name()
                                .to_lowercase()
                                .cmp(&b.display_name().to_lowercase())
                                .then_with(|| b.last_opened_unix_ms.cmp(&a.last_opened_unix_ms))
                        });
                    }
                    previous_shades::PreviousShadesSort::SavedAt => {
                        rows.sort_by(|a, b| b.saved_at_unix_ms.cmp(&a.saved_at_unix_ms));
                    }
                    previous_shades::PreviousShadesSort::Path => {
                        rows.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
                    }
                }

                for entry in &rows {
                    if self.previous_shade_list_textures.contains_key(&entry.path) {
                        continue;
                    }
                    if let Ok(Some(thumbnail)) = previous_shades::decode_cached_thumbnail(entry) {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [thumbnail.width, thumbnail.height],
                            &thumbnail.rgba,
                        );
                        let texture = ctx.load_texture(
                            format!("previous-shade-list:{}", entry.path),
                            image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.previous_shade_list_textures
                            .insert(entry.path.clone(), texture);
                    }
                }

                let selection_visible = self.previous_shades_selected.as_deref().is_some_and(|path| {
                    rows.iter().any(|entry| entry.path == path)
                });
                if query_changed || !selection_visible {
                    requested_select = rows.first().map(|entry| entry.path.clone());
                    if rows.is_empty() {
                        clear_selection = true;
                    }
                }

                let (up, down, enter) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                        input.key_pressed(egui::Key::Enter),
                    )
                });
                if !rows.is_empty() && (up || down) {
                    let effective = requested_select
                        .as_deref()
                        .or(self.previous_shades_selected.as_deref());
                    let current = effective
                        .and_then(|path| rows.iter().position(|entry| entry.path == path))
                        .unwrap_or(0);
                    let next = if down {
                        (current + 1).min(rows.len() - 1)
                    } else {
                        current.saturating_sub(1)
                    };
                    requested_select = Some(rows[next].path.clone());
                }
                if enter {
                    if let Some(path) = requested_select
                        .as_deref()
                        .or(self.previous_shades_selected.as_deref())
                    {
                        if Path::new(path).is_file() {
                            requested_open = Some(path.to_owned());
                        }
                    }
                }

                ui.columns(2, |columns| {
                    columns[0].strong(format!("History · {}", rows.len()));
                    columns[0].add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("previous-shades-history")
                        .show(&mut columns[0], |ui| {
                            for entry in &rows {
                                let display_name = entry.display_name();
                                let label = if Path::new(&entry.path).is_file() {
                                    display_name
                                } else {
                                    format!("[missing] {display_name}")
                                };
                                let opened = format_previous_shade_time(entry.last_opened_unix_ms);
                                let match_detail = if query.is_empty() {
                                    None
                                } else {
                                    entry
                                        .matching_snapshot(&query)
                                        .map(|snapshot| {
                                            if snapshot.code.trim().is_empty()
                                                || snapshot.code.eq_ignore_ascii_case(&snapshot.name)
                                            {
                                                format!("Snapshot: {} · #{}", snapshot.name, snapshot.id)
                                            } else {
                                                format!("Snapshot: {} · code {}", snapshot.name, snapshot.code)
                                            }
                                        })
                                        .or_else(|| {
                                            entry.test_code_matches(&query).then(|| {
                                                format!("Test code: {}", entry.test_code_text)
                                            })
                                        })
                                };
                                let detail = match_detail.as_deref().unwrap_or(&opened);
                                let source_bytes = if entry.total_source_bytes > 0 {
                                    format_byte_count(entry.total_source_bytes)
                                } else {
                                    "-".to_owned()
                                };
                                let metadata = format!("{} face(s) · {source_bytes}", entry.face_count);
                                let selected = requested_select.as_deref()
                                    .or(self.previous_shades_selected.as_deref())
                                    == Some(entry.path.as_str());
                                let thumbnail = self.previous_shade_list_textures.get(&entry.path);
                                let response = previous_shade_history_row(
                                    ui,
                                    selected,
                                    &label,
                                    &metadata,
                                    detail,
                                    thumbnail,
                                )
                                .on_hover_text(&entry.path);
                                if response.clicked() {
                                    requested_select = Some(entry.path.clone());
                                }
                                if response.double_clicked() && Path::new(&entry.path).is_file() {
                                    requested_open = Some(entry.path.clone());
                                }
                            }
                        });

                    columns[1].strong("Preview");
                    columns[1].add_space(4.0);
                    if let Some(err) = self.previous_shade_preview_error.as_ref() {
                        columns[1].colored_label(egui::Color32::LIGHT_RED, err);
                        if let Some(path) = self.previous_shades_selected.as_deref() {
                            columns[1].label(path);
                        }
                    } else if let Some(preview) = self.previous_shade_preview.as_ref() {
                        columns[1].heading(&preview.project_name);
                        if let Some(texture) = self.previous_shade_texture.as_ref() {
                            let natural = texture.size_vec2();
                            if natural.x > 0.0 && natural.y > 0.0 {
                                let max_size = egui::vec2(columns[1].available_width().min(440.0), 280.0);
                                let scale = (max_size.x / natural.x)
                                    .min(max_size.y / natural.y)
                                    .min(1.0);
                                columns[1].add(
                                    egui::Image::from_texture(texture)
                                        .fit_to_exact_size(natural * scale),
                                );
                            }
                        } else if let Some(err) = preview.thumbnail_error.as_ref() {
                            columns[1].small(format!("Thumbnail unavailable: {err}"));
                        } else {
                            columns[1].small("No embedded thumbnail in this .shade file.");
                        }
                        columns[1].add_space(6.0);
                        egui::Grid::new("previous-shade-preview-meta")
                            .num_columns(2)
                            .striped(true)
                            .spacing([12.0, 5.0])
                            .show(&mut columns[1], |ui| {
                                ui.strong("Saved");
                                ui.label(format_previous_shade_time(preview.saved_at_unix_ms));
                                ui.end_row();
                                ui.strong("File modified");
                                ui.label(preview.file_modified_unix_ms.map(format_previous_shade_time).unwrap_or_else(|| "-".to_owned()));
                                ui.end_row();
                                ui.strong("Faces");
                                ui.label(preview.face_count.to_string());
                                ui.end_row();
                                ui.strong("Snapshots");
                                ui.label(preview.snapshot_count.to_string());
                                ui.end_row();
                                ui.strong("Active snapshot");
                                ui.label(preview.active_snapshot_name.as_deref().unwrap_or("-"));
                                ui.end_row();
                                ui.strong("Test code");
                                ui.label(if preview.test_code_enabled { "Enabled" } else { "Off" });
                                ui.end_row();
                                ui.strong("Source bytes");
                                ui.label(format_byte_count(preview.total_source_bytes));
                                ui.end_row();
                            });

                        columns[1].separator();
                        columns[1].strong(format!("Snapshots · {}", preview.snapshots.len()));
                        if preview.snapshots.is_empty() {
                            columns[1].small("No saved Snapshots in this project.");
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("previous-shade-snapshots")
                                .max_height(170.0)
                                .show(&mut columns[1], |ui| {
                                    for snapshot in &preview.snapshots {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.strong(format!("#{}", snapshot.id));
                                            ui.label(&snapshot.name);
                                        });
                                        if !snapshot.code.trim().is_empty()
                                            && !snapshot.code.eq_ignore_ascii_case(&snapshot.name)
                                        {
                                            ui.small(format!("Code: {}", snapshot.code));
                                        }
                                    }
                                });
                        }

                        if let Some(face) = preview.active_face.as_ref() {
                            columns[1].separator();
                            columns[1].strong(format!(
                                "Face {} of {} · {}",
                                preview.active_face_index.saturating_add(1).min(preview.face_count.max(1)),
                                preview.face_count,
                                face.label
                            ));
                            columns[1].label(format!("{} · {} x {} px · {}-bit · {}", face.source_file_name, face.width, face.height, face.bit_depth, face.color_model));
                            columns[1].label(format!("{:.0} x {:.0} DPI · {} channels · {}", face.dpi_x, face.dpi_y, face.channel_count, format_byte_count(face.file_size_bytes)));
                            if !face.channel_names.is_empty() {
                                columns[1].small(format!("Channels: {}", face.channel_names.join(", ")));
                            }
                        }
                        columns[1].separator();
                        columns[1].small(preview.path.display().to_string());
                        columns[1].horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    self.job.is_none() && preview.path.is_file(),
                                    egui::Button::new("Open selected .shade"),
                                )
                                .clicked()
                            {
                                requested_open = Some(preview.path.to_string_lossy().into_owned());
                            }
                            let folder = preview.path.parent().map(Path::to_path_buf);
                            if ui
                                .add_enabled(
                                    folder.as_ref().is_some_and(|path| path.is_dir()),
                                    egui::Button::new("Open shade folder"),
                                )
                                .clicked()
                            {
                                requested_folder = folder;
                            }
                        });
                    } else {
                        columns[1].label("Select a project to inspect its embedded thumbnail, Snapshots and saved metadata.");
                    }
                });
            });
        self.show_previous_shades = open;
        if clear_selection {
            self.previous_shades_selected = None;
            self.previous_shade_preview = None;
            self.previous_shade_preview_error = None;
            self.previous_shade_texture = None;
        }
        if let Some(path) = requested_select {
            self.load_previous_shade_preview(ctx, &path);
        }
        if let Some(folder) = requested_folder {
            if let Err(err) = open_folder(&folder) {
                self.report_error(err);
            }
        }
        if let Some(path) = requested_open {
            self.show_previous_shades = false;
            self.open_project_path(PathBuf::from(path));
        }
    }

''',
    'previous shades window',
)
app = replace_once(
    app,
    '                changed |= ui\n                    .checkbox(\n                        &mut self.settings.validate_after_export,\n                        "Validate TIFF after normal Export face / Export all",\n                    )\n                    .changed();\n                ui.small("When enabled, Shade Editor immediately re-decodes every exported TIFF and verifies channel layout/names, ICC/Photoshop resources, compression/predictor policy and complete strip decoding.");',
    '                changed |= ui\n                    .checkbox(\n                        &mut self.settings.validate_after_export,\n                        "Validate TIFF after normal Export face / Export all",\n                    )\n                    .changed();\n                ui.small("When enabled, Shade Editor immediately re-decodes every exported TIFF and verifies channel layout/names, ICC/Photoshop resources, compression/predictor policy and complete strip decoding.");\n                changed |= ui\n                    .checkbox(\n                        &mut self.settings.export_all_test_code,\n                        "Write Test Code during Export all",\n                    )\n                    .changed();\n                ui.small("Off by default: Export all writes clean Face TIFFs without Test Code. Enable this only when every Face in Export all should receive the current Test Code configuration.");',
    'export all setting UI',
)
helper_marker = 'fn unique_shade_path(directory: &Path, stem: &str) -> PathBuf {'
helpers = '''fn project_name_for_path(current: &str, path: &Path) -> String {
    let trimmed = current.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("Untitled Shade") {
        return trimmed.to_owned();
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Shade Project".to_owned())
}

fn previous_shade_history_row(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    metadata: &str,
    detail: &str,
    thumbnail: Option<&egui::TextureHandle>,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
    let height = 68.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let visuals = ui.visuals();
    let fill = if selected {
        visuals.selection.bg_fill.gamma_multiply(0.72)
    } else if response.hovered() {
        visuals.widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, fill);
    }

    let thumb_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(7.0, 8.0),
        egui::vec2(52.0, 52.0),
    );
    if let Some(texture) = thumbnail {
        ui.painter().image(
            texture.id(),
            thumb_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        ui.painter().rect_stroke(
            thumb_rect,
            4.0,
            visuals.widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            thumb_rect.center(),
            egui::Align2::CENTER_CENTER,
            "—",
            egui::FontId::proportional(16.0),
            visuals.weak_text_color(),
        );
    }

    let text_left = thumb_rect.right() + 9.0;
    ui.painter().text(
        egui::pos2(text_left, rect.top() + 16.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.5),
        if selected { visuals.selection.stroke.color } else { visuals.text_color() },
    );
    ui.painter().text(
        egui::pos2(text_left, rect.top() + 36.0),
        egui::Align2::LEFT_CENTER,
        metadata,
        egui::FontId::proportional(12.0),
        visuals.weak_text_color(),
    );
    ui.painter().text(
        egui::pos2(text_left, rect.top() + 54.0),
        egui::Align2::LEFT_CENTER,
        detail,
        egui::FontId::proportional(11.5),
        visuals.weak_text_color(),
    );
    response
}

fn adjustment_tab_bar(ui: &mut egui::Ui, tool: &mut ToolPanel) -> bool {
    ui.add_space(9.0);
    let mut reset = false;
    ui.horizontal(|ui| {
        if ui
            .add_sized(
                [82.0, 32.0],
                egui::Button::new("Levels").selected(*tool == ToolPanel::Levels),
            )
            .clicked()
        {
            *tool = ToolPanel::Levels;
        }
        if ui
            .add_sized(
                [82.0, 32.0],
                egui::Button::new("Mixer").selected(*tool == ToolPanel::Mixer),
            )
            .clicked()
        {
            *tool = ToolPanel::Mixer;
        }
        if ui
            .add_sized(
                [82.0, 32.0],
                egui::Button::new("Curve").selected(*tool == ToolPanel::Curves),
            )
            .clicked()
        {
            *tool = ToolPanel::Curves;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            reset = ui.add_sized([64.0, 32.0], egui::Button::new("Reset")).clicked();
        });
    });
    ui.add_space(9.0);
    reset
}

'''
if helper_marker not in app:
    raise RuntimeError('helper insertion marker missing')
app = app.replace(helper_marker, helpers + helper_marker, 1)
app_path.write_text(app, encoding='utf-8')

# ---------------- version / notes ----------------
cargo_path = root / 'Cargo.toml'
cargo = cargo_path.read_text(encoding='utf-8')
cargo = replace_once(cargo, 'version = "0.13.1"', 'version = "0.13.2"', 'Cargo version')
cargo_path.write_text(cargo, encoding='utf-8')

lock_path = root / 'Cargo.lock'
lock = lock_path.read_text(encoding='utf-8')
lock = replace_once(
    lock,
    'name = "windows-shade-editor"\nversion = "0.13.1"',
    'name = "windows-shade-editor"\nversion = "0.13.2"',
    'lock package version',
)
lock_path.write_text(lock, encoding='utf-8')

notes_path = root / 'RELEASE_NOTES.md'
notes = notes_path.read_text(encoding='utf-8')
notes = '''# Shade Editor 0.13.2

- Upgrade embedded project thumbnails to 512px PNG with bilinear resampling, high PNG compression, and RGB encoding when alpha is fully opaque.
- Cache a compact 72px Previous Shades list thumbnail plus Face count and source bytes for fast history rows, including offline entries.
- Use the `.shade` filename when a project still carries the default `Untitled Shade` name, and normalize the name on the next successful Save/Quick Save.
- Previous Shades now supports Enter to open, Up/Down navigation while Search has focus, first-result selection while searching, an Open shade folder action, and a Snapshot list in the preview pane.
- Export all omits Test Code by default; a persistent Export & storage setting can explicitly enable Test Code for all Face exports.
- Adjustments defaults to All channels, moves the enable toggle into the Editing header, swaps the All channels/channel controls, and uses larger, spaced Levels/Mixer/Curve tabs.

''' + notes
notes_path.write_text(notes, encoding='utf-8')

print('v0.13.2 patch applied')
