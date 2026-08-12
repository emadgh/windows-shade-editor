from pathlib import Path
import re


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if text.count(old) != 1:
        raise SystemExit(f"{label}: expected 1 match, found {text.count(old)}")
    return text.replace(old, new, 1)


settings_path = Path("src/settings_v6.rs")
settings = settings_path.read_text(encoding="utf-8")
settings = replace_once(
    settings,
    "    pub show_curve_histogram: bool,\n    pub default_dpi: f64,",
    "    pub show_curve_histogram: bool,\n    pub compact_curve_controls: bool,\n    pub default_dpi: f64,",
    "settings field",
)
settings = replace_once(
    settings,
    "            show_curve_histogram: true,\n            default_dpi: DEFAULT_DPI,",
    "            show_curve_histogram: true,\n            compact_curve_controls: false,\n            default_dpi: DEFAULT_DPI,",
    "settings default",
)
test_anchor = """    #[test]
    fn default_dpi_is_220() {
        assert_eq!(AppSettings::default().default_dpi, 220.0);
    }
"""
settings = replace_once(
    settings,
    test_anchor,
    test_anchor
    + """
    #[test]
    fn compact_curve_controls_default_off() {
        assert!(!AppSettings::default().compact_curve_controls);
    }
""",
    "settings test",
)
settings_path.write_text(settings, encoding="utf-8")

app_path = Path("src/app_main.rs")
app = app_path.read_text(encoding="utf-8")

settings_anchor = """                changed |= ui
                    .checkbox(
                        &mut self.settings.adjustment_tabs,
                        "Use tabs for Levels / Curve / Mixer",
                    )
                    .changed();
                ui.separator();
                ui.heading("Color guides");"""
settings_replacement = """                changed |= ui
                    .checkbox(
                        &mut self.settings.adjustment_tabs,
                        "Use tabs for Levels / Curve / Mixer",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.compact_curve_controls,
                        "Compact Curve editor (hide Input / Output fields)",
                    )
                    .changed();
                ui.small("When enabled, Curve keeps only the draggable graph and hides the selected-point label, Input / Output fields, and helper text.");
                ui.separator();
                ui.heading("Color guides");"""
app = replace_once(app, settings_anchor, settings_replacement, "settings UI")

old_panel_header = """                if let Some(color) = panel_accent {
                    ui.visuals_mut().widgets.noninteractive.bg_stroke.color =
                        color.gamma_multiply(0.52);
                    ui.colored_label(color, format!("Editing: {output_display}"));
                }
                if ui.button("Reset all adjustments").clicked() {
                    self.project.reset_adjustments(&channel_names);
                    self.mark_all_previews_dirty();
                    self.report_info("All adjustments reset to defaults");
                }"""
new_panel_header = """                if let Some(color) = panel_accent {
                    ui.visuals_mut().widgets.noninteractive.bg_stroke.color =
                        color.gamma_multiply(0.52);
                }
                let reset_all = ui
                    .horizontal(|ui| {
                        if let Some(color) = panel_accent {
                            ui.colored_label(color, format!("Editing: {output_display}"));
                        } else {
                            ui.strong("Editing: All channels");
                        }
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| ui.small_button("Reset all").clicked(),
                        )
                        .inner
                    })
                    .inner;
                if reset_all {
                    self.project.reset_adjustments(&channel_names);
                    self.mark_all_previews_dirty();
                    self.report_info("All adjustments reset to defaults");
                }"""
app = replace_once(app, old_panel_header, new_panel_header, "editing/reset-all header")

selected_fn = r'''    fn ui_selected_adjustment(
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
        changed |= ui
            .checkbox(
                &mut adjustment.enabled,
                "Enable adjustment for this channel",
            )
            .changed();
        ui.add_enabled_ui(adjustment.enabled, |ui| {
            if self.settings.adjustment_tabs {
                let mut reset_tool = false;
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                    ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                    ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        reset_tool = ui.small_button("Reset").clicked();
                    });
                });
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
            }
        });
        changed
    }
'''
app, n = re.subn(
    r"    fn ui_selected_adjustment\(.*?\n    fn ui_all_adjustments\(",
    selected_fn + "\n    fn ui_all_adjustments(",
    app,
    count=1,
    flags=re.S,
)
if n != 1:
    raise SystemExit(f"selected adjustment replacement failed: {n}")

all_fn = r'''    fn ui_all_adjustments(
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
        let enabled_count = channel_names
            .iter()
            .filter(|name| {
                self.project
                    .adjustments
                    .get(*name)
                    .map(|adjustment| adjustment.enabled)
                    .unwrap_or(true)
            })
            .count();
        let mut all_enabled = enabled_count == channel_names.len();
        if ui
            .checkbox(&mut all_enabled, "Enable adjustments on all channels")
            .changed()
        {
            for name in channel_names {
                self.project
                    .adjustments
                    .entry(name.clone())
                    .or_default()
                    .enabled = all_enabled;
            }
            changed = true;
        }
        ui.small(
            "Levels broadcasts to every channel. Curve keeps one Broadcast control plus independent per-channel foldouts. Mixer output rows remain independent.",
        );

        if self.settings.adjustment_tabs {
            let mut reset_tool = false;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");
                ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");
                ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_tool = ui.small_button("Reset").clicked();
                });
            });
            if reset_tool {
                match self.tool {
                    ToolPanel::Levels => reset_all_levels(&mut self.project.adjustments, channel_names),
                    ToolPanel::Curves => reset_all_curves(&mut self.project.adjustments, channel_names),
                    ToolPanel::Mixer => reset_all_mixers(&mut self.project.adjustments, channel_names),
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
                "Levels — all channels",
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
                "all-curves-section",
                "Curve — broadcast + per channel",
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

            ui.add_space(4.0);
            let (body_changed, reset) = adjustment_foldout(
                ui,
                "all-mixers-section",
                "Channel Mixer — all output rows",
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
        }
        changed
    }
'''
app, n = re.subn(
    r"    fn ui_all_adjustments\(.*?\n    fn ui_tools\(",
    all_fn + "\n    fn ui_tools(",
    app,
    count=1,
    flags=re.S,
)
if n != 1:
    raise SystemExit(f"all adjustment replacement failed: {n}")

helper_block = r'''fn adjustment_foldout<R>(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    title: impl Into<egui::WidgetText>,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> (Option<R>, bool) {
    let id = ui.make_persistent_id(id_salt);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let title = title.into();
    let mut reset = false;
    let header = ui.horizontal(|ui| {
        state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
        let title_response = ui.add(egui::Label::new(title).sense(egui::Sense::click()));
        if title_response.clicked() {
            state.toggle(ui);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            reset = ui.small_button("Reset").clicked();
        });
    });
    let body = state.show_body_indented(&header.response, ui, body);
    (body.map(|response| response.inner), reset)
}

fn reset_mixer_row(
    adjustment: &mut ChannelAdjustment,
    output_name: &str,
    channel_names: &[String],
) {
    adjustment.mixer.coefficients.clear();
    for name in channel_names {
        adjustment
            .mixer
            .coefficients
            .insert(name.clone(), if name == output_name { 1.0 } else { 0.0 });
    }
    adjustment.mixer.constant = 0.0;
}

fn reset_all_levels(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
) {
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().levels = model::Levels::default();
    }
}

fn reset_all_curves(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
) {
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().curve = model::Curve::default();
    }
}

fn reset_all_mixers(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
) {
    for output_name in channel_names {
        let adjustment = adjustments.entry(output_name.clone()).or_default();
        reset_mixer_row(adjustment, output_name, channel_names);
    }
}

'''
if "fn adjustment_foldout<R>(" in app:
    raise SystemExit("adjustment_foldout already exists")
app = replace_once(app, "fn levels_ui(\n", helper_block + "fn levels_ui(\n", "helper insertion")

reset_levels_block = """        if ui.small_button("Reset Levels").clicked() {
            adjustment.levels = model::Levels::default();
            changed = true;
        }
"""
app = replace_once(app, reset_levels_block, "", "legacy Reset Levels")

curves_fn = r'''fn curves_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    compact_controls: bool,
) -> bool {
    with_accent(ui, accent, |ui| {
        let (graph_changed, selected) =
            curve_editor_graph(ui, &mut adjustment.curve, histogram, accent);
        let mut changed = graph_changed;
        if !compact_controls {
            ui.add_space(6.0);
            changed |= curve_point_fields(ui, &mut adjustment.curve, selected);
            ui.add_space(4.0);
            ui.small("Drag any of the three points directly. Input / Output use Photoshop-style 0-255 values.");
        }
        changed
    })
}
'''
app, n = re.subn(
    r"fn curves_ui\(.*?\n\}\n\nfn mixer_ui\(",
    curves_fn + "\nfn mixer_ui(",
    app,
    count=1,
    flags=re.S,
)
if n != 1:
    raise SystemExit(f"curves_ui replacement failed: {n}")

reset_mixer_block = """        if ui.small_button("Reset Mixer").clicked() {
            adjustment.mixer.coefficients.clear();
            for name in channel_names {
                adjustment
                    .mixer
                    .coefficients
                    .insert(name.clone(), if name == output_name { 1.0 } else { 0.0 });
            }
            adjustment.mixer.constant = 0.0;
            changed = true;
        }
"""
app = replace_once(app, reset_mixer_block, "", "legacy Reset Mixer")

curves_all_block = r'''fn broadcast_curves_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    compact_controls: bool,
) -> bool {
    let mut draft = adjustments.get(template_name).cloned().unwrap_or_default();
    if !curves_ui(ui, &mut draft, histogram, accent, compact_controls) {
        return false;
    }
    for name in channel_names {
        adjustments.entry(name.clone()).or_default().curve = draft.curve;
    }
    true
}

fn all_curves_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    histograms: &[[u32; 256]],
    colorize: bool,
    show_histogram: bool,
    compact_controls: bool,
    palette: Option<&ChannelPalette>,
) -> bool {
    let mut changed = false;
    let template_index = channel_names
        .iter()
        .position(|name| name == template_name)
        .unwrap_or(0);
    let broadcast_accent = colorize.then(|| channel_color(palette, template_name, template_index));
    let broadcast_histogram = show_histogram
        .then(|| histograms.get(template_index))
        .flatten();

    egui::Frame::new()
        .inner_margin(6)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(5)
        .show(ui, |ui| {
            ui.strong("Broadcast to all");
            ui.small("Changes here are copied to every channel Curve.");
            changed |= broadcast_curves_ui(
                ui,
                adjustments,
                template_name,
                channel_names,
                broadcast_histogram,
                broadcast_accent,
                compact_controls,
            );
        });

    ui.add_space(7.0);
    ui.strong("Per-channel Curves");
    ui.small("Open any channel to refine it after using Broadcast.");
    for (index, name) in channel_names.iter().enumerate() {
        let accent = colorize.then(|| channel_color(palette, name, index));
        let title = if let Some(color) = accent {
            egui::RichText::new(format!("●  {}", channel_display_name(palette, name, index)))
                .color(color)
        } else {
            egui::RichText::new(channel_display_name(palette, name, index))
        };
        egui::CollapsingHeader::new(title)
            .id_salt(format!("all-channel-curve-{index}-{name}"))
            .default_open(false)
            .show(ui, |ui| {
                let histogram = if show_histogram {
                    histograms.get(index)
                } else {
                    None
                };
                let adjustment = adjustments.entry(name.clone()).or_default();
                changed |= curves_ui(ui, adjustment, histogram, accent, compact_controls);
            });
    }
    changed
}
'''
app, n = re.subn(
    r"fn broadcast_curves_ui\(.*?\nfn all_mixers_ui\(",
    curves_all_block + "\nfn all_mixers_ui(",
    app,
    count=1,
    flags=re.S,
)
if n != 1:
    raise SystemExit(f"broadcast/all curves replacement failed: {n}")

if "Reset Levels" in app or "Reset Curve" in app or "Reset Mixer" in app:
    raise SystemExit("legacy bottom reset button still present")
if app.count("compact_curve_controls") < 5:
    raise SystemExit("compact Curve setting is not wired through enough UI paths")

app_path.write_text(app, encoding="utf-8")

cargo_path = Path("Cargo.toml")
cargo = cargo_path.read_text(encoding="utf-8")
cargo = replace_once(cargo, 'version = "0.7.0"', 'version = "0.7.1"', "Cargo version")
cargo_path.write_text(cargo, encoding="utf-8")

notes_path = Path("RELEASE_NOTES.md")
notes = notes_path.read_text(encoding="utf-8")
prefix = """# Shade Editor 0.7.1

Compact Adjustment layout controls.

- Adds an optional Compact Curve editor setting that hides the selected-point label, Input / Output numeric fields, and helper text while keeping all three graph points directly draggable.
- In Stacked mode, Levels, Curve, and Channel Mixer Reset actions now live on the same row as their foldout headers instead of at the bottom of each tool.
- Reset all is moved to the right side of the Editing channel header.
- Tabs mode keeps a contextual Reset action beside the tool tabs.
- All-channels Stacked mode uses the same header-level Reset behavior for Levels, Curve, and Mixer.
- No `.shade` schema change; the compact Curve choice is an application setting.

"""
notes_path.write_text(prefix + notes, encoding="utf-8")
