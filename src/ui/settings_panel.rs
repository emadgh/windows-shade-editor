use crate::*;
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsCategory {
    General,
    Interface,
    WorkspacePreview,
    EditingTools,
    History,
    FileHandling,
    Export,
    Performance,
}

impl SettingsCategory {
    const ALL: [Self; 8] = [
        Self::General,
        Self::Interface,
        Self::WorkspacePreview,
        Self::EditingTools,
        Self::History,
        Self::FileHandling,
        Self::Export,
        Self::Performance,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Interface => "Interface",
            Self::WorkspacePreview => "Workspace / Preview",
            Self::EditingTools => "Editing / Tools",
            Self::History => "History",
            Self::FileHandling => "File Handling",
            Self::Export => "Export",
            Self::Performance => "Performance",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SettingsViewState {
    pub(crate) category: SettingsCategory,
}

impl Default for SettingsViewState {
    fn default() -> Self {
        Self {
            category: SettingsCategory::General,
        }
    }
}

#[derive(Default)]
struct Effects {
    changed: bool,
    dark_changed: bool,
    history_limit_changed: bool,
    rebuild_previews: bool,
}

impl ShadeApp {
    pub(crate) fn ui_preferences_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = self.show_settings;
        let category = self.settings_view.category;
        let mut effects = Effects::default();
        egui::Window::new("Preferences")
            .open(&mut open)
            .resizable(true)
            .default_size([900.0, 620.0])
            .min_width(700.0)
            .min_height(500.0)
            .show(ctx, |ui| {
                let height = ui.available_height().max(440.0);
                let sidebar_width = 174.0;
                let gap = ui.spacing().item_spacing.x + 8.0;
                let content_width = (ui.available_width() - sidebar_width - gap).max(420.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(content_width, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("preferences-category-content")
                                .auto_shrink([false, false])
                                .show(ui, |ui| match category {
                                    SettingsCategory::General => {
                                        settings_general(self, ui, &mut effects)
                                    }
                                    SettingsCategory::Interface => {
                                        settings_interface(self, ui, &mut effects)
                                    }
                                    SettingsCategory::WorkspacePreview => {
                                        settings_workspace_preview(self, ui, &mut effects)
                                    }
                                    SettingsCategory::EditingTools => {
                                        settings_editing_tools(self, ui, &mut effects)
                                    }
                                    SettingsCategory::History => {
                                        settings_history(self, ui, &mut effects)
                                    }
                                    SettingsCategory::FileHandling => {
                                        settings_file_handling(self, ui, &mut effects)
                                    }
                                    SettingsCategory::Export => {
                                        settings_export(self, ui, &mut effects)
                                    }
                                    SettingsCategory::Performance => {
                                        settings_performance(self, ui, &mut effects)
                                    }
                                });
                        },
                    );
                    ui.separator();
                    ui.allocate_ui_with_layout(
                        egui::vec2(sidebar_width, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.heading("Preferences");
                            ui.add_space(5.0);
                            for item in SettingsCategory::ALL {
                                if ui
                                    .selectable_label(
                                        self.settings_view.category == item,
                                        item.label(),
                                    )
                                    .clicked()
                                {
                                    self.settings_view.category = item;
                                }
                            }
                        },
                    );
                });
            });
        self.show_settings = open;

        if effects.history_limit_changed {
            self.settings.sanitize();
            self.history.set_limit(self.settings.history_steps);
            if let Some((_, backup)) = self.history_clear_backup.as_mut() {
                backup.set_limit(self.settings.history_steps);
            }
        }
        if effects.dark_changed {
            apply_theme(ctx, self.settings.dark_mode);
        }
        if effects.changed {
            self.settings.sanitize();
            if let Err(err) = self.settings.save() {
                self.report_error(err);
            }
        }
        if effects.rebuild_previews {
            self.rebuild_previews();
        }
    }
}

fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.heading(title);
    ui.separator();
    ui.add_space(4.0);
}

fn settings_general(app: &mut ShadeApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(ui, "General");
    effects.changed |= ui
        .checkbox(
            &mut app.settings.auto_update,
            "Automatically check and download updates",
        )
        .changed();
    let dark_changed = ui
        .checkbox(&mut app.settings.dark_mode, "Dark mode")
        .changed();
    effects.dark_changed |= dark_changed;
    effects.changed |= dark_changed;
}

fn settings_interface(app: &mut ShadeApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(ui, "Interface");
    effects.changed |= ui
        .checkbox(
            &mut app.settings.sidebar_two_columns,
            "Use two-column tools sidebar",
        )
        .changed();
    effects.changed |= ui
        .checkbox(
            &mut app.settings.adjustment_tabs,
            "Use tabs for Levels / Mixer / Curve",
        )
        .changed();
    effects.changed |= ui
        .checkbox(
            &mut app.settings.compact_curve_controls,
            "Compact Curve editor",
        )
        .changed();
}

fn settings_workspace_preview(app: &mut ShadeApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(ui, "Workspace / Preview");
    effects.changed |= ui
        .checkbox(
            &mut app.settings.show_all_histograms,
            "Show a histogram for every channel",
        )
        .changed();
    effects.changed |= ui
        .checkbox(
            &mut app.settings.show_clipping_warnings,
            "Show per-channel clipping warnings",
        )
        .changed();
    ui.horizontal(|ui| {
        ui.label("Curve / Histogram direction");
        effects.changed |= tonal_display_mode_selector(ui, &mut app.settings.tonal_display_mode);
    });
    effects.changed |= ui
        .checkbox(
            &mut app.settings.colorize_histograms,
            "Colorize histograms by channel",
        )
        .changed();
    effects.changed |= ui
        .checkbox(
            &mut app.settings.colorize_adjustments,
            "Colorize Levels / Mixer / Curve by channel",
        )
        .changed();
    effects.changed |= ui
        .checkbox(
            &mut app.settings.show_curve_histogram,
            "Show active histogram behind Curve",
        )
        .changed();
}

fn settings_history(app: &mut ShadeApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(ui, "History");
    ui.label("History Steps");
    let changed = ui
        .add(
            egui::Slider::new(
                &mut app.settings.history_steps,
                model::MIN_HISTORY_STEPS..=model::MAX_SNAPSHOT_HISTORY_STATES,
            )
            .integer(),
        )
        .changed();
    effects.changed |= changed;
    effects.history_limit_changed |= changed;
    ui.small(format!(
        "Current stack: {} states · configured limit: {}",
        app.history.len(),
        app.settings.history_steps
    ));
    ui.small("Reducing the limit trims older history while preserving the currently selected state. New and loaded projects use this limit automatically.");
}

fn settings_performance(app: &mut ShadeApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(ui, "Performance");
    effects.changed |= ui
        .add(
            egui::Slider::new(&mut app.settings.max_preview_dimension, 600..=4000)
                .text("Preview max dimension"),
        )
        .changed();
    if ui
        .add_enabled(
            !app.faces.is_empty() && app.job.is_none(),
            egui::Button::new("Rebuild previews"),
        )
        .clicked()
    {
        effects.rebuild_previews = true;
    }
    ui.small("Preview dimension affects loaded viewport data only; export remains full-resolution and sample preserving.");
}

fn settings_export(app: &mut ShadeApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(ui, "Export");
    effects.changed |= ui
        .checkbox(
            &mut app.settings.lzw_compression,
            "Use LZW compression for exported TIFF files",
        )
        .changed();
    effects.changed |= ui
        .checkbox(
            &mut app.settings.validate_after_export,
            "Validate TIFF after normal Export face / Export all",
        )
        .changed();
    effects.changed |= ui
        .checkbox(
            &mut app.settings.export_all_test_code,
            "Write Test Code during Export all",
        )
        .changed();
    ui.add_space(8.0);
    ui.strong("Snapshot / Test export filename template");
    effects.changed |= ui
        .add(
            egui::TextEdit::singleline(&mut app.settings.snapshot_export_template)
                .desired_width(f32::INFINITY),
        )
        .changed();
    ui.small("Tokens: {project}, {face}, {snapshot}, {testcode}, {source}, {date}.");
}

fn settings_file_handling(app: &mut ShadeApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(ui, "File Handling");
    let old_default_dpi = app.settings.default_dpi;
    effects.changed |= ui
        .add(
            egui::Slider::new(&mut app.settings.default_dpi, 72.0..=1200.0)
                .text("Default DPI")
                .suffix(" dpi"),
        )
        .changed();
    if (old_default_dpi - app.settings.default_dpi).abs() > f64::EPSILON {
        for face in &mut app.faces {
            if face.dpi.used_default {
                face.dpi = dpi::DpiInfo::with_default(app.settings.default_dpi);
            }
        }
    }
    ui.add_space(12.0);
    ui.strong("Windows Explorer integration");
    let shell_installer = ShadeApp::bundled_shell_script("Install-ShadeEditorShell.ps1");
    let shell_uninstaller = ShadeApp::bundled_shell_script("Uninstall-ShadeEditorShell.ps1");
    if let Some(installer) = shell_installer {
        ui.small(format!(
            "Bundled Shell package: {}",
            installer
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .display()
        ));
        ui.horizontal(|ui| {
            if ui.button("Install Shell integration").clicked() {
                app.launch_shell_script("Install-ShadeEditorShell.ps1", "installation");
            }
            if shell_uninstaller.is_some() && ui.button("Uninstall Shell integration").clicked() {
                app.launch_shell_script("Uninstall-ShadeEditorShell.ps1", "removal");
            }
        });
    } else {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Bundled shell folder not found next to ShadeEditor.exe.",
        );
    }
}

fn settings_editing_tools(app: &mut ShadeApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(ui, "Editing / Tools");
    ui.strong("Channel palettes");
    let palette_library = app.settings.palette_library();
    let default_palette_name = if app.settings.default_palette_id == palette::AUTO_PALETTE_ID {
        "Automatic - CMYK/RGB from first Face".to_owned()
    } else {
        palette_library
            .iter()
            .find(|palette| palette.id == app.settings.default_palette_id)
            .map(|palette| palette.name.clone())
            .unwrap_or_else(|| "Automatic - CMYK/RGB from first Face".to_owned())
    };
    egui::ComboBox::from_label("Default palette for new projects")
        .selected_text(default_palette_name)
        .show_ui(ui, |ui| {
            effects.changed |= ui
                .selectable_value(
                    &mut app.settings.default_palette_id,
                    palette::AUTO_PALETTE_ID.to_owned(),
                    "Automatic - CMYK/RGB from first Face",
                )
                .changed();
            for palette in &palette_library {
                effects.changed |= ui
                    .selectable_value(
                        &mut app.settings.default_palette_id,
                        palette.id.clone(),
                        &palette.name,
                    )
                    .changed();
            }
        });

    ui.label("Built-in palettes (read-only)");
    for builtin in palette::builtin_palettes() {
        egui::CollapsingHeader::new(&builtin.name)
            .id_salt(format!("builtin-palette-{}", builtin.id))
            .show(ui, |ui| {
                for entry in &builtin.channels {
                    palette_entry_readonly(ui, entry);
                }
            });
    }

    let mut delete_palette = None;
    let mut add_channel_to = None;
    let mut remove_channel = None;
    for custom in &mut app.settings.custom_palettes {
        let custom_id = custom.id.clone();
        egui::CollapsingHeader::new(format!("Custom - {}", custom.name))
            .id_salt(format!("custom-palette-{custom_id}"))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Palette name");
                    effects.changed |= ui.text_edit_singleline(&mut custom.name).changed();
                    if ui.small_button("Delete palette").clicked() {
                        delete_palette = Some(custom_id.clone());
                    }
                });
                for (index, entry) in custom.channels.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{}", index + 1));
                        effects.changed |= ui
                            .add(egui::TextEdit::singleline(&mut entry.name).desired_width(130.0))
                            .changed();
                        effects.changed |= ui.color_edit_button_srgb(&mut entry.color).changed();
                        if ui
                            .small_button("-")
                            .on_hover_text("Remove channel slot")
                            .clicked()
                        {
                            remove_channel = Some((custom_id.clone(), index));
                        }
                    });
                }
                if ui.small_button("+ Channel slot").clicked() {
                    add_channel_to = Some(custom_id.clone());
                }
            });
    }
    if let Some((id, index)) = remove_channel {
        if let Some(custom) = app
            .settings
            .custom_palettes
            .iter_mut()
            .find(|item| item.id == id)
        {
            if index < custom.channels.len() {
                custom.channels.remove(index);
                effects.changed = true;
            }
        }
    }
    if let Some(id) = add_channel_to {
        if let Some(custom) = app
            .settings
            .custom_palettes
            .iter_mut()
            .find(|item| item.id == id)
        {
            let number = custom.channels.len() + 1;
            custom.channels.push(palette::ChannelPaletteEntry {
                name: format!("Ink {number}"),
                color: palette::fallback_channel_color("Spot", number - 1),
            });
            effects.changed = true;
        }
    }
    if let Some(id) = delete_palette {
        effects.changed |= app.settings.delete_custom_palette(&id);
    }
    if ui.button("+ New custom palette").clicked() {
        app.settings.create_custom_palette();
        effects.changed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_categories_cover_requested_groups() {
        let labels = SettingsCategory::ALL.map(SettingsCategory::label);
        assert!(labels.contains(&"General"));
        assert!(labels.contains(&"History"));
        assert!(labels.contains(&"File Handling"));
        assert!(labels.contains(&"Export"));
        assert!(labels.contains(&"Performance"));
    }
}
