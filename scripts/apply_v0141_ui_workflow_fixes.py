from pathlib import Path
import re

ROOT = Path('.')


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise RuntimeError(f'{label}: marker not found')
    return text.replace(old, new, 1)


# --- app_main.rs -----------------------------------------------------------
path = ROOT / 'src/app_main.rs'
text = path.read_text(encoding='utf-8')

# Export All: keep the window compact and prevent infinite-width text edits
# from forcing it to the full application width.
text = replace_once(
    text,
    '''        egui::Window::new("Export All Faces")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .show(ctx, |ui| {''',
    '''        egui::Window::new("Export All Faces")
            .open(&mut open)
            .resizable(true)
            .default_width(500.0)
            .min_width(460.0)
            .max_width(560.0)
            .show(ctx, |ui| {''',
    'export all window sizing',
)
text = replace_once(
    text,
    '''                        egui::TextEdit::singleline(&mut self.export_all_folder)
                            .desired_width(f32::INFINITY),''',
    '''                        egui::TextEdit::singleline(&mut self.export_all_folder)
                            .desired_width(360.0),''',
    'export folder field width',
)
text = replace_once(
    text,
    '''                        egui::TextEdit::singleline(&mut self.settings.export_all_template)
                            .desired_width(f32::INFINITY),''',
    '''                        egui::TextEdit::singleline(&mut self.settings.export_all_template)
                            .desired_width(455.0),''',
    'export template field width',
)

# Rename the user-facing Previous Shades terminology to Project View.
text = text.replace('"Previous shades"', '"Project View"')
text = text.replace('"Previous Shades"', '"Project View"')
text = text.replace('ui.heading("Project Browser")', 'ui.heading("Project View")')

# Compact Adjustment heading: keep modified state without crowding the title
# line, and keep the layout toggle anchored on the first row.
adjust_start = text.index('        ui.horizontal_wrapped(|ui| {\n            ui.heading("Adjustments");')
adjust_end = text.index('\n\n        let mut frame = egui::Frame::new()', adjust_start)
new_adjust_header = '''        ui.horizontal(|ui| {
            ui.heading("Adjustments");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
        });
        ui.horizontal_wrapped(|ui| {
            let mut all_channels = self.adjustment_scope == AdjustmentScope::All;
            if ui.checkbox(&mut all_channels, "All channels").changed() {
                self.adjustment_scope = if all_channels {
                    AdjustmentScope::All
                } else {
                    AdjustmentScope::Selected
                };
            }
            let selected = self.adjustment_scope == AdjustmentScope::Selected;
            let channel_button_label = if output_modified {
                format!("{output_display}  •")
            } else {
                output_display.to_owned()
            };
            let response = with_accent(ui, control_accent, |ui| {
                ui.add(egui::Button::new(channel_button_label).selected(selected))
            });
            if response.clicked() {
                self.adjustment_scope = AdjustmentScope::Selected;
            }
            if modified_count > 0 {
                ui.small(format!("Modified {modified_count}/{}", channel_names.len()));
            }
        });'''
text = text[:adjust_start] + new_adjust_header + text[adjust_end:]

# Compact/flexible Adjustment tabs. No right-to-left reset overlay, so Curve
# can never collide with Reset on a narrow tools sidebar.
tab_start = text.index('fn adjustment_tab_bar(ui: &mut egui::Ui, tool: &mut ToolPanel) -> bool {')
tab_end = text.index('\n\nfn unique_shade_path', tab_start)
new_tab_bar = '''fn adjustment_tab_bar(ui: &mut egui::Ui, tool: &mut ToolPanel) -> bool {
    ui.add_space(7.0);
    let mut reset = false;
    ui.horizontal(|ui| {
        let spacing = ui.spacing().item_spacing.x;
        let reset_width = 54.0;
        let available = ui.available_width();
        let tab_width = ((available - reset_width - spacing * 3.0) / 3.0).clamp(54.0, 76.0);
        if ui
            .add_sized(
                [tab_width, 30.0],
                egui::Button::new("Levels").selected(*tool == ToolPanel::Levels),
            )
            .clicked()
        {
            *tool = ToolPanel::Levels;
        }
        if ui
            .add_sized(
                [tab_width, 30.0],
                egui::Button::new("Mixer").selected(*tool == ToolPanel::Mixer),
            )
            .clicked()
        {
            *tool = ToolPanel::Mixer;
        }
        if ui
            .add_sized(
                [tab_width, 30.0],
                egui::Button::new("Curve").selected(*tool == ToolPanel::Curves),
            )
            .clicked()
        {
            *tool = ToolPanel::Curves;
        }
        reset = ui
            .add_sized([reset_width, 30.0], egui::Button::new("Reset"))
            .clicked();
    });
    ui.add_space(7.0);
    reset
}'''
text = text[:tab_start] + new_tab_bar + text[tab_end:]

# Operation progress: keep one larger bar, put operation + detail inside it,
# and remove the second detail line that changed toolbar height.
progress_start = text.index('    fn ui_operation_progress(&self, ui: &mut egui::Ui) {')
progress_end = text.index('\n\n    fn ui_update_compact', progress_start)
new_progress = '''    fn ui_operation_progress(&self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            if let Ok(progress) = job.progress.lock() {
                let value = progress.fraction.unwrap_or(0.5);
                let full_text = if progress.detail.trim().is_empty() {
                    progress.label.clone()
                } else {
                    format!("{} - {}", progress.label, progress.detail)
                };
                let mut compact = full_text.chars().take(64).collect::<String>();
                if full_text.chars().count() > 64 {
                    compact.push('…');
                }
                ui.add(
                    egui::ProgressBar::new(value)
                        .desired_width(380.0)
                        .text(compact)
                        .animate(progress.fraction.is_none()),
                )
                .on_hover_text(full_text);
                return;
            }
        }
        if self.render_busy.is_some() {
            ui.add(
                egui::ProgressBar::new(0.45)
                    .desired_width(300.0)
                    .text("Rendering preview")
                    .animate(true),
            );
        }
    }'''
text = text[:progress_start] + new_progress + text[progress_end:]

# Put the operation bar first in the right-to-left toolbar so it remains
# anchored to the right rather than drifting toward the left controls.
text = replace_once(
    text,
    '''            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Logs").clicked() { self.log_cache = self.log.read(); self.show_logs = true; }
                self.ui_update_compact(ui);
                self.ui_operation_progress(ui);''',
    '''            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.ui_operation_progress(ui);
                if ui.small_button("Logs").clicked() { self.log_cache = self.log.read(); self.show_logs = true; }
                self.ui_update_compact(ui);''',
    'toolbar progress order',
)

# Opening project progress should say what stage/Face is loading, not render a
# filename on a separate line.
old_open_progress = '''                    Self::set_progress(
                        &progress,
                        Some(index as f32 / total as f32),
                        "Opening project",
                        &source
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );'''
new_open_progress = '''                    Self::set_progress(
                        &progress,
                        Some(index as f32 / total as f32),
                        "Opening project",
                        &format!("Loading Face {}/{}", index + 1, total),
                    );'''
text = replace_once(text, old_open_progress, new_open_progress, 'opening project progress detail')

# Run global shortcuts even while Project View is open. History remains editor-only.
text = replace_once(
    text,
    '''        if !self.show_previous_shades {
            workflow_v0103::handle_shortcuts(self, ui.ctx());
            self.handle_history_shortcuts(ui.ctx());
        }''',
    '''        workflow_v0103::handle_shortcuts(self, ui.ctx());
        if !self.show_previous_shades {
            self.handle_history_shortcuts(ui.ctx());
        }''',
    'global shortcut dispatch',
)

# Project View: preserve the 0.14 lazy/virtualized list and row actions, but
# restore the complete 0.13.2 inspection metadata and use a resizable right
# preview pane with a 350x350 maximum image.
func_start = text.index('    fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {')
block_start = text.index('                ui.separator();\n                ui.columns(2, |columns| {', func_start)
window_close = text.index('            });\n\n        self.show_previous_shades = open;', block_start)
new_project_view = '''                ui.separator();

                let selected_path = requested_select
                    .clone()
                    .or_else(|| self.previous_shades_selected.clone());
                let cached_selected = selected_path.as_deref().and_then(|path| {
                    self.previous_shades
                        .entries()
                        .iter()
                        .find(|entry| entry.path == path)
                        .cloned()
                });

                egui::SidePanel::right("project-view-preview-pane")
                    .resizable(true)
                    .default_width(420.0)
                    .width_range(320.0..=580.0)
                    .show_inside(ui, |preview_ui| {
                        preview_ui.strong("Preview");
                        preview_ui.add_space(4.0);
                        let Some(path) = selected_path.as_deref() else {
                            preview_ui.label("Select a project to inspect its thumbnail, Snapshots and metadata.");
                            return;
                        };

                        let exists = Path::new(path).is_file();
                        preview_ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(exists, egui::Button::new("Open")).clicked() {
                                requested_open = Some(path.to_owned());
                            }
                            if ui.add_enabled(exists, egui::Button::new("Reveal in Explorer")).clicked() {
                                requested_reveal = Some(path.to_owned());
                            }
                            if !exists && ui.button("Relink missing...").clicked() {
                                requested_relink = Some(path.to_owned());
                            }
                            if ui.button("Remove from history").clicked() {
                                requested_remove = Some(path.to_owned());
                            }
                        });
                        preview_ui.separator();

                        if let Some(error) = self.previous_shade_preview_error.as_ref() {
                            preview_ui.colored_label(egui::Color32::YELLOW, error);
                            if let Some(entry) = cached_selected.as_ref() {
                                preview_ui.label(format!(
                                    "Cached: {} face(s) · {}",
                                    entry.face_count,
                                    entry.active_face_display()
                                ));
                                if let Some(snapshot) = entry.latest_snapshot() {
                                    preview_ui.label(format!(
                                        "Latest Snapshot: {} · #{}",
                                        snapshot.name, snapshot.id
                                    ));
                                }
                            }
                            preview_ui.small(path);
                            return;
                        }

                        let Some(preview) = self.previous_shade_preview.as_ref() else {
                            preview_ui.label("Loading project inspection...");
                            return;
                        };

                        preview_ui.heading(&preview.project_name);
                        if let Some(texture) = self.previous_shade_texture.as_ref() {
                            let natural = texture.size_vec2();
                            if natural.x > 0.0 && natural.y > 0.0 {
                                let max_size = egui::vec2(
                                    preview_ui.available_width().min(350.0),
                                    350.0,
                                );
                                let scale = (max_size.x / natural.x)
                                    .min(max_size.y / natural.y)
                                    .min(1.0);
                                preview_ui.add(
                                    egui::Image::from_texture(texture)
                                        .fit_to_exact_size(natural * scale),
                                );
                            }
                        } else if let Some(error) = preview.thumbnail_error.as_ref() {
                            preview_ui.small(format!("Thumbnail unavailable: {error}"));
                        } else {
                            preview_ui.small("No embedded thumbnail in this .shade file.");
                        }

                        preview_ui.add_space(6.0);
                        egui::Grid::new("project-view-preview-meta")
                            .num_columns(2)
                            .striped(true)
                            .spacing([12.0, 5.0])
                            .show(preview_ui, |ui| {
                                ui.strong("Saved");
                                ui.label(format_previous_shade_time(preview.saved_at_unix_ms));
                                ui.end_row();
                                ui.strong("File modified");
                                ui.label(
                                    preview
                                        .file_modified_unix_ms
                                        .map(format_previous_shade_time)
                                        .unwrap_or_else(|| "-".to_owned()),
                                );
                                ui.end_row();
                                ui.strong("Faces");
                                ui.label(preview.face_count.to_string());
                                ui.end_row();
                                ui.strong("Active Face");
                                ui.label(preview.active_face_index.saturating_add(1).to_string());
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

                        if let Some(face) = preview.active_face.as_ref() {
                            preview_ui.separator();
                            preview_ui.strong(format!(
                                "Face {} of {} · {}",
                                preview
                                    .active_face_index
                                    .saturating_add(1)
                                    .min(preview.face_count.max(1)),
                                preview.face_count,
                                face.label
                            ));
                            preview_ui.label(format!(
                                "{} · {} x {} px · {}-bit · {}",
                                face.source_file_name,
                                face.width,
                                face.height,
                                face.bit_depth,
                                face.color_model
                            ));
                            preview_ui.label(format!(
                                "{:.0} x {:.0} DPI · {} channels · {}",
                                face.dpi_x,
                                face.dpi_y,
                                face.channel_count,
                                format_byte_count(face.file_size_bytes)
                            ));
                            if !face.channel_names.is_empty() {
                                preview_ui.small(format!(
                                    "Channels: {}",
                                    face.channel_names.join(", ")
                                ));
                            }
                        }

                        preview_ui.separator();
                        preview_ui.strong(format!("Snapshots · {}", preview.snapshots.len()));
                        if preview.snapshots.is_empty() {
                            preview_ui.small("No saved Snapshots in this project.");
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("project-view-snapshots")
                                .max_height(180.0)
                                .show(preview_ui, |ui| {
                                    for snapshot in &preview.snapshots {
                                        let active = preview.active_snapshot_name.as_deref()
                                            == Some(snapshot.name.as_str());
                                        ui.horizontal_wrapped(|ui| {
                                            if active {
                                                ui.strong(format!("#{}", snapshot.id));
                                                ui.strong(format!("{} · active", snapshot.name));
                                            } else {
                                                ui.strong(format!("#{}", snapshot.id));
                                                ui.label(&snapshot.name);
                                            }
                                        });
                                        if !snapshot.code.trim().is_empty()
                                            && !snapshot.code.eq_ignore_ascii_case(&snapshot.name)
                                        {
                                            ui.small(format!("Code: {}", snapshot.code));
                                        }
                                    }
                                });
                        }
                        preview_ui.separator();
                        preview_ui.small(preview.path.display().to_string());
                    });

                ui.strong(format!("Projects · {}", paths.len()));
                ui.add_space(4.0);
                if paths.is_empty() {
                    ui.label("No matching .shade projects.");
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("project-view-list")
                        .auto_shrink([false, false])
                        .show_rows(ui, 72.0, indices.len(), |ui, range| {
                            for row in range {
                                let entry = self.previous_shades.entries()[indices[row]].clone();
                                self.ensure_previous_shade_list_texture(ctx, &entry);
                                let label = entry.display_name();
                                let source_bytes = if entry.total_source_bytes > 0 {
                                    format_byte_count(entry.total_source_bytes)
                                } else {
                                    "-".to_owned()
                                };
                                let metadata = format!(
                                    "{} face(s) · {} · {}",
                                    entry.face_count,
                                    source_bytes,
                                    entry.active_face_display(),
                                );
                                let latest = entry
                                    .latest_snapshot()
                                    .map(|snapshot| {
                                        let code = snapshot.code.trim();
                                        if code.is_empty() || code == snapshot.name.trim() {
                                            format!("Latest: {}", snapshot.name)
                                        } else {
                                            format!("Latest: {} · {}", snapshot.name, code)
                                        }
                                    })
                                    .unwrap_or_else(|| "No Snapshots".to_owned());
                                let detail = if entry.is_missing() {
                                    format!("{latest} · MISSING")
                                } else {
                                    latest
                                };
                                let selected = requested_select
                                    .as_deref()
                                    .or(self.previous_shades_selected.as_deref())
                                    == Some(entry.path.as_str());
                                let thumbnail = self.previous_shade_list_textures.get(&entry.path);
                                let response = previous_shade_history_row(
                                    ui,
                                    selected,
                                    &label,
                                    &metadata,
                                    &detail,
                                    thumbnail,
                                )
                                .on_hover_text(&entry.path);
                                if response.clicked() {
                                    requested_select = Some(entry.path.clone());
                                }
                                if response.double_clicked() && !entry.is_missing() {
                                    requested_open = Some(entry.path.clone());
                                }
                            }
                        });
                }
'''
text = text[:block_start] + new_project_view + text[window_close:]

path.write_text(text, encoding='utf-8')

# --- workflow_v0103.rs -----------------------------------------------------
path = ROOT / 'src/workflow_v0103.rs'
text = path.read_text(encoding='utf-8')
shortcut_start = text.index('pub(super) fn handle_shortcuts(app: &mut ShadeApp, ctx: &egui::Context) {')
shortcut_end = text.index('\n\npub(super) fn rebuild_previews', shortcut_start)
new_shortcuts = '''pub(super) fn handle_shortcuts(app: &mut ShadeApp, ctx: &egui::Context) {
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
                input.key_pressed(egui::Key::Enter)
                    && input.modifiers.ctrl
                    && !input.modifiers.alt,
            )
        });

    if new_project {
        app.show_previous_shades = false;
        app.new_project();
    }
    if save_as {
        app.save_project(true);
    } else if save {
        app.save_project(false);
    }
    if export_all {
        app.export_all_dialog();
    } else if export_face {
        app.export_current_dialog();
    }
    if update_snapshot {
        update_active_snapshot(app);
    }

    if ctx.wants_keyboard_input() {
        return;
    }
    let (settings, fit, solo, channel) = ctx.input(|input| {
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
        (
            no_modifiers && input.key_pressed(egui::Key::G),
            no_modifiers && input.key_pressed(egui::Key::F),
            no_modifiers && input.key_pressed(egui::Key::S),
            if no_modifiers {
                keys.iter().position(|key| input.key_pressed(*key))
            } else {
                None
            },
        )
    });
    if settings {
        app.show_settings = true;
    }
    if app.show_previous_shades {
        return;
    }
    if fit {
        app.fit_requested = true;
        app.viewport_recenter = true;
    }
    if let Some(channel) = channel {
        if app
            .faces
            .get(app.current_face)
            .filter(|face| face.available)
            .is_some_and(|face| channel < face.preview.metadata.channel_names.len())
        {
            app.select_channel(channel, false);
        }
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
}'''
text = text[:shortcut_start] + new_shortcuts + text[shortcut_end:]
path.write_text(text, encoding='utf-8')

# --- Project View terminology in persistence errors/docs ------------------
for rel in ['src/previous_shades.rs', 'README.md']:
    p = ROOT / rel
    text = p.read_text(encoding='utf-8')
    text = text.replace('Previous Shades', 'Project View')
    text = text.replace('Previous shades', 'Project View')
    p.write_text(text, encoding='utf-8')

# --- version / release notes ----------------------------------------------
p = ROOT / 'Cargo.toml'
text = p.read_text(encoding='utf-8')
text = replace_once(text, 'version = "0.14.0"', 'version = "0.14.1"', 'Cargo.toml version')
p.write_text(text, encoding='utf-8')

p = ROOT / 'Cargo.lock'
text = p.read_text(encoding='utf-8')
text, count = re.subn(
    r'(name = "windows-shade-editor"\nversion = ")0\.14\.0(")',
    r'\g<1>0.14.1\2',
    text,
    count=1,
)
if count != 1:
    raise RuntimeError('Cargo.lock root package version marker not found')
p.write_text(text, encoding='utf-8')

p = ROOT / 'RELEASE_NOTES.md'
text = p.read_text(encoding='utf-8')
text = text.replace('Previous Shades', 'Project View').replace('Previous shades', 'Project View')
marker = '# Shade Editor 0.14.0\n'
if marker not in text:
    raise RuntimeError('release notes marker not found')
section = '''# Shade Editor 0.14.1

- Keep Export All Faces compact at a ~500px default width and prevent its text fields from expanding the dialog to the application width.
- Rename the user-facing Previous Shades workspace to Project View and restore the complete v0.13.2 preview metadata while keeping v0.14 relink/remove/reveal/lazy-thumbnail behavior.
- Make the Project View preview pane horizontally resizable and cap its embedded thumbnail display at 350x350px.
- Reflow the Adjustments header and use compact adaptive Levels/Mixer/Curve/Reset tabs so the sidebar stays usable at narrow widths while retaining modified-state indicators.
- Add Ctrl+N, Ctrl+E, Ctrl+Shift+E and G shortcuts for New, Export Face, Export All and Settings.
- Keep operation progress in the right toolbar, widen it, and render operation + stage inside the progress bar without a second detail line.

'''
text = text.replace(marker, section + marker, 1)
p.write_text(text, encoding='utf-8')
