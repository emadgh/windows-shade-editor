from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_exact(path: Path, old: str, new: str, expected: int = 1) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count == 0:
        if new in text:
            return
        raise RuntimeError(f"pattern not found in {path}: {old[:120]!r}")
    if count != expected:
        raise RuntimeError(f"unexpected match count in {path}: got {count}, expected {expected}")
    path.write_text(text.replace(old, new), encoding="utf-8")


app = ROOT / "src" / "app_main.rs"
render = ROOT / "src" / "render.rs"
export = ROOT / "src" / "export_v6.rs"

replace_exact(
    app,
    "enum ToolPanel {\n    Levels,\n    Curves,\n    Mixer,\n}",
    "enum ToolPanel {\n    Levels,\n    Mixer,\n    Curves,\n}",
)

# Selected-channel tab row.
replace_exact(
    app,
    '                    ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");\n'
    '                    ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");\n'
    '                    ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");',
    '                    ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");\n'
    '                    ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");\n'
    '                    ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");',
)

# All-channels tab row has one less indentation level.
replace_exact(
    app,
    '                ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");\n'
    '                ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");\n'
    '                ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");',
    '                ui.selectable_value(&mut self.tool, ToolPanel::Levels, "Levels");\n'
    '                ui.selectable_value(&mut self.tool, ToolPanel::Mixer, "Mixer");\n'
    '                ui.selectable_value(&mut self.tool, ToolPanel::Curves, "Curve");',
)

selected_old = '''                ui.add_space(4.0);
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
                }'''
selected_new = '''                ui.add_space(4.0);
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
                }'''
replace_exact(app, selected_old, selected_new)

all_old = '''            ui.add_space(4.0);
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
            }'''
all_new = '''            ui.add_space(4.0);
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
            }'''
replace_exact(app, all_old, all_new)

replace_exact(app, "Use tabs for Levels / Curve / Mixer", "Use tabs for Levels / Mixer / Curve")
replace_exact(app, "Colorize Levels / Curve / Mixer by channel", "Colorize Levels / Mixer / Curve by channel")
replace_exact(
    app,
    "Levels broadcasts to every channel. Curve keeps one Broadcast control plus independent per-channel foldouts. Mixer output rows remain independent.",
    "Levels broadcasts to every channel. Mixer output rows remain independent. Curve keeps one Broadcast control plus independent per-channel foldouts.",
)

# Preview pipeline: Levels on source channels, then output-row Mixer, then output Curve.
replace_exact(
    render,
    "                    apply_curve(apply_levels(raw, adjustment.levels), adjustment.curve)",
    "                    apply_levels(raw, adjustment.levels)",
)
replace_exact(
    render,
    "                    mixed\n                }\n            } else {\n                prepared[out_channel][pixel]\n            };",
    "                    apply_curve(mixed, adjustment.curve)\n                }\n            } else {\n                prepared[out_channel][pixel]\n            };",
)

# Production export, full-decode and streaming paths.
replace_exact(
    export,
    "                        apply_curve(apply_levels(raw, adjustment.levels), adjustment.curve)",
    "                        apply_levels(raw, adjustment.levels)",
    expected=2,
)
replace_exact(
    export,
    "                        mixed\n                    }\n                    _ => prepared[out_channel],",
    "                        apply_curve(mixed, adjustment.curve)\n                    }\n                    _ => prepared[out_channel],",
    expected=2,
)

# Regression test makes the ordering contract explicit.
test_anchor = '''mod streaming_tests {
    use super::*;
    use tiff::encoder::{Compression, TiffEncoder, colortype};
    use tiff::tags::ExtraSamples;
'''
test_block = '''mod streaming_tests {
    use super::*;
    use tiff::encoder::{Compression, TiffEncoder, colortype};
    use tiff::tags::ExtraSamples;

    #[test]
    fn adjustment_pipeline_is_levels_then_mixer_then_curve() {
        let names = vec!["A".to_owned(), "B".to_owned()];
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);

        {
            let adjustment = project.adjustments.get_mut("A").unwrap();
            adjustment.levels.output_white = 0.5;
            adjustment.mixer.constant = 0.0;
            adjustment.mixer.coefficients.insert("A".to_owned(), 0.5);
            adjustment.mixer.coefficients.insert("B".to_owned(), 0.5);
            adjustment.curve.midpoint_enabled = true;
            adjustment.curve.midpoint_input = 0.5;
            adjustment.curve.midpoint = 0.1;
        }

        let input = [13_107u16, 52_428u16];
        let output = adjusted_strip(&input, 2, &names, &project);

        let raw_a = input[0] as f32 / 65_535.0;
        let raw_b = input[1] as f32 / 65_535.0;
        let a = project.adjustments.get("A").unwrap();
        let b = project.adjustments.get("B").unwrap();
        let leveled_a = apply_levels(raw_a, a.levels);
        let leveled_b = apply_levels(raw_b, b.levels);
        let mixed = a.mixer.constant + leveled_a * 0.5 + leveled_b * 0.5;
        let expected = apply_curve(mixed, a.curve);
        let actual = output[0] as f32 / 65_535.0;
        assert!((actual - expected).abs() <= 2.0 / 65_535.0);

        let legacy = apply_curve(leveled_a, a.curve) * 0.5 + leveled_b * 0.5;
        assert!((actual - legacy).abs() > 0.20);
    }
'''
replace_exact(export, test_anchor, test_block)
