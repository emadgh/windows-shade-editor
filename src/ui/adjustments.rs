use super::actions::AdjustmentUiAction;
use super::curve_editor::curves_ui;
use super::levels_mixer::{levels_ui, mixer_ui};
use super::match_color;
use crate::*;
use eframe::egui;

fn toggled_adjustment_scope(scope: AdjustmentScope) -> AdjustmentScope {
    match scope {
        AdjustmentScope::Selected => AdjustmentScope::All,
        AdjustmentScope::All => AdjustmentScope::Selected,
    }
}

impl ShadeApp {
    pub(crate) fn ui_history(&mut self, ui: &mut egui::Ui) {
        let scope = self.project.active_snapshot_id;
        let can_undo_clear = self
            .history_clear_backup
            .as_ref()
            .is_some_and(|(backup_scope, _)| *backup_scope == scope);
        let mut clear = false;
        let mut undo_clear = false;
        ui.horizontal(|ui| {
            ui.strong("History");
            if ui
                .add_enabled(self.history.can_undo(), egui::Button::new("Undo").small())
                .on_hover_text("Ctrl+Alt+Z")
                .clicked()
            {
                self.dispatch_adjustment_ui_action(AdjustmentUiAction::Undo, ui.ctx());
            }
            if ui
                .add_enabled(self.history.can_redo(), egui::Button::new("Redo").small())
                .on_hover_text("Ctrl+Shift+Z")
                .clicked()
            {
                self.dispatch_adjustment_ui_action(AdjustmentUiAction::Redo, ui.ctx());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if can_undo_clear {
                    undo_clear = ui.small_button("Undo clear").clicked();
                }
                clear = ui
                    .add_enabled(
                        self.history.len() > 1,
                        egui::Button::new("Clear history").small(),
                    )
                    .clicked();
            });
        });
        if let Some(name) = self.project.active_snapshot_name() {
            ui.small(format!(
                "Snapshot: {name} · up to {} adjustment states are saved in this .shade file.",
                self.settings.history_steps
            ));
        } else {
            ui.small("Working adjustment history. Create/select a Snapshot to keep an independent saved history.");
        }

        if clear {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::ClearHistory, ui.ctx());
        } else if undo_clear {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::RestoreClearedHistory, ui.ctx());
        }

        let rows = self
            .history
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| (index, entry.label.clone()))
            .collect::<Vec<_>>();
        let cursor = self.history.cursor();
        let mut requested = None;
        egui::ScrollArea::vertical()
            .id_salt("adjustment-history")
            .max_height(210.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (index, label) in rows {
                    if clickable_row(ui, index == cursor, &label, None, None, 28.0).clicked() {
                        requested = Some(index);
                    }
                }
            });
        if let Some(index) = requested {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::JumpHistory(index), ui.ctx());
        }
    }

    pub(crate) fn ui_channels_histogram(&mut self, ui: &mut egui::Ui) {
        let Some(face) = self.faces.get(self.current_face) else {
            ui.heading("Channels");
            ui.label("No active face");
            return;
        };
        if !face.available {
            ui.heading("Channels");
            ui.label("Source TIFF missing. Relink this Face to inspect channels and histograms.");
            return;
        }
        let channel_names = face.preview.metadata.channel_names.clone();
        let original_histograms = face.preview.histograms.clone();
        let adjusted_histograms = face.adjusted_histograms.clone();
        let clipping = face.clipping.clone();
        let base_count = face.preview.metadata.base_channel_count;
        let color_model = face.preview.metadata.color_model;
        let photoshop_display = face.preview.metadata.channel_display_info.clone();
        let mut match_target = match_color::target_snapshot();
        let mut target_overlay_visible = match_color::overlay_visible();
        if channel_names.is_empty() {
            return;
        }
        self.selected_channel = self.selected_channel.min(channel_names.len() - 1);
        let mut active_palette = self.project.channel_palette.clone();
        let palette_library = self.settings.palette_library();

        ui.horizontal(|ui| {
            ui.heading("Channels");
            let selected = active_palette
                .as_ref()
                .map(|palette| palette.name.as_str())
                .unwrap_or("TIFF channel names");
            let mut requested_palette = None;
            egui::ComboBox::from_id_salt("project-channel-palette")
                .selected_text(selected)
                .width(155.0)
                .show_ui(ui, |ui| {
                    for palette in &palette_library {
                        if ui
                            .selectable_label(
                                active_palette
                                    .as_ref()
                                    .is_some_and(|current| current.id == palette.id),
                                &palette.name,
                            )
                            .clicked()
                        {
                            requested_palette = Some(palette.clone());
                        }
                    }
                });
            if let Some(palette) = requested_palette {
                active_palette = Some(palette.clone());
                self.dispatch_adjustment_ui_action(
                    AdjustmentUiAction::SelectProjectPalette(palette),
                    ui.ctx(),
                );
            }
        });
        let composite_accent = ui.visuals().text_color();
        if clickable_channel_row(
            ui,
            self.adjustment_scope == AdjustmentScope::All,
            false,
            self.solo_channel.is_none(),
            "Composite",
            composite_accent,
            None,
            32.0,
        )
        .on_hover_text("Composite view · filled square means no individual channel is soloed. Click to return adjustment editing to Master.")
        .clicked()
        {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::ShowComposite, ui.ctx());
        }
        ui.small(format!(
            "{} + {} extra",
            color_model.title(),
            channel_names.len().saturating_sub(base_count)
        ));
        ui.add_space(3.0);
        for (index, name) in channel_names.iter().enumerate() {
            let display_info = photoshop_display.get(index).and_then(|value| *value);
            let suffix = if index >= base_count {
                match display_info {
                    Some(info) if info.is_spot() => "  spot",
                    Some(_) => "  alpha",
                    None => "  extra",
                }
            } else {
                ""
            };
            let accent = channel_color_with_photoshop(
                active_palette.as_ref(),
                &photoshop_display,
                name,
                index,
            );
            let is_solo = self.solo_channel == Some(index);
            let display_name = channel_display_name(active_palette.as_ref(), name, index);
            let label = format!("{display_name}{suffix}");
            let mut hover = match display_info {
                Some(info) if info.is_spot() => format!(
                    "Photoshop Spot Channel · Solidity {:.0}% · click to select; click again to toggle solo preview.",
                    info.solidity * 100.0
                ),
                Some(_) => "Photoshop Alpha/auxiliary channel · click to select; click again to toggle solo preview.".to_owned(),
                None => "Extra TIFF channel (Spot/Alpha type not declared) · click to select; click again to toggle solo preview.".to_owned(),
            };
            let warning = if self.settings.show_clipping_warnings {
                clipping
                    .get(index)
                    .copied()
                    .and_then(clipping_warning_color)
            } else {
                None
            };
            if self.settings.show_clipping_warnings {
                if let Some(stats) = clipping.get(index).copied() {
                    hover.push_str(&format!("\n{}", clipping_tooltip(stats)));
                }
            }
            let response = clickable_channel_row(
                ui,
                self.adjustment_scope == AdjustmentScope::Selected
                    && self.selected_channel == index,
                false,
                is_solo,
                &label,
                accent,
                warning,
                32.0,
            )
            .on_hover_text(hover);
            if response.clicked() {
                self.dispatch_adjustment_ui_action(
                    AdjustmentUiAction::SelectChannel(index),
                    ui.ctx(),
                );
            }
        }
        if self.solo_channel.is_some() && ui.small_button("Return to composite").clicked() {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::ShowComposite, ui.ctx());
        }

        ui.separator();
        let mut tonal_display_changed = false;
        let mut clear_match_target = false;
        ui.horizontal(|ui| {
            ui.strong("Histogram");
            let label = if self.settings.show_all_histograms {
                "Master"
            } else {
                "Selected"
            };
            if ui.small_button(label).clicked() {
                self.settings.show_all_histograms = !self.settings.show_all_histograms;
                self.dispatch_adjustment_ui_action(AdjustmentUiAction::PersistSettings, ui.ctx());
            }
            ui.separator();
            tonal_display_changed |=
                tonal_display_mode_selector(ui, &mut self.settings.tonal_display_mode);
            if let Some(target_name) = match_target.as_ref().map(|target| target.display_name()) {
                ui.separator();
                let toggle_label = if target_overlay_visible { "◉" } else { "○" };
                if ui
                    .small_button(toggle_label)
                    .on_hover_text(format!(
                        "Show/hide Reference/Match Color histogram overlay ({target_name})"
                    ))
                    .clicked()
                {
                    target_overlay_visible = !target_overlay_visible;
                    match_color::set_overlay_visible(target_overlay_visible);
                }
                if target_overlay_visible {
                    ui.colored_label(match_color::target_overlay_color(ui), "Reference");
                }
                clear_match_target = ui
                    .small_button("×")
                    .on_hover_text("Clear Reference/Match Color target. Applied Levels are kept.")
                    .clicked();
            }
        });
        if clear_match_target {
            match_color::clear_target();
            match_target = None;
            target_overlay_visible = false;
            self.report_info("Cleared Reference image; applied Match Color Levels were kept.");
        }
        if tonal_display_changed {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::PersistSettings, ui.ctx());
        }
        if self.settings.show_all_histograms {
            for (index, name) in channel_names.iter().enumerate() {
                let accent = self.settings.colorize_histograms.then(|| {
                    channel_color_with_photoshop(
                        active_palette.as_ref(),
                        &photoshop_display,
                        name,
                        index,
                    )
                });
                let display = channel_display_name(active_palette.as_ref(), name, index);
                ui.colored_label(accent.unwrap_or(ui.visuals().text_color()), display);
                let target_histogram = if target_overlay_visible {
                    match_target
                        .as_ref()
                        .and_then(|target| target.histograms.get(index))
                } else {
                    None
                };
                match_color::draw_histogram_with_target(
                    ui,
                    original_histograms.get(index),
                    adjusted_histograms.get(index),
                    target_histogram,
                    accent,
                    self.settings.tonal_display_mode,
                );
            }
        } else {
            let index = self.selected_channel;
            let accent = self.settings.colorize_histograms.then(|| {
                channel_color_with_photoshop(
                    active_palette.as_ref(),
                    &photoshop_display,
                    &channel_names[index],
                    index,
                )
            });
            let display =
                channel_display_name(active_palette.as_ref(), &channel_names[index], index);
            ui.strong(format!("Histogram - {display}"));
            let target_histogram = if target_overlay_visible {
                match_target
                    .as_ref()
                    .and_then(|target| target.histograms.get(index))
            } else {
                None
            };
            match_color::draw_histogram_with_target(
                ui,
                original_histograms.get(index),
                adjusted_histograms.get(index),
                target_histogram,
                accent,
                self.settings.tonal_display_mode,
            );
        }
    }

    pub(crate) fn ui_adjustment_quick_tools(
        &mut self,
        ui: &mut egui::Ui,
        channel_names: &[String],
        source_histograms: &[[u32; 256]],
        palette: Option<&ChannelPalette>,
        output_name: &str,
    ) -> bool {
        let display_names = channel_names
            .iter()
            .enumerate()
            .map(|(index, name)| channel_display_name(palette, name, index).to_owned())
            .collect::<Vec<_>>();
        let custom_presets = self.settings.relative_adjustment_presets.clone();
        let mut changed = false;
        let mut settings_changed = false;
        let mut copy_part = None;
        let mut paste_requested = false;
        let scope_is_master = self.adjustment_scope == AdjustmentScope::All;
        let source_adjustment = if scope_is_master {
            self.project
                .adjustments
                .get(MASTER_ADJUSTMENT_KEY)
                .cloned()
                .unwrap_or_default()
        } else {
            self.project
                .adjustments
                .get(output_name)
                .cloned()
                .unwrap_or_default()
        };

        let existing_match_target = match_color::target_snapshot();
        let mut match_requested = false;
        ui.horizontal_wrapped(|ui| {
            match_requested = ui
                .button("Match Color")
                .on_hover_text(
                    "Match all source channels to the current Reference image using editable Levels. If no Reference is selected, choose one first.",
                )
                .clicked();
            if let Some(target) = existing_match_target.as_ref() {
                ui.small(format!("Reference: {}", target.display_name()))
                    .on_hover_text(target.path.display().to_string());
            } else {
                ui.small("No Reference selected");
            }
        });
        if match_requested {
            let target_result = if let Some(target) = existing_match_target.clone() {
                Ok(Some(target))
            } else {
                match_color::choose_target(self.settings.max_preview_dimension)
            };
            match target_result {
                Ok(Some(target)) => {
                    let report = match_color::apply_histogram_match_levels(
                        &mut self.project.adjustments,
                        channel_names,
                        source_histograms,
                        &target,
                    );
                    changed |= report.changed;
                    self.tool = ToolPanel::Levels;
                    self.adjustment_scope = AdjustmentScope::Selected;
                    self.report_info(format!(
                        "Match Color: {} matched to {} · {} source-only set to zero · {} target-only ignored",
                        report.matched_channels,
                        target.display_name(),
                        report.zeroed_source_only_channels,
                        report.ignored_target_only_channels,
                    ));
                }
                Ok(None) => {}
                Err(err) => self.report_error(err),
            }
        }

        egui::CollapsingHeader::new("Quick relative adjustments / Presets")
            .id_salt("relative-adjustment-presets")
            .default_open(false)
            .show(ui, |ui| {
                ui.small("Each click changes the current mixer intensity relatively; repeated clicks accumulate. Existing cross-channel mix values are not replaced.");
                ui.horizontal_wrapped(|ui| {
                    for preset in adjustment_tools::BUILTIN_RELATIVE_PRESETS {
                        if ui.small_button(preset.label).clicked() {
                            if adjustment_tools::apply_builtin(
                                &mut self.project.adjustments,
                                channel_names,
                                &display_names,
                                preset.id,
                            ) {
                                changed = true;
                                self.report_info(format!("Applied {} relative adjustment", preset.label));
                            } else {
                                self.report_info(format!("{}: no matching channels in this project", preset.label));
                            }
                        }
                    }
                });

                if !custom_presets.is_empty() {
                    ui.add_space(5.0);
                    ui.strong("Custom presets");
                    for (index, preset) in custom_presets.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button(&preset.name).clicked()
                                && adjustment_tools::apply_custom(
                                    &mut self.project.adjustments,
                                    channel_names,
                                    preset,
                                )
                            {
                                changed = true;
                                self.report_info(format!("Applied relative preset: {}", preset.name));
                            }
                            if ui.small_button("Edit").clicked() {
                                self.relative_preset_draft = Some(adjustment_tools::RelativePresetDraft {
                                    name: preset.name.clone(),
                                    channel_percent: preset.channel_percent.clone(),
                                });
                            }
                            if ui.small_button("Delete").clicked() {
                                if index < self.settings.relative_adjustment_presets.len() {
                                    self.settings.relative_adjustment_presets.remove(index);
                                    settings_changed = true;
                                }
                            }
                        });
                    }
                }

                ui.add_space(6.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.menu_button("Copy", |ui| {
                        if ui.button("All adjustments").clicked() {
                            copy_part = Some(adjustment_tools::ClipboardPart::All);
                            ui.close();
                        }
                        if ui.button("Levels").clicked() {
                            copy_part = Some(adjustment_tools::ClipboardPart::Levels);
                            ui.close();
                        }
                        if ui.button("Curve").clicked() {
                            copy_part = Some(adjustment_tools::ClipboardPart::Curve);
                            ui.close();
                        }
                        if ui
                            .add_enabled(!scope_is_master, egui::Button::new("Mixer"))
                            .clicked()
                        {
                            copy_part = Some(adjustment_tools::ClipboardPart::Mixer);
                            ui.close();
                        }
                    });
                    let paste_label = self
                        .adjustment_clipboard
                        .as_ref()
                        .map(|item| format!("Paste {}", item.label()))
                        .unwrap_or_else(|| "Paste".to_owned());
                    let paste_allowed = self.adjustment_clipboard.as_ref().is_some_and(|item| {
                        !scope_is_master || !item.is_mixer_only()
                    });
                    paste_requested = ui
                        .add_enabled(paste_allowed, egui::Button::new(paste_label))
                        .clicked();
                    if ui.small_button("New custom preset").clicked() {
                        self.relative_preset_draft = Some(adjustment_tools::RelativePresetDraft {
                            name: String::new(),
                            channel_percent: channel_names
                                .iter()
                                .map(|name| (name.clone(), 0.0))
                                .collect(),
                        });
                    }
                });

                let mut save_draft = false;
                let mut cancel_draft = false;
                if let Some(draft) = self.relative_preset_draft.as_mut() {
                    ui.add_space(7.0);
                    egui::Frame::new()
                        .inner_margin(7)
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                        .corner_radius(4)
                        .show(ui, |ui| {
                            ui.strong("Custom relative preset editor");
                            ui.add(
                                egui::TextEdit::singleline(&mut draft.name)
                                    .hint_text("Preset name")
                                    .desired_width(220.0),
                            );
                            ui.small("Relative percent per channel. Example: -2 means current value × 0.98; +2 means × 1.02.");
                            for (index, channel) in channel_names.iter().enumerate() {
                                let display = display_names.get(index).map(String::as_str).unwrap_or(channel);
                                let value = draft.channel_percent.entry(channel.clone()).or_insert(0.0);
                                ui.horizontal(|ui| {
                                    ui.label(display);
                                    ui.add(
                                        egui::DragValue::new(value)
                                            .range(-25.0..=25.0)
                                            .speed(0.25)
                                            .suffix(" %"),
                                    );
                                });
                            }
                            ui.horizontal(|ui| {
                                save_draft = ui
                                    .add_enabled(!draft.name.trim().is_empty(), egui::Button::new("Save preset"))
                                    .clicked();
                                cancel_draft = ui.button("Cancel").clicked();
                            });
                        });
                }
                if save_draft {
                    if let Some(draft) = self.relative_preset_draft.take() {
                        let name = draft.name.trim().to_owned();
                        let preset = adjustment_tools::RelativePreset {
                            name: name.clone(),
                            channel_percent: draft.channel_percent,
                        };
                        if let Some(existing) = self
                            .settings
                            .relative_adjustment_presets
                            .iter_mut()
                            .find(|item| item.name.eq_ignore_ascii_case(&name))
                        {
                            *existing = preset;
                        } else {
                            self.settings.relative_adjustment_presets.push(preset);
                        }
                        settings_changed = true;
                    }
                } else if cancel_draft {
                    self.relative_preset_draft = None;
                }
            });

        if let Some(part) = copy_part {
            self.adjustment_clipboard = Some(adjustment_tools::AdjustmentClipboard::capture(
                &source_adjustment,
                part,
            ));
            if let Some(item) = &self.adjustment_clipboard {
                self.report_info(format!("Copied {}", item.label()));
            }
        }
        if paste_requested {
            if let Some(clipboard) = self.adjustment_clipboard.clone() {
                let target = if scope_is_master {
                    self.project
                        .adjustments
                        .entry(MASTER_ADJUSTMENT_KEY.to_owned())
                        .or_default()
                } else {
                    self.project
                        .adjustments
                        .entry(output_name.to_owned())
                        .or_default()
                };
                if clipboard.paste_into(target, !scope_is_master) {
                    changed = true;
                    if scope_is_master {
                        cleanup_master_adjustment(&mut self.project.adjustments);
                    }
                    self.report_info(format!("Pasted {}", clipboard.label()));
                }
            }
        }
        if settings_changed {
            self.settings.sanitize();
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::PersistSettings, ui.ctx());
        }
        changed
    }

    pub(crate) fn ui_adjustments(&mut self, ui: &mut egui::Ui) {
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
        let all_original_histograms = face.preview.histograms.clone();
        let all_adjusted_histograms = face.adjusted_histograms.clone();
        let active_original_histogram = all_original_histograms.get(self.selected_channel).copied();
        let active_adjusted_histogram = all_adjusted_histograms.get(self.selected_channel).copied();
        let active_clipping = face.clipping.get(self.selected_channel).copied();
        let control_accent = self
            .settings
            .colorize_adjustments
            .then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));
        let panel_accent = (self.adjustment_scope == AdjustmentScope::Selected)
            .then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));
        let modified_count = channel_names
            .iter()
            .filter(|name| {
                self.project
                    .adjustments
                    .get(*name)
                    .is_some_and(adjustment_is_modified)
            })
            .count();
        let output_modified = self
            .project
            .adjustments
            .get(&output_name)
            .is_some_and(adjustment_is_modified);
        let master_modified = self
            .project
            .adjustments
            .get(MASTER_ADJUSTMENT_KEY)
            .is_some_and(master_adjustment_is_modified);

        let mut tonal_display_changed = false;
        let mut reset_everything = false;
        ui.horizontal(|ui| {
            ui.heading("Adjustments");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                reset_everything = ui
                    .small_button("↻")
                    .on_hover_text("Reset All Channels + Master adjustments")
                    .clicked();
                ui.separator();
                let layout_label = if self.settings.adjustment_tabs {
                    "Tabs"
                } else {
                    "Stacked"
                };
                if ui.small_button(layout_label).clicked() {
                    self.settings.adjustment_tabs = !self.settings.adjustment_tabs;
                    self.dispatch_adjustment_ui_action(
                        AdjustmentUiAction::PersistSettings,
                        ui.ctx(),
                    );
                }
                ui.separator();
                tonal_display_changed |=
                    tonal_display_mode_selector(ui, &mut self.settings.tonal_display_mode);
            });
        });
        if reset_everything {
            for output in &channel_names {
                let adjustment = self.project.adjustments.entry(output.clone()).or_default();
                *adjustment = ChannelAdjustment::default();
                reset_mixer_row(adjustment, output, &channel_names);
            }
            self.project.adjustments.remove(MASTER_ADJUSTMENT_KEY);
            self.report_info("Reset all channel and Master adjustments");
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::InvalidatePreviews, ui.ctx());
        }
        if tonal_display_changed {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::PersistSettings, ui.ctx());
        }
        let quick_changed = self.ui_adjustment_quick_tools(
            ui,
            &channel_names,
            &all_original_histograms,
            palette.as_ref(),
            &output_name,
        );
        if quick_changed {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::InvalidatePreviews, ui.ctx());
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let selected_scope = self.adjustment_scope == AdjustmentScope::Selected;
            let scope_label = match self.adjustment_scope {
                AdjustmentScope::Selected if output_modified => format!("{output_display}  •"),
                AdjustmentScope::Selected => output_display.to_owned(),
                AdjustmentScope::All if master_modified => "Master  •".to_owned(),
                AdjustmentScope::All => "Master".to_owned(),
            };
            let scope_text = if selected_scope && control_accent.is_some() {
                egui::WidgetText::from(egui::RichText::new(scope_label).color(egui::Color32::WHITE))
            } else {
                egui::WidgetText::from(scope_label)
            };
            let scope_accent = if selected_scope { control_accent } else { None };
            let hover = if selected_scope {
                "Switch adjustment scope to Master (~)".to_owned()
            } else {
                format!("Switch adjustment scope to {output_display} (~)")
            };
            let response = with_accent(ui, scope_accent, |ui| {
                ui.add(egui::Button::new(scope_text).selected(true))
            })
            .on_hover_text(hover);
            if response.clicked() {
                self.adjustment_scope = toggled_adjustment_scope(self.adjustment_scope);
            }
            if modified_count > 0 {
                ui.small(format!("Modified {modified_count}/{}", channel_names.len()));
            }
        });
        if self.settings.show_clipping_warnings {
            if let Some(stats) = active_clipping.filter(|stats| stats.max_percent() > 0.0) {
                clipping_summary_ui(ui, stats);
            }
        }

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
                                ui.strong("Editing: Master");
                                match self.tool {
                                    ToolPanel::Levels | ToolPanel::Curves => {
                                        let mut master_enabled = self
                                            .project
                                            .adjustments
                                            .get(MASTER_ADJUSTMENT_KEY)
                                            .map(|adjustment| adjustment.enabled)
                                            .unwrap_or(true);
                                        if ui
                                            .checkbox(&mut master_enabled, "Master enabled")
                                            .on_hover_text(
                                                "Bypasses only Master Levels and Master Curve. Per-channel controls are never changed.",
                                            )
                                            .changed()
                                        {
                                            self.project
                                                .adjustments
                                                .entry(MASTER_ADJUSTMENT_KEY.to_owned())
                                                .or_default()
                                                .enabled = master_enabled;
                                            cleanup_master_adjustment(&mut self.project.adjustments);
                                            header_changed = true;
                                        }
                                    }
                                    ToolPanel::Mixer => {
                                        ui.small("Mixer rows remain channel-specific");
                                    }
                                }
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let tooltip = match self.adjustment_scope {
                                AdjustmentScope::Selected => "Reset Channel — reset all adjustments for this channel",
                                AdjustmentScope::All => "Reset Master — reset the complete Master adjustment stack",
                            };
                            ui.small_button("↻").on_hover_text(tooltip).clicked()
                        })
                        .inner
                    })
                    .inner;
                if reset_all {
                    match self.adjustment_scope {
                        AdjustmentScope::Selected => {
                            let adjustment = self
                                .project
                                .adjustments
                                .entry(output_name.clone())
                                .or_default();
                            *adjustment = ChannelAdjustment::default();
                            reset_mixer_row(adjustment, &output_name, &channel_names);
                            self.report_info(format!("Reset {output_display} adjustments"));
                        }
                        AdjustmentScope::All => {
                            self.project.adjustments.remove(MASTER_ADJUSTMENT_KEY);
                            self.report_info("Reset Master adjustments");
                        }
                    }
                    self.dispatch_adjustment_ui_action(AdjustmentUiAction::InvalidatePreviews, ui.ctx());
                }
                let body_changed = match self.adjustment_scope {
                    AdjustmentScope::Selected => self.ui_selected_adjustment(
                        ui,
                        &output_name,
                        &channel_names,
                        active_original_histogram.as_ref(),
                        active_adjusted_histogram.as_ref(),
                        control_accent,
                        palette.as_ref(),
                    ),
                    AdjustmentScope::All => self.ui_all_adjustments(
                        ui,
                        &channel_names,
                        &all_original_histograms,
                        &all_adjusted_histograms,
                        palette.as_ref(),
                    ),
                };
                header_changed || body_changed
            })
            .inner;
        if changed {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::InvalidatePreviews, ui.ctx());
        }
        if self.project.adjustments != adjustments_before {
            self.dispatch_adjustment_ui_action(
                AdjustmentUiAction::QueueHistory(adjustments_before),
                ui.ctx(),
            );
        }
    }

    pub(crate) fn ui_selected_adjustment(
        &mut self,
        ui: &mut egui::Ui,
        output_name: &str,
        channel_names: &[String],
        histogram_before: Option<&[u32; 256]>,
        histogram_after: Option<&[u32; 256]>,
        accent: Option<egui::Color32>,
        palette: Option<&ChannelPalette>,
    ) -> bool {
        let mut changed = false;
        let compact_curve_controls = self.settings.compact_curve_controls;
        let tonal_display_mode = self.settings.tonal_display_mode;
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
                    ToolPanel::Levels => levels_ui(
                        ui,
                        adjustment,
                        histogram_before,
                        histogram_after,
                        accent,
                        tonal_display_mode,
                    ),
                    ToolPanel::Curves => curves_ui(
                        ui,
                        adjustment,
                        histogram_before.filter(|_| self.settings.show_curve_histogram),
                        histogram_after.filter(|_| self.settings.show_curve_histogram),
                        accent,
                        tonal_display_mode,
                        compact_curve_controls,
                        false,
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
                    |ui| {
                        levels_ui(
                            ui,
                            adjustment,
                            histogram_before,
                            histogram_after,
                            accent,
                            tonal_display_mode,
                        )
                    },
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
                            histogram_before.filter(|_| self.settings.show_curve_histogram),
                            histogram_after.filter(|_| self.settings.show_curve_histogram),
                            accent,
                            tonal_display_mode,
                            compact_curve_controls,
                            false,
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
}

#[cfg(test)]
mod scope_toggle_tests {
    use super::{AdjustmentScope, toggled_adjustment_scope};

    #[test]
    fn adjustment_scope_toggle_round_trips() {
        let master = toggled_adjustment_scope(AdjustmentScope::Selected);
        assert!(matches!(master, AdjustmentScope::All));
        let selected = toggled_adjustment_scope(master);
        assert!(matches!(selected, AdjustmentScope::Selected));
    }
}
