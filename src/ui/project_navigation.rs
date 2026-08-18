use super::actions::{NavigationUiAction, ProjectViewUiAction};
use crate::*;
use eframe::egui;

impl ShadeApp {
    pub(crate) fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut actions = Vec::new();
        let mut dismiss_error = false;
        let mut inspect_requested = false;
        let mut queue_requested = false;
        let recent_projects = self
            .previous_shades
            .recent(8)
            .into_iter()
            .map(|entry| (entry.display_name(), entry.path.clone(), entry.is_missing()))
            .collect::<Vec<_>>();
        let mut recent_requested: Option<PathBuf> = None;
        ui.horizontal(|ui| {
            ui.horizontal_wrapped(|ui| {
                let enabled = self.job.is_none();
                ui.menu_button("File", |ui| {
                    if ui.add_enabled(enabled, egui::Button::new("New project")).clicked() {
                        actions.push(NavigationUiAction::NewProject);
                    }
                    if ui.add_enabled(enabled, egui::Button::new("Open .shade...")).clicked() {
                        actions.push(NavigationUiAction::OpenProjectDialog);
                    }
                    ui.menu_button("Recent projects", |ui| {
                        if recent_projects.is_empty() {
                            ui.label("No recent projects");
                        } else {
                            for (name, path, missing) in &recent_projects {
                                let label = if *missing {
                                    format!("{name}  [missing]")
                                } else {
                                    name.clone()
                                };
                                if ui
                                    .add_enabled(enabled && !*missing, egui::Button::new(label))
                                    .on_hover_text(path)
                                    .clicked()
                                {
                                    recent_requested = Some(PathBuf::from(path));
                                    ui.close();
                                }
                            }
                        }
                    });
                    ui.separator();
                    if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save")).clicked() {
                        actions.push(NavigationUiAction::Save);
                    }
                    if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save As...")).clicked() {
                        actions.push(NavigationUiAction::SaveAs);
                    }
                    ui.separator();
                    if ui.button(app_features::TIFF_INSPECTOR_LABEL).clicked() {
                        inspect_requested = true;
                    }
                    if ui.button(app_features::EXPORT_QUEUE_LABEL).clicked() {
                        queue_requested = true;
                    }
                });
                if ui.add_enabled(enabled, egui::Button::new("New")).clicked() { actions.push(NavigationUiAction::NewProject); }
                if ui.add_enabled(enabled, egui::Button::new("Open .shade")).clicked() { actions.push(NavigationUiAction::OpenProjectDialog); }
                if ui.button("Project View").clicked() { actions.push(NavigationUiAction::ShowProjectView); }
                ui.separator();
                if self.project_path.is_none() && ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Quick Save")).on_hover_text("Create the first .shade project beside the source TIFF files without opening a Save dialog").clicked() { actions.push(NavigationUiAction::QuickSave); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save")).clicked() { actions.push(NavigationUiAction::Save); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save As")).clicked() { actions.push(NavigationUiAction::SaveAs); }
                ui.separator();
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Export face")).clicked() { actions.push(NavigationUiAction::ExportCurrent); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Export all")).clicked() { actions.push(NavigationUiAction::ExportAll); }
                let queue_pending = self.export.queue.pending_count();
                let queue_recovered = self.export.queue.recovered_waiting_count();
                let queue_label = self.export.queue.compact_status().unwrap_or_else(|| {
                    if queue_recovered > 0 {
                        format!("Queue ({queue_pending} + {queue_recovered} recovered)")
                    } else {
                        format!("Queue ({queue_pending})")
                    }
                });
                if ui.button(queue_label).clicked() { actions.push(NavigationUiAction::ShowExportQueue); }
                if ui.add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Validate face")).on_hover_text("Run a no-adjustment export through the production TIFF backend, re-decode it, and compare pixels plus critical Photoshop/TIFF metadata.").clicked() { actions.push(NavigationUiAction::ValidateCurrent); }
                ui.separator();
                if ui.button("Settings").clicked() { actions.push(NavigationUiAction::ShowSettings); }
                if ui.button("About").clicked() { actions.push(NavigationUiAction::ShowAbout); }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (save_state, _) = self.project_save_state_label();
                if !save_state.is_empty() {
                    ui.label(egui::RichText::new(save_state).small());
                }
                self.ui_operation_progress(ui);
                if ui.small_button("Logs").clicked() { actions.push(NavigationUiAction::ShowLogs); }
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
            actions.push(NavigationUiAction::InspectTiff);
        }
        if queue_requested {
            actions.push(NavigationUiAction::ShowExportQueue);
        }
        if let Some(path) = recent_requested {
            actions.push(NavigationUiAction::OpenRecent(path));
        }
        if dismiss_error {
            actions.push(NavigationUiAction::DismissError);
        }
        for action in actions {
            self.dispatch_navigation_ui_action(action, ui.ctx());
        }
    }

    pub(crate) fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {
        if !self.project_view.open {
            return;
        }
        let mut actions = Vec::new();
        let mut open = self.project_view.open;
        let query_before = self.project_view.query.clone();
        let mut requested_open: Option<String> = None;
        let mut requested_select: Option<String> = None;
        let mut requested_reveal: Option<String> = None;
        let mut requested_remove: Option<String> = None;
        let mut requested_relink: Option<String> = None;

        egui::Window::new("Project View")
            .open(&mut open)
            .default_width(1040.0)
            .default_height(680.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Project View");
                    ui.separator();
                    ui.label(format!("{} project(s)", self.previous_shades.entries().len()));
                });
                ui.horizontal(|ui| {
                    ui.label("Search");
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.project_view.query)
                            .hint_text("Project, path, Snapshot name / ID / Test Code")
                            .desired_width(390.0),
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
                            self.project_view.query.push_str(&typed);
                            search.request_focus();
                        }
                    }
                    ui.label("Sort");
                    egui::ComboBox::from_id_salt("previous-shades-sort")
                        .selected_text(self.project_view.sort.label())
                        .show_ui(ui, |ui| {
                            for sort in [
                                previous_shades::PreviousShadesSort::LastOpened,
                                previous_shades::PreviousShadesSort::ProjectName,
                                previous_shades::PreviousShadesSort::SavedAt,
                                previous_shades::PreviousShadesSort::Path,
                            ] {
                                ui.selectable_value(&mut self.project_view.sort, sort, sort.label());
                            }
                        });
                });

                let query = self.project_view.query.trim().to_lowercase();
                let entries = self.previous_shades.entries();
                let mut indices = entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.matches_query(&query))
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                match self.project_view.sort {
                    previous_shades::PreviousShadesSort::LastOpened => indices.sort_by(|a, b| {
                        entries[*b].last_opened_unix_ms.cmp(&entries[*a].last_opened_unix_ms)
                    }),
                    previous_shades::PreviousShadesSort::ProjectName => indices.sort_by(|a, b| {
                        entries[*a].display_name().to_lowercase().cmp(&entries[*b].display_name().to_lowercase())
                    }),
                    previous_shades::PreviousShadesSort::SavedAt => indices.sort_by(|a, b| {
                        entries[*b].saved_at_unix_ms.cmp(&entries[*a].saved_at_unix_ms)
                    }),
                    previous_shades::PreviousShadesSort::Path => indices.sort_by(|a, b| {
                        entries[*a].path.to_lowercase().cmp(&entries[*b].path.to_lowercase())
                    }),
                }
                let paths = indices
                    .iter()
                    .map(|index| entries[*index].path.clone())
                    .collect::<Vec<_>>();

                if query_before != self.project_view.query {
                    requested_select = paths.first().cloned();
                }
                let current_path = requested_select
                    .as_deref()
                    .or(self.project_view.selected.as_deref());
                let current_position = current_path.and_then(|path| paths.iter().position(|item| item == path));
                let (up, down, enter) = ctx.input(|input| {
                    (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                        input.key_pressed(egui::Key::Enter),
                    )
                });
                if !paths.is_empty() && (up || down) {
                    let next = match (current_position, up, down) {
                        (Some(position), true, _) => position.saturating_sub(1),
                        (Some(position), _, true) => (position + 1).min(paths.len() - 1),
                        (None, _, true) => 0,
                        (None, true, _) => paths.len() - 1,
                        _ => 0,
                    };
                    requested_select = paths.get(next).cloned();
                }
                if enter {
                    requested_open = requested_select
                        .clone()
                        .or_else(|| self.project_view.selected.clone())
                        .filter(|path| Path::new(path).is_file());
                }

                ui.separator();

                let selected_path = requested_select
                    .clone()
                    .or_else(|| self.project_view.selected.clone());
                let cached_selected = selected_path.as_deref().and_then(|path| {
                    self.previous_shades
                        .entries()
                        .iter()
                        .find(|entry| entry.path == path)
                        .cloned()
                });

                egui::Panel::right("project-view-preview-pane")
                    .resizable(true)
                    .default_size(420.0)
                    .size_range(320.0..=580.0)
                    .show(ui, |preview_ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("project-view-preview-scroll")
                            .auto_shrink([false, false])
                            .show(preview_ui, |preview_ui| {
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

                        if let Some(error) = self.project_view.preview_error.as_ref() {
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

                        let Some(preview) = self.project_view.preview.as_ref() else {
                            preview_ui.label("Loading project inspection...");
                            return;
                        };

                        preview_ui.heading(&preview.project_name);
                        if let Some(texture) = self.project_view.texture.as_ref() {
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
                            .num_columns(4)
                            .striped(true)
                            .spacing([12.0, 5.0])
                            .show(preview_ui, |ui| {
                                ui.strong("Saved");
                                ui.label(format_previous_shade_time(preview.saved_at_unix_ms));
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
                                ui.strong("Active Face");
                                ui.label(preview.active_face_index.saturating_add(1).to_string());
                                ui.end_row();
                                ui.strong("Snapshots");
                                ui.label(preview.snapshot_count.to_string());
                                ui.strong("Active snapshot");
                                ui.label(preview.active_snapshot_name.as_deref().unwrap_or("-"));
                                ui.end_row();
                                ui.strong("Test code");
                                ui.label(if preview.test_code_enabled { "Enabled" } else { "Off" });
                                ui.strong("Source bytes");
                                ui.label(format_byte_count(preview.total_source_bytes));
                                ui.end_row();
                            });

                        if let Some(face) = preview.active_face.as_ref() {
                            preview_ui.separator();
                            preview_ui
                                .strong(format!(
                                    "TIFF details · Face {} of {}",
                                    preview
                                        .active_face_index
                                        .saturating_add(1)
                                        .min(preview.face_count.max(1)),
                                    preview.face_count,
                                ))
                                .on_hover_text(&face.source_file_name);
                            egui::Grid::new("project-view-active-face-meta")
                                .num_columns(4)
                                .striped(true)
                                .spacing([12.0, 5.0])
                                .show(preview_ui, |ui| {
                                    ui.strong("Face");
                                    ui.label(
                                        preview
                                            .active_face_index
                                            .saturating_add(1)
                                            .min(preview.face_count.max(1))
                                            .to_string(),
                                    );
                                    ui.strong("Dimensions");
                                    ui.label(format!("{} x {} px", face.width, face.height));
                                    ui.end_row();
                                    ui.strong("Bit depth");
                                    ui.label(format!("{}-bit", face.bit_depth));
                                    ui.strong("Color model");
                                    ui.label(&face.color_model);
                                    ui.end_row();
                                    ui.strong("DPI");
                                    ui.label(format!("{:.0} x {:.0}", face.dpi_x, face.dpi_y));
                                    ui.strong("Channels");
                                    ui.label(face.channel_count.to_string());
                                    ui.end_row();
                                    ui.strong("File size");
                                    ui.label(format_byte_count(face.file_size_bytes));
                                    ui.strong("Channel names");
                                    ui.label(if face.channel_names.is_empty() {
                                        "-".to_owned()
                                    } else {
                                        face.channel_names.join(", ")
                                    });
                                    ui.end_row();
                                });
                        }

                        preview_ui.separator();
                        preview_ui.strong(format!("Snapshots · {}", preview.snapshots.len()));
                        if preview.snapshots.is_empty() {
                            preview_ui.small("No saved Snapshots in this project.");
                        } else {
                            let mut snapshots = preview.snapshots.iter().collect::<Vec<_>>();
                            snapshots.sort_by(|left, right| {
                                (right.created_at_unix_ms, right.id)
                                    .cmp(&(left.created_at_unix_ms, left.id))
                            });
                            egui::Grid::new("project-view-snapshots-grid")
                                .num_columns(2)
                                .striped(true)
                                .spacing([14.0, 4.0])
                                .show(preview_ui, |ui| {
                                    for pair in snapshots.chunks(2) {
                                        for snapshot in pair {
                                            let active = preview.active_snapshot_name.as_deref()
                                                == Some(snapshot.name.as_str());
                                            let label = if active {
                                                format!("#{}  {} · active", snapshot.id, snapshot.name)
                                            } else {
                                                format!("#{}  {}", snapshot.id, snapshot.name)
                                            };
                                            let response = if active {
                                                ui.strong(label)
                                            } else {
                                                ui.label(label)
                                            };
                                            if !snapshot.code.trim().is_empty()
                                                && !snapshot
                                                    .code
                                                    .eq_ignore_ascii_case(&snapshot.name)
                                            {
                                                response.on_hover_text(format!(
                                                    "Code: {}", snapshot.code
                                                ));
                                            }
                                        }
                                        if pair.len() == 1 {
                                            ui.label("");
                                        }
                                        ui.end_row();
                                    }
                                });
                        }
                        preview_ui.separator();
                        preview_ui.small(preview.path.display().to_string());
                            });
                    });

                ui.strong(format!("Projects · {}", paths.len()));
                ui.add_space(4.0);
                if paths.is_empty() {
                    ui.label("No matching .shade projects.");
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("project-view-list")
                        .auto_shrink([false, false])
                        .show_rows(ui, 88.0, indices.len(), |ui, range| {
                            for row in range {
                                let entry = self.previous_shades.entries()[indices[row]].clone();
                                self.ensure_previous_shade_list_texture(ctx, &entry);
                                let display_name = entry.display_name();
                                let label = if entry.is_missing() {
                                    format!("[missing] {display_name}")
                                } else {
                                    display_name
                                };
                                let source_bytes = if entry.total_source_bytes > 0 {
                                    format_byte_count(entry.total_source_bytes)
                                } else {
                                    "-".to_owned()
                                };
                                let pixel_size = entry
                                    .active_face_pixel_size()
                                    .map(|(width, height)| format!("{width} x {height} px"))
                                    .unwrap_or_else(|| "-".to_owned());
                                let metadata = format!(
                                    "{} face(s) · {} · {}",
                                    entry.face_count, source_bytes, pixel_size,
                                );
                                let recent_names = entry
                                    .recent_snapshots(8)
                                    .into_iter()
                                    .map(|snapshot| snapshot.name.trim())
                                    .filter(|name| !name.is_empty())
                                    .collect::<Vec<_>>();
                                let snapshot_line_1 = if recent_names.is_empty() {
                                    "No Snapshots".to_owned()
                                } else {
                                    format!(
                                        "Snapshots: {}",
                                        recent_names[..recent_names.len().min(4)].join(" · ")
                                    )
                                };
                                let snapshot_line_2 = if recent_names.len() > 4 {
                                    recent_names[4..].join(" · ")
                                } else {
                                    String::new()
                                };
                                let selected = requested_select
                                    .as_deref()
                                    .or(self.project_view.selected.as_deref())
                                    == Some(entry.path.as_str());
                                let thumbnail = self.project_view.list_textures.get(&entry.path);
                                let response = previous_shade_history_row(
                                    ui,
                                    selected,
                                    &label,
                                    &metadata,
                                    &snapshot_line_1,
                                    &snapshot_line_2,
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
            });

        actions.push(ProjectViewUiAction::SetOpen(open));
        if let Some(path) = requested_select {
            actions.push(ProjectViewUiAction::Select(path));
        }
        if let Some(path) = requested_reveal {
            actions.push(ProjectViewUiAction::Reveal(path));
        }
        if let Some(path) = requested_relink {
            actions.push(ProjectViewUiAction::Relink(path));
        }
        if let Some(path) = requested_remove {
            actions.push(ProjectViewUiAction::Remove(path));
        }
        if let Some(path) = requested_open {
            actions.push(ProjectViewUiAction::Open(path));
        }
        for action in actions {
            self.dispatch_project_view_ui_action(action, ctx);
        }
    }
}
