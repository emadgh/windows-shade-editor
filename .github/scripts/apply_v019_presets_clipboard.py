from pathlib import Path
import re


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


root = Path(__file__).resolve().parents[2]
main_path = root / "src" / "main.rs"
settings_path = root / "src" / "settings.rs"
tools_path = root / "src" / "adjustment_tools.rs"
main = main_path.read_text(encoding="utf-8")
settings = settings_path.read_text(encoding="utf-8")

# Focused adjustment service: clipboard + relative preset math live outside the UI shell.
tools_path.write_text(r'''use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{ChannelAdjustment, Curve, Levels, MixerRow};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardPart {
    All,
    Levels,
    Curve,
    Mixer,
}

#[derive(Clone, Debug)]
pub enum AdjustmentClipboard {
    All(ChannelAdjustment),
    Levels(Levels),
    Curve(Curve),
    Mixer(MixerRow),
}

impl AdjustmentClipboard {
    pub fn capture(adjustment: &ChannelAdjustment, part: ClipboardPart) -> Self {
        match part {
            ClipboardPart::All => Self::All(adjustment.clone()),
            ClipboardPart::Levels => Self::Levels(adjustment.levels),
            ClipboardPart::Curve => Self::Curve(adjustment.curve),
            ClipboardPart::Mixer => Self::Mixer(adjustment.mixer.clone()),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::All(_) => "All adjustments",
            Self::Levels(_) => "Levels",
            Self::Curve(_) => "Curve",
            Self::Mixer(_) => "Mixer",
        }
    }

    pub fn is_mixer_only(&self) -> bool {
        matches!(self, Self::Mixer(_))
    }

    pub fn paste_into(&self, target: &mut ChannelAdjustment, allow_mixer: bool) -> bool {
        let before = target.clone();
        match self {
            Self::All(source) => {
                target.enabled = source.enabled;
                target.levels = source.levels;
                target.curve = source.curve;
                if allow_mixer {
                    target.mixer = source.mixer.clone();
                }
            }
            Self::Levels(value) => target.levels = *value,
            Self::Curve(value) => target.curve = *value,
            Self::Mixer(value) if allow_mixer => target.mixer = value.clone(),
            Self::Mixer(_) => {}
        }
        *target != before
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RelativePreset {
    pub name: String,
    /// Exact project channel name -> relative percent change of that output's
    /// diagonal mixer coefficient. +2 means multiply current value by 1.02.
    pub channel_percent: BTreeMap<String, f32>,
}

#[derive(Clone, Debug, Default)]
pub struct RelativePresetDraft {
    pub name: String,
    pub channel_percent: BTreeMap<String, f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct BuiltinRelativePreset {
    pub id: &'static str,
    pub label: &'static str,
}

pub const BUILTIN_RELATIVE_PRESETS: [BuiltinRelativePreset; 6] = [
    BuiltinRelativePreset { id: "warmer", label: "Warmer" },
    BuiltinRelativePreset { id: "cooler", label: "Cooler" },
    BuiltinRelativePreset { id: "richer", label: "Darker / Richer" },
    BuiltinRelativePreset { id: "lighter", label: "Lighter" },
    BuiltinRelativePreset { id: "redder", label: "Redder" },
    BuiltinRelativePreset { id: "beiger", label: "More beige" },
];

fn normalized_channel(actual: &str, display: &str) -> String {
    format!("{} {}", actual.trim(), display.trim()).to_ascii_lowercase()
}

fn contains_role(value: &str, names: &[&str]) -> bool {
    names.iter().any(|name| {
        value == *name
            || value.split(|c: char| !c.is_ascii_alphanumeric()).any(|part| part == *name)
    })
}

fn builtin_delta(id: &str, actual: &str, display: &str) -> f32 {
    let name = normalized_channel(actual, display);
    let cyan_blue = contains_role(&name, &["cyan", "blue", "c"]);
    let magenta_red = contains_role(&name, &["magenta", "red", "m"]);
    let yellow = contains_role(&name, &["yellow", "y"]);
    let beige_brown = contains_role(&name, &["beige", "brown"]);
    let black = contains_role(&name, &["black", "key", "k"]);
    match id {
        "warmer" if yellow || beige_brown => 2.0,
        "warmer" if magenta_red => 1.0,
        "warmer" if cyan_blue => -2.0,
        "cooler" if yellow || beige_brown => -2.0,
        "cooler" if magenta_red => -1.0,
        "cooler" if cyan_blue => 2.0,
        "richer" => 2.0,
        "lighter" => -2.0,
        "redder" if magenta_red => 2.0,
        "redder" if cyan_blue => -1.0,
        "beiger" if yellow || beige_brown => 2.0,
        "beiger" if magenta_red => 1.0,
        "beiger" if cyan_blue => -1.5,
        "beiger" if black => -0.5,
        _ => 0.0,
    }
}

fn apply_percent(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel: &str,
    percent: f32,
) -> bool {
    if !percent.is_finite() || percent.abs() < f32::EPSILON {
        return false;
    }
    let adjustment = adjustments.entry(channel.to_owned()).or_default();
    let coefficient = adjustment
        .mixer
        .coefficients
        .entry(channel.to_owned())
        .or_insert(1.0);
    let before = *coefficient;
    *coefficient = (before * (1.0 + percent.clamp(-25.0, 25.0) / 100.0)).clamp(-2.0, 2.0);
    (*coefficient - before).abs() > f32::EPSILON
}

pub fn apply_builtin(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
    display_names: &[String],
    id: &str,
) -> bool {
    let mut changed = false;
    for (index, channel) in channel_names.iter().enumerate() {
        let display = display_names.get(index).map(String::as_str).unwrap_or(channel);
        changed |= apply_percent(adjustments, channel, builtin_delta(id, channel, display));
    }
    changed
}

pub fn apply_custom(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
    preset: &RelativePreset,
) -> bool {
    let mut changed = false;
    for channel in channel_names {
        if let Some(percent) = preset.channel_percent.get(channel) {
            changed |= apply_percent(adjustments, channel, *percent);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmer_is_relative_and_accumulates_without_overwriting_other_mix_values() {
        let channels = vec!["Blue".to_owned(), "Yellow".to_owned()];
        let display = channels.clone();
        let mut adjustments = BTreeMap::new();
        let blue = adjustments.entry("Blue".to_owned()).or_insert_with(ChannelAdjustment::default);
        blue.mixer.coefficients.insert("Blue".to_owned(), 0.90);
        blue.mixer.coefficients.insert("Yellow".to_owned(), 0.15);
        assert!(apply_builtin(&mut adjustments, &channels, &display, "warmer"));
        let blue = &adjustments["Blue"].mixer.coefficients;
        assert!((blue["Blue"] - 0.882).abs() < 0.0001);
        assert!((blue["Yellow"] - 0.15).abs() < 0.0001);
        assert!((adjustments["Yellow"].mixer.coefficients["Yellow"] - 1.02).abs() < 0.0001);
        apply_builtin(&mut adjustments, &channels, &display, "warmer");
        assert!(adjustments["Blue"].mixer.coefficients["Blue"] < 0.882);
    }

    #[test]
    fn clipboard_mixer_is_blocked_for_master_but_levels_are_allowed() {
        let mut source = ChannelAdjustment::default();
        source.levels.gamma = 1.25;
        source.mixer.coefficients.insert("Cyan".to_owned(), 0.8);
        let mixer = AdjustmentClipboard::capture(&source, ClipboardPart::Mixer);
        let mut target = ChannelAdjustment::default();
        assert!(!mixer.paste_into(&mut target, false));
        let levels = AdjustmentClipboard::capture(&source, ClipboardPart::Levels);
        assert!(levels.paste_into(&mut target, false));
        assert_eq!(target.levels.gamma, 1.25);
    }
}
''', encoding="utf-8")

# Register the extracted service module.
main = replace_once(main, "mod app_log;\nmod color_management;", "mod app_log;\nmod adjustment_tools;\nmod color_management;", "module declaration")

# Add non-persistent UI state to ShadeApp.
old_fields = '''    history_pending_label: Option<String>,\n    history_pending_at: Option<Instant>,\n    recovery_candidate: Option<recovery::RecoveryFile>,\n'''
new_fields = '''    history_pending_label: Option<String>,\n    history_pending_at: Option<Instant>,\n    adjustment_clipboard: Option<adjustment_tools::AdjustmentClipboard>,\n    relative_preset_draft: Option<adjustment_tools::RelativePresetDraft>,\n    recovery_candidate: Option<recovery::RecoveryFile>,\n'''
main = replace_once(main, old_fields, new_fields, "ShadeApp adjustment tool fields")
old_init = '''            history_pending_label: None,\n            history_pending_at: None,\n            recovery_candidate,\n'''
new_init = '''            history_pending_label: None,\n            history_pending_at: None,\n            adjustment_clipboard: None,\n            relative_preset_draft: None,\n            recovery_candidate,\n'''
main = replace_once(main, old_init, new_init, "ShadeApp adjustment tool init")

# Add the foldable quick-tools panel immediately before the main Adjustments function.
ui_anchor = '''    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {\n'''
ui_method = r'''    fn ui_adjustment_quick_tools(
        &mut self,
        ui: &mut egui::Ui,
        channel_names: &[String],
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
            self.save_settings_quietly();
        }
        changed
    }

    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {
'''
main = replace_once(main, ui_anchor, ui_method, "quick tools UI method")

# Insert the collapsed panel after heading/display mode controls and before scope selection.
insert_anchor = '''        if tonal_display_changed {\n            self.save_settings_quietly();\n        }\n        ui.horizontal_wrapped(|ui| {\n'''
insert_new = '''        if tonal_display_changed {\n            self.save_settings_quietly();\n        }\n        let quick_changed = self.ui_adjustment_quick_tools(\n            ui,\n            &channel_names,\n            palette.as_ref(),\n            &output_name,\n        );\n        if quick_changed {\n            self.mark_all_previews_dirty();\n        }\n        ui.add_space(4.0);\n        ui.horizontal_wrapped(|ui| {\n'''
main = replace_once(main, insert_anchor, insert_new, "quick tools call")
main_path.write_text(main, encoding="utf-8")

# Persist user-created relative presets in application settings, not in .shade.
settings = replace_once(
    settings,
    "use crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE, DEFAULT_FOLDER_TEMPLATE};",
    "use crate::adjustment_tools::RelativePreset;\nuse crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE, DEFAULT_FOLDER_TEMPLATE};",
    "settings preset import",
)
old_field = '''    pub default_palette_id: String,\n    pub custom_palettes: Vec<ChannelPalette>,\n'''
new_field = '''    pub default_palette_id: String,\n    pub custom_palettes: Vec<ChannelPalette>,\n    pub relative_adjustment_presets: Vec<RelativePreset>,\n'''
settings = replace_once(settings, old_field, new_field, "settings preset field")
old_default = '''            default_palette_id: AUTO_PALETTE_ID.to_owned(),\n            custom_palettes: Vec::new(),\n'''
new_default = '''            default_palette_id: AUTO_PALETTE_ID.to_owned(),\n            custom_palettes: Vec::new(),\n            relative_adjustment_presets: Vec::new(),\n'''
settings = replace_once(settings, old_default, new_default, "settings preset default")

sanitize_anchor = '''        if self.default_palette_id != AUTO_PALETTE_ID\n            && self.palette_by_id(&self.default_palette_id).is_none()\n        {\n            self.default_palette_id = AUTO_PALETTE_ID.to_owned();\n        }\n'''
sanitize_new = sanitize_anchor + '''\n        let mut preset_names = HashSet::new();\n        self.relative_adjustment_presets.retain_mut(|preset| {\n            preset.name = preset.name.trim().to_owned();\n            if preset.name.is_empty()\n                || !preset_names.insert(preset.name.to_ascii_lowercase())\n            {\n                return false;\n            }\n            preset.channel_percent.retain(|channel, value| {\n                !channel.trim().is_empty() && value.is_finite()\n            });\n            for value in preset.channel_percent.values_mut() {\n                *value = value.clamp(-25.0, 25.0);\n            }\n            true\n        });\n'''
settings = replace_once(settings, sanitize_anchor, sanitize_new, "settings preset sanitize")
settings_path.write_text(settings, encoding="utf-8")

Path(__file__).unlink()
bootstrap = root / ".github" / "workflows" / "apply-v019-presets-clipboard.yml"
if bootstrap.exists():
    bootstrap.unlink()
