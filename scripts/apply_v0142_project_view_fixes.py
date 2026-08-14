from pathlib import Path
import re

ROOT = Path('.')


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise RuntimeError(f'{label}: marker not found')
    return text.replace(old, new, 1)


# ---------------------------------------------------------------------------
# src/previous_shades.rs
# ---------------------------------------------------------------------------
path = ROOT / 'src/previous_shades.rs'
text = path.read_text(encoding='utf-8')
text = replace_once(text, 'const SNAPSHOT_CACHE_VERSION: u32 = 3;', 'const SNAPSHOT_CACHE_VERSION: u32 = 4;', 'snapshot cache version')
text = replace_once(
    text,
    '''    pub active_face_index: usize,\n    pub active_face_label: String,\n    pub total_source_bytes: u64,''',
    '''    pub active_face_index: usize,\n    pub active_face_label: String,\n    pub active_face_width: u32,\n    pub active_face_height: u32,\n    pub total_source_bytes: u64,''',
    'cached active face dimensions fields',
)
text = replace_once(
    text,
    '''            active_face_index: 0,\n            active_face_label: String::new(),\n            total_source_bytes: 0,''',
    '''            active_face_index: 0,\n            active_face_label: String::new(),\n            active_face_width: 0,\n            active_face_height: 0,\n            total_source_bytes: 0,''',
    'cached active face dimensions defaults',
)

marker = '''        self.active_face_label = project\n            .file_metadata'''
start = text.index(marker)
end = text.index('        self.total_source_bytes = project', start)
insert_at = end
active_face_dimension_code = '''        let active_face_metadata = project.file_metadata.as_ref().and_then(|metadata| {\n            metadata\n                .faces\n                .get(self.active_face_index)\n                .or_else(|| metadata.faces.first())\n        });\n        self.active_face_width = active_face_metadata.map(|face| face.width).unwrap_or(0);\n        self.active_face_height = active_face_metadata.map(|face| face.height).unwrap_or(0);\n'''
text = text[:insert_at] + active_face_dimension_code + text[insert_at:]

method_marker = '''    pub fn latest_snapshot(&self) -> Option<&CachedSnapshot> {\n        self.snapshots\n            .iter()\n            .max_by_key(|snapshot| (snapshot.created_at_unix_ms, snapshot.id))\n    }\n\n'''
new_methods = method_marker + '''    pub fn recent_snapshots(&self, limit: usize) -> Vec<&CachedSnapshot> {\n        let mut snapshots = self.snapshots.iter().collect::<Vec<_>>();\n        snapshots.sort_by(|left, right| {\n            (right.created_at_unix_ms, right.id).cmp(&(left.created_at_unix_ms, left.id))\n        });\n        snapshots.truncate(limit);\n        snapshots\n    }\n\n    pub fn active_face_pixel_size(&self) -> Option<(u32, u32)> {\n        (self.active_face_width > 0 && self.active_face_height > 0)\n            .then_some((self.active_face_width, self.active_face_height))\n    }\n\n'''
text = replace_once(text, method_marker, new_methods, 'recent snapshots and pixel-size methods')

text = text.replace(
    '''            existing.active_face_label = entry.active_face_label;\n            existing.total_source_bytes = entry.total_source_bytes;''',
    '''            existing.active_face_label = entry.active_face_label;\n            existing.active_face_width = entry.active_face_width;\n            existing.active_face_height = entry.active_face_height;\n            existing.total_source_bytes = entry.total_source_bytes;''',
)
text = text.replace(
    '''                    existing.active_face_label = entry.active_face_label.clone();\n                    existing.total_source_bytes = entry.total_source_bytes;''',
    '''                    existing.active_face_label = entry.active_face_label.clone();\n                    existing.active_face_width = entry.active_face_width;\n                    existing.active_face_height = entry.active_face_height;\n                    existing.total_source_bytes = entry.total_source_bytes;''',
)
# The previous block occurs in both sanitize branches; ensure both were updated.
if text.count('existing.active_face_width = entry.active_face_width;') < 3:
    raise RuntimeError('cached dimension copy markers were not all updated')

# Add a regression test for the requested newest-first eight-Snapshot list behavior.
test_marker = '''    #[test]\n    fn untitled_history_uses_shade_filename() {'''
new_test = '''    #[test]\n    fn recent_snapshots_are_newest_first_and_limited() {\n        let mut entry = PreviousShadeEntry::default();\n        for id in 1..=10 {\n            entry.snapshots.push(CachedSnapshot {\n                id,\n                name: format!("S{id}"),\n                code: String::new(),\n                created_at_unix_ms: id as i64 * 100,\n            });\n        }\n        let recent = entry.recent_snapshots(8);\n        assert_eq!(recent.len(), 8);\n        assert_eq!(recent[0].name, "S10");\n        assert_eq!(recent[7].name, "S3");\n    }\n\n'''
text = replace_once(text, test_marker, new_test + test_marker, 'recent snapshots regression test')
path.write_text(text, encoding='utf-8')


# ---------------------------------------------------------------------------
# src/app_main.rs
# ---------------------------------------------------------------------------
path = ROOT / 'src/app_main.rs'
text = path.read_text(encoding='utf-8')

# Export All: Reveal folder is always beside Browse and is enabled only for a
# valid destination directory.
text = replace_once(
    text,
    '''                ui.horizontal(|ui| {\n                    ui.add(\n                        egui::TextEdit::singleline(&mut self.export_all_folder)\n                            .desired_width(360.0),\n                    );\n                    browse = ui.button("Browse...").clicked();\n                });\n                if existing_tiffs > 0 {\n                    ui.colored_label(\n                        egui::Color32::YELLOW,\n                        format!("Warning: this folder already contains {existing_tiffs} TIFF file(s). Mixing source/old exports can cause mistakes."),\n                    );\n                    reveal = ui.button("Reveal folder").clicked();\n                }''',
    '''                ui.horizontal(|ui| {\n                    ui.add(\n                        egui::TextEdit::singleline(&mut self.export_all_folder)\n                            .desired_width(300.0),\n                    );\n                    browse = ui.button("Browse...").clicked();\n                    reveal = ui\n                        .add_enabled(folder.is_dir(), egui::Button::new("Reveal folder"))\n                        .clicked();\n                });\n                if existing_tiffs > 0 {\n                    ui.colored_label(\n                        egui::Color32::YELLOW,\n                        format!("Warning: this folder already contains {existing_tiffs} TIFF file(s). Mixing source/old exports can cause mistakes."),\n                    );\n                }''',
    'export reveal button placement',
)

# Selected channel button: when its channel accent becomes the fill, force the
# button label to white for readable contrast.
text = replace_once(
    text,
    '''            let response = with_accent(ui, control_accent, |ui| {\n                ui.add(egui::Button::new(channel_button_label).selected(selected))\n            });''',
    '''            let channel_button_text = if selected && control_accent.is_some() {\n                egui::WidgetText::from(\n                    egui::RichText::new(channel_button_label).color(egui::Color32::WHITE),\n                )\n            } else {\n                egui::WidgetText::from(channel_button_label)\n            };\n            let response = with_accent(ui, control_accent, |ui| {\n                ui.add(egui::Button::new(channel_button_text).selected(selected))\n            });''',
    'selected channel button text contrast',
)

# Project View preview pane becomes vertically scrollable while the panel itself
# remains horizontally resizable.
text = replace_once(
    text,
    '''                    .size_range(320.0..=580.0)\n                    .show(ui, |preview_ui| {\n                        preview_ui.strong("Preview");''',
    '''                    .size_range(320.0..=580.0)\n                    .show(ui, |preview_ui| {\n                        egui::ScrollArea::vertical()\n                            .id_salt("project-view-preview-scroll")\n                            .auto_shrink([false, false])\n                            .show(preview_ui, |preview_ui| {\n                        preview_ui.strong("Preview");''',
    'preview scroll start',
)
text = replace_once(
    text,
    '''                        preview_ui.separator();\n                        preview_ui.small(preview.path.display().to_string());\n                    });\n\n                ui.strong(format!("Projects · {}", paths.len()));''',
    '''                        preview_ui.separator();\n                        preview_ui.small(preview.path.display().to_string());\n                            });\n                    });\n\n                ui.strong(format!("Projects · {}", paths.len()));''',
    'preview scroll end',
)

# The summary immediately below the image is two information columns (two
# label/value pairs per row), not one long vertical pair list.
summary_start = text.index('                        egui::Grid::new("project-view-preview-meta")')
summary_end = text.index('\n\n                        if let Some(face) = preview.active_face.as_ref() {', summary_start)
new_summary = '''                        egui::Grid::new("project-view-preview-meta")\n                            .num_columns(4)\n                            .striped(true)\n                            .spacing([12.0, 5.0])\n                            .show(preview_ui, |ui| {\n                                ui.strong("Saved");\n                                ui.label(format_previous_shade_time(preview.saved_at_unix_ms));\n                                ui.strong("File modified");\n                                ui.label(\n                                    preview\n                                        .file_modified_unix_ms\n                                        .map(format_previous_shade_time)\n                                        .unwrap_or_else(|| "-".to_owned()),\n                                );\n                                ui.end_row();\n                                ui.strong("Faces");\n                                ui.label(preview.face_count.to_string());\n                                ui.strong("Active Face");\n                                ui.label(preview.active_face_index.saturating_add(1).to_string());\n                                ui.end_row();\n                                ui.strong("Snapshots");\n                                ui.label(preview.snapshot_count.to_string());\n                                ui.strong("Active snapshot");\n                                ui.label(preview.active_snapshot_name.as_deref().unwrap_or("-"));\n                                ui.end_row();\n                                ui.strong("Test code");\n                                ui.label(if preview.test_code_enabled { "Enabled" } else { "Off" });\n                                ui.strong("Source bytes");\n                                ui.label(format_byte_count(preview.total_source_bytes));\n                                ui.end_row();\n                            });'''
text = text[:summary_start] + new_summary + text[summary_end:]

# Rebuild the TIFF details and Snapshot list as compact grids. Snapshot cells are
# arranged two per row and code is available as hover detail instead of adding
# extra vertical lines.
face_start = text.index('                        if let Some(face) = preview.active_face.as_ref() {', summary_start)
face_end = text.index('                        preview_ui.separator();\n                        preview_ui.small(preview.path.display().to_string());', face_start)
new_face_and_snapshots = '''                        if let Some(face) = preview.active_face.as_ref() {\n                            preview_ui.separator();\n                            preview_ui\n                                .strong(format!(\n                                    "TIFF details · Face {} of {}",\n                                    preview\n                                        .active_face_index\n                                        .saturating_add(1)\n                                        .min(preview.face_count.max(1)),\n                                    preview.face_count,\n                                ))\n                                .on_hover_text(&face.source_file_name);\n                            egui::Grid::new("project-view-active-face-meta")\n                                .num_columns(4)\n                                .striped(true)\n                                .spacing([12.0, 5.0])\n                                .show(preview_ui, |ui| {\n                                    ui.strong("Face");\n                                    ui.label(\n                                        preview\n                                            .active_face_index\n                                            .saturating_add(1)\n                                            .min(preview.face_count.max(1))\n                                            .to_string(),\n                                    );\n                                    ui.strong("Dimensions");\n                                    ui.label(format!("{} x {} px", face.width, face.height));\n                                    ui.end_row();\n                                    ui.strong("Bit depth");\n                                    ui.label(format!("{}-bit", face.bit_depth));\n                                    ui.strong("Color model");\n                                    ui.label(&face.color_model);\n                                    ui.end_row();\n                                    ui.strong("DPI");\n                                    ui.label(format!("{:.0} x {:.0}", face.dpi_x, face.dpi_y));\n                                    ui.strong("Channels");\n                                    ui.label(face.channel_count.to_string());\n                                    ui.end_row();\n                                    ui.strong("File size");\n                                    ui.label(format_byte_count(face.file_size_bytes));\n                                    ui.strong("Channel names");\n                                    ui.label(if face.channel_names.is_empty() {\n                                        "-".to_owned()\n                                    } else {\n                                        face.channel_names.join(", ")\n                                    });\n                                    ui.end_row();\n                                });\n                        }\n\n                        preview_ui.separator();\n                        preview_ui.strong(format!("Snapshots · {}", preview.snapshots.len()));\n                        if preview.snapshots.is_empty() {\n                            preview_ui.small("No saved Snapshots in this project.");\n                        } else {\n                            let mut snapshots = preview.snapshots.iter().collect::<Vec<_>>();\n                            snapshots.sort_by(|left, right| {\n                                (right.created_at_unix_ms, right.id)\n                                    .cmp(&(left.created_at_unix_ms, left.id))\n                            });\n                            egui::Grid::new("project-view-snapshots-grid")\n                                .num_columns(2)\n                                .striped(true)\n                                .spacing([14.0, 4.0])\n                                .show(preview_ui, |ui| {\n                                    for pair in snapshots.chunks(2) {\n                                        for snapshot in pair {\n                                            let active = preview.active_snapshot_name.as_deref()\n                                                == Some(snapshot.name.as_str());\n                                            let label = if active {\n                                                format!("#{}  {} · active", snapshot.id, snapshot.name)\n                                            } else {\n                                                format!("#{}  {}", snapshot.id, snapshot.name)\n                                            };\n                                            let response = if active {\n                                                ui.strong(label)\n                                            } else {\n                                                ui.label(label)\n                                            };\n                                            if !snapshot.code.trim().is_empty()\n                                                && !snapshot\n                                                    .code\n                                                    .eq_ignore_ascii_case(&snapshot.name)\n                                            {\n                                                response.on_hover_text(format!(\n                                                    "Code: {}", snapshot.code\n                                                ));\n                                            }\n                                        }\n                                        if pair.len() == 1 {\n                                            ui.label("");\n                                        }\n                                        ui.end_row();\n                                    }\n                                });\n                        }\n'''
text = text[:face_start] + new_face_and_snapshots + text[face_end:]

# Project rows: no active Face filename. Show Face count + total source bytes +
# active Face pixel dimensions, then up to the eight newest Snapshot names over
# two compact lines.
row_meta_start = text.index('                                let label = entry.display_name();', text.index('id_salt("project-view-list")'))
row_meta_end = text.index('                                let selected = requested_select', row_meta_start)
new_row_meta = '''                                let display_name = entry.display_name();\n                                let label = if entry.is_missing() {\n                                    format!("[missing] {display_name}")\n                                } else {\n                                    display_name\n                                };\n                                let source_bytes = if entry.total_source_bytes > 0 {\n                                    format_byte_count(entry.total_source_bytes)\n                                } else {\n                                    "-".to_owned()\n                                };\n                                let pixel_size = entry\n                                    .active_face_pixel_size()\n                                    .map(|(width, height)| format!("{width} x {height} px"))\n                                    .unwrap_or_else(|| "-".to_owned());\n                                let metadata = format!(\n                                    "{} face(s) · {} · {}",\n                                    entry.face_count, source_bytes, pixel_size,\n                                );\n                                let recent_names = entry\n                                    .recent_snapshots(8)\n                                    .into_iter()\n                                    .map(|snapshot| snapshot.name.trim())\n                                    .filter(|name| !name.is_empty())\n                                    .collect::<Vec<_>>();\n                                let snapshot_line_1 = if recent_names.is_empty() {\n                                    "No Snapshots".to_owned()\n                                } else {\n                                    format!(\n                                        "Snapshots: {}",\n                                        recent_names[..recent_names.len().min(4)].join(" · ")\n                                    )\n                                };\n                                let snapshot_line_2 = if recent_names.len() > 4 {\n                                    recent_names[4..].join(" · ")\n                                } else {\n                                    String::new()\n                                };\n'''
text = text[:row_meta_start] + new_row_meta + text[row_meta_end:]
text = replace_once(
    text,
    '''                                    &metadata,\n                                    &detail,\n                                    thumbnail,''',
    '''                                    &metadata,\n                                    &snapshot_line_1,\n                                    &snapshot_line_2,\n                                    thumbnail,''',
    'project row snapshot lines call',
)
text = replace_once(text, '.show_rows(ui, 72.0, indices.len(), |ui, range| {', '.show_rows(ui, 88.0, indices.len(), |ui, range| {', 'project row virtual height')

# Rewrite the fixed painter row to fit two Snapshot lines.
row_fn_start = text.index('fn previous_shade_history_row(')
row_fn_end = text.index('\n\nfn adjustment_tab_bar', row_fn_start)
new_row_fn = '''fn previous_shade_history_row(\n    ui: &mut egui::Ui,\n    selected: bool,\n    label: &str,\n    metadata: &str,\n    detail_primary: &str,\n    detail_secondary: &str,\n    thumbnail: Option<&egui::TextureHandle>,\n) -> egui::Response {\n    let width = ui.available_width().max(1.0);\n    let height = 84.0;\n    let (rect, response) =\n        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());\n    let visuals = ui.visuals();\n    let fill = if selected {\n        visuals.selection.bg_fill.gamma_multiply(0.72)\n    } else if response.hovered() {\n        visuals.widgets.hovered.bg_fill\n    } else {\n        egui::Color32::TRANSPARENT\n    };\n    if fill != egui::Color32::TRANSPARENT {\n        ui.painter().rect_filled(rect, 5.0, fill);\n    }\n\n    let thumb_rect = egui::Rect::from_min_size(\n        rect.left_top() + egui::vec2(7.0, 14.0),\n        egui::vec2(56.0, 56.0),\n    );\n    if let Some(texture) = thumbnail {\n        let natural = texture.size_vec2();\n        let scale = if natural.x > 0.0 && natural.y > 0.0 {\n            (thumb_rect.width() / natural.x)\n                .min(thumb_rect.height() / natural.y)\n                .min(1.0)\n        } else {\n            1.0\n        };\n        let image_rect = egui::Rect::from_center_size(thumb_rect.center(), natural * scale);\n        ui.painter()\n            .rect_filled(thumb_rect, 4.0, ui.visuals().extreme_bg_color);\n        ui.painter().image(\n            texture.id(),\n            image_rect,\n            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),\n            egui::Color32::WHITE,\n        );\n    } else {\n        ui.painter().rect_stroke(\n            thumb_rect,\n            4.0,\n            visuals.widgets.noninteractive.bg_stroke,\n            egui::StrokeKind::Inside,\n        );\n        ui.painter().text(\n            thumb_rect.center(),\n            egui::Align2::CENTER_CENTER,\n            "—",\n            egui::FontId::proportional(16.0),\n            visuals.weak_text_color(),\n        );\n    }\n\n    let text_left = thumb_rect.right() + 9.0;\n    ui.painter().text(\n        egui::pos2(text_left, rect.top() + 14.0),\n        egui::Align2::LEFT_CENTER,\n        label,\n        egui::FontId::proportional(14.5),\n        if selected {\n            visuals.selection.stroke.color\n        } else {\n            visuals.text_color()\n        },\n    );\n    ui.painter().text(\n        egui::pos2(text_left, rect.top() + 34.0),\n        egui::Align2::LEFT_CENTER,\n        metadata,\n        egui::FontId::proportional(12.0),\n        visuals.weak_text_color(),\n    );\n    ui.painter().text(\n        egui::pos2(text_left, rect.top() + 54.0),\n        egui::Align2::LEFT_CENTER,\n        detail_primary,\n        egui::FontId::proportional(11.5),\n        visuals.weak_text_color(),\n    );\n    if !detail_secondary.is_empty() {\n        ui.painter().text(\n            egui::pos2(text_left, rect.top() + 72.0),\n            egui::Align2::LEFT_CENTER,\n            detail_secondary,\n            egui::FontId::proportional(11.5),\n            visuals.weak_text_color(),\n        );\n    }\n    response\n}'''
text = text[:row_fn_start] + new_row_fn + text[row_fn_end:]

# Track Curve graph focus in egui temp data. The app clears this flag once per
# frame after shortcut dispatch; any focused Curve graph sets it back for the
# following frame. This lets channel digits bypass graph keyboard focus while
# ordinary TextEdit / DragValue focus remains protected.
text = replace_once(
    text,
    '''    (changed, selected)\n}\n\nfn curve_point_fields''',
    '''    if graph_response.has_focus() {\n        ui.ctx().data_mut(|data| {\n            data.insert_temp(\n                egui::Id::new("shade-editor-curve-graph-focused"),\n                true,\n            );\n        });\n    }\n    (changed, selected)\n}\n\nfn curve_point_fields''',
    'curve graph focus marker',
)
text = replace_once(
    text,
    '''        if !self.show_previous_shades {\n            self.handle_history_shortcuts(ui.ctx());\n        }\n        self.maybe_autosave();''',
    '''        if !self.show_previous_shades {\n            self.handle_history_shortcuts(ui.ctx());\n        }\n        ui.ctx().data_mut(|data| {\n            data.insert_temp(\n                egui::Id::new("shade-editor-curve-graph-focused"),\n                false,\n            );\n        });\n        self.maybe_autosave();''',
    'curve graph focus reset',
)

# About window: keep the displayed shortcut reference in sync.
text = replace_once(
    text,
    '''                        ui.strong("File");\n                        ui.label("Ctrl+S  Save   |   Ctrl+Shift+S  Save As");\n                        ui.end_row();\n                        ui.strong("View");\n                        ui.label("F  Fit image");''',
    '''                        ui.strong("File");\n                        ui.label("Ctrl+N  New   |   Ctrl+S  Save   |   Ctrl+Shift+S  Save As");\n                        ui.end_row();\n                        ui.strong("Export");\n                        ui.label("Ctrl+E  Export Face   |   Ctrl+Shift+E  Export All");\n                        ui.end_row();\n                        ui.strong("View / Settings");\n                        ui.label("F  Fit image   |   G  Settings");''',
    'about shortcuts',
)
path.write_text(text, encoding='utf-8')


# ---------------------------------------------------------------------------
# src/workflow_v0103.rs
# ---------------------------------------------------------------------------
path = ROOT / 'src/workflow_v0103.rs'
text = path.read_text(encoding='utf-8')
old_guard = '''    if ctx.wants_keyboard_input() {\n        return;\n    }\n    let (settings, fit, solo, channel) = ctx.input(|input| {\n        let no_modifiers = !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.shift;\n        let keys = [\n            egui::Key::Num1,\n            egui::Key::Num2,\n            egui::Key::Num3,\n            egui::Key::Num4,\n            egui::Key::Num5,\n            egui::Key::Num6,\n            egui::Key::Num7,\n            egui::Key::Num8,\n            egui::Key::Num9,\n        ];\n        (\n            no_modifiers && input.key_pressed(egui::Key::G),\n            no_modifiers && input.key_pressed(egui::Key::F),\n            no_modifiers && input.key_pressed(egui::Key::S),\n            if no_modifiers {\n                keys.iter().position(|key| input.key_pressed(*key))\n            } else {\n                None\n            },\n        )\n    });'''
new_guard = '''    let channel = ctx.input(|input| {\n        let no_modifiers = !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.shift;\n        let keys = [\n            egui::Key::Num1,\n            egui::Key::Num2,\n            egui::Key::Num3,\n            egui::Key::Num4,\n            egui::Key::Num5,\n            egui::Key::Num6,\n            egui::Key::Num7,\n            egui::Key::Num8,\n            egui::Key::Num9,\n        ];\n        no_modifiers\n            .then(|| keys.iter().position(|key| input.key_pressed(*key)))\n            .flatten()\n    });\n    let curve_graph_focused = ctx.data(|data| {\n        data.get_temp::<bool>(egui::Id::new("shade-editor-curve-graph-focused"))\n            .unwrap_or(false)\n    });\n    if ctx.wants_keyboard_input() {\n        if curve_graph_focused {\n            if let Some(channel) = channel {\n                select_channel_shortcut(app, channel);\n            }\n        }\n        return;\n    }\n    let (settings, fit, solo) = ctx.input(|input| {\n        let no_modifiers = !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.shift;\n        (\n            no_modifiers && input.key_pressed(egui::Key::G),\n            no_modifiers && input.key_pressed(egui::Key::F),\n            no_modifiers && input.key_pressed(egui::Key::S),\n        )\n    });'''
text = replace_once(text, old_guard, new_guard, 'curve-safe channel shortcut guard')
old_channel_apply = '''    if let Some(channel) = channel {\n        if app\n            .faces\n            .get(app.current_face)\n            .filter(|face| face.available)\n            .is_some_and(|face| channel < face.preview.metadata.channel_names.len())\n        {\n            app.select_channel(channel, false);\n        }\n    }'''
text = replace_once(text, old_channel_apply, '''    if let Some(channel) = channel {\n        select_channel_shortcut(app, channel);\n    }''', 'channel shortcut apply helper')
insert_marker = '''pub(super) fn rebuild_previews(app: &mut ShadeApp) {'''
helper = '''fn select_channel_shortcut(app: &mut ShadeApp, channel: usize) {\n    if app\n        .faces\n        .get(app.current_face)\n        .filter(|face| face.available)\n        .is_some_and(|face| channel < face.preview.metadata.channel_names.len())\n    {\n        app.select_channel(channel, false);\n    }\n}\n\n'''
text = replace_once(text, insert_marker, helper + insert_marker, 'channel shortcut helper')
path.write_text(text, encoding='utf-8')


# ---------------------------------------------------------------------------
# Version / docs
# ---------------------------------------------------------------------------
path = ROOT / 'Cargo.toml'
text = path.read_text(encoding='utf-8')
text = replace_once(text, 'version = "0.14.1"', 'version = "0.14.2"', 'Cargo.toml version')
path.write_text(text, encoding='utf-8')

path = ROOT / 'Cargo.lock'
text = path.read_text(encoding='utf-8')
pattern = r'(\[\[package\]\]\nname = "windows-shade-editor"\nversion = ")0\.14\.1(")'
text, count = re.subn(pattern, r'\g<1>0.14.2\2', text, count=1)
if count != 1:
    raise RuntimeError('Cargo.lock package version marker not found')
path.write_text(text, encoding='utf-8')

path = ROOT / 'RELEASE_NOTES.md'
text = path.read_text(encoding='utf-8')
notes = '''# Shade Editor 0.14.2\n\n- Make the Project View Preview pane vertically scrollable while preserving its resizable width and 350px thumbnail cap.\n- Project rows show Face count, total source bytes, active-Face pixel dimensions, and the eight newest Snapshot names without repeating the Face filename.\n- Reformat Project View project/TIFF metadata into compact two-pair grids and display Snapshots two per row.\n- Force white text on the selected accent-filled adjustment channel button.\n- Keep 1-9 channel shortcuts active while the Curve graph owns keyboard focus without stealing digits from text/numeric editors.\n- Move Export All Reveal folder beside Browse.\n\n'''
path.write_text(notes + text, encoding='utf-8')

path = ROOT / 'README.md'
text = path.read_text(encoding='utf-8')
text = replace_once(
    text,
    '''## Releases and automatic updates\n\nGitHub Actions builds Windows artifacts on pull requests and `main`. Pushing a version tag such as `v0.1.0` also publishes a GitHub Release containing `ShadeEditor.exe`.''',
    '''## Builds and automatic updates\n\nGitHub Actions builds Windows artifacts for validation and direct retrieval from workflow runs. The repository does not publish build artifacts as public GitHub Releases. The in-app updater remains compatible with a separately managed release source if one is configured in the future.''',
    'README public release policy',
)
path.write_text(text, encoding='utf-8')

print('v0.14.2 migration applied')
