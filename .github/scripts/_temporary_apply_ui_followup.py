from pathlib import Path
import re


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one match, found {count}\n--- needle ---\n{old}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


def append_before_last_brace(path: str, marker: str, addition: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if marker not in text:
        raise SystemExit(f"{path}: marker not found: {marker}")
    text = text.replace(marker, addition + marker, 1)
    p.write_text(text, encoding="utf-8", newline="\n")


# 1) Keep viewport zoom strip clear of the horizontal scrollbar.
replace_once(
    "src/ui/viewport_controls.rs",
    "use crate::*;\nuse eframe::egui;\n",
    "use crate::*;\nuse eframe::egui;\n\nconst ZOOM_STRIP_BOTTOM_CLEARANCE: f32 = 30.0;\n\nfn zoom_strip_position(\n    viewport: egui::Rect,\n    panel_width: f32,\n    panel_height: f32,\n) -> egui::Pos2 {\n    egui::pos2(\n        viewport.center().x - panel_width * 0.5,\n        viewport.bottom() - panel_height - ZOOM_STRIP_BOTTOM_CLEARANCE,\n    )\n}\n",
)
replace_once(
    "src/ui/viewport_controls.rs",
    "    let pos = egui::pos2(\n        viewport.center().x - panel_width * 0.5,\n        viewport.bottom() - panel_height - 8.0,\n    );",
    "    let pos = zoom_strip_position(viewport, panel_width, panel_height);",
)
replace_once(
    "src/ui/viewport_controls.rs",
    "    fn zoom_strip_never_needs_to_exceed_usable_viewport_width() {\n        for viewport_width in [180.0_f32, 220.0, 360.0, 900.0] {\n            let panel_width = viewport_width.min(360.0);\n            assert!(panel_width <= viewport_width);\n        }\n    }",
    "    fn zoom_strip_never_needs_to_exceed_usable_viewport_width() {\n        for viewport_width in [180.0_f32, 220.0, 360.0, 900.0] {\n            let panel_width = viewport_width.min(360.0);\n            assert!(panel_width <= viewport_width);\n        }\n    }\n\n    #[test]\n    fn zoom_strip_reserves_bottom_scrollbar_clearance() {\n        let viewport = egui::Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(900.0, 700.0));\n        let panel_height = 32.0;\n        let pos = zoom_strip_position(viewport, 360.0, panel_height);\n        let strip_bottom = pos.y + panel_height;\n        assert!((viewport.bottom() - strip_bottom - ZOOM_STRIP_BOTTOM_CLEARANCE).abs() < f32::EPSILON);\n        assert!(strip_bottom <= viewport.bottom() - 24.0);\n    }",
)

# 2) Composite is the visual row for Master adjustment scope.
replace_once(
    "src/ui/adjustments.rs",
    "        if clickable_channel_row(\n            ui,\n            false,\n            false,\n            self.solo_channel.is_none(),\n            \"Composite\",",
    "        if clickable_channel_row(\n            ui,\n            self.adjustment_scope == AdjustmentScope::All,\n            false,\n            self.solo_channel.is_none(),\n            \"Composite\",",
)

# 3 + 5) Levels histogram uses per-series peak normalization and conventional gamma-marker direction.
replace_once(
    "src/ui/levels_mixer.rs",
    "fn gamma_marker_fraction(gamma: f32) -> f32 {\n    let gamma = gamma.clamp(0.1, 4.0);\n    if gamma <= 1.0 {\n        0.5 * (gamma / 0.1).ln() / 10.0_f32.ln()\n    } else {\n        0.5 + 0.5 * gamma.ln() / 4.0_f32.ln()\n    }\n}\n\nfn gamma_from_marker_fraction(fraction: f32) -> f32 {\n    let fraction = fraction.clamp(0.0, 1.0);\n    if fraction <= 0.5 {\n        0.1 * 10.0_f32.powf(fraction * 2.0)\n    } else {\n        4.0_f32.powf((fraction - 0.5) * 2.0)\n    }\n    .clamp(0.1, 4.0)\n}\n",
    "fn gamma_marker_fraction(gamma: f32) -> f32 {\n    let gamma = gamma.clamp(0.1, 4.0);\n    // Conventional Levels behavior: moving the midtone marker left lightens\n    // midtones (gamma > 1), while moving it right darkens them (gamma < 1).\n    if gamma >= 1.0 {\n        0.5 - 0.5 * gamma.ln() / 4.0_f32.ln()\n    } else {\n        0.5 + 0.5 * (1.0 / gamma).ln() / 10.0_f32.ln()\n    }\n}\n\nfn gamma_from_marker_fraction(fraction: f32) -> f32 {\n    let fraction = fraction.clamp(0.0, 1.0);\n    if fraction <= 0.5 {\n        4.0_f32.powf((0.5 - fraction) * 2.0)\n    } else {\n        10.0_f32.powf(-(fraction - 0.5) * 2.0)\n    }\n    .clamp(0.1, 4.0)\n}\n\nfn histogram_height_at_peak_normalized(\n    histogram: &[u32; 256],\n    index: usize,\n    graph_height: f32,\n) -> f32 {\n    let peak = histogram.iter().copied().max().unwrap_or(0).max(1) as f32;\n    histogram[index] as f32 / peak * graph_height\n}\n",
)
replace_once(
    "src/ui/levels_mixer.rs",
    "    let max_value = before\n        .into_iter()\n        .flat_map(|bins| bins.iter())\n        .chain(after.into_iter().flat_map(|bins| bins.iter()))\n        .copied()\n        .max()\n        .unwrap_or(1)\n        .max(1) as f32;\n    for index in 0..256 {",
    "    for index in 0..256 {",
)
replace_once(
    "src/ui/levels_mixer.rs",
    "            let h = bins[index] as f32 / max_value * rect.height();",
    "            let h = histogram_height_at_peak_normalized(bins, index, rect.height());",
)
replace_once(
    "src/ui/levels_mixer.rs",
    "            let h = bins[index] as f32 / max_value * rect.height();",
    "            let h = histogram_height_at_peak_normalized(bins, index, rect.height());",
)
replace_once(
    "src/ui/levels_mixer.rs",
    "    fn gamma_marker_round_trip_is_stable() {\n        for gamma in [0.1, 0.25, 0.5, 1.0, 2.0, 4.0] {\n            let round_trip = gamma_from_marker_fraction(gamma_marker_fraction(gamma));\n            assert!((round_trip - gamma).abs() < 0.0001);\n        }\n    }",
    "    fn gamma_marker_round_trip_is_stable() {\n        for gamma in [0.1, 0.25, 0.5, 1.0, 2.0, 4.0] {\n            let round_trip = gamma_from_marker_fraction(gamma_marker_fraction(gamma));\n            assert!((round_trip - gamma).abs() < 0.0001);\n        }\n    }\n\n    #[test]\n    fn gamma_marker_direction_matches_conventional_levels() {\n        assert!(gamma_marker_fraction(2.0) < 0.5);\n        assert_eq!(gamma_marker_fraction(1.0), 0.5);\n        assert!(gamma_marker_fraction(0.5) > 0.5);\n\n        let mut left = model::Levels::default();\n        apply_input_marker_drag(\n            &mut left,\n            LevelMarker::Gamma,\n            0.25,\n            TonalDisplayMode::Light,\n        );\n        assert!(left.gamma > 1.0);\n        assert!(model::apply_levels(0.5, left) > 0.5);\n\n        let mut right = model::Levels::default();\n        apply_input_marker_drag(\n            &mut right,\n            LevelMarker::Gamma,\n            0.75,\n            TonalDisplayMode::Light,\n        );\n        assert!(right.gamma < 1.0);\n        assert!(model::apply_levels(0.5, right) < 0.5);\n    }\n\n    #[test]\n    fn histogram_series_each_normalize_their_own_peak_to_full_height() {\n        let mut weak = [0_u32; 256];\n        weak[120] = 10;\n        let mut strong = [0_u32; 256];\n        strong[120] = 10_000;\n        let height = 118.0;\n        assert!((histogram_height_at_peak_normalized(&weak, 120, height) - height).abs() < 0.001);\n        assert!((histogram_height_at_peak_normalized(&strong, 120, height) - height).abs() < 0.001);\n    }",
)

# 3 + 4) Channels histogram: per-series normalization and four vertical divisions.
replace_once(
    "src/ui/match_color.rs",
    "fn histogram_bin_density(histogram: &[u32; 256], index: usize) -> f32 {\n    let total = histogram_total(histogram) as f32;\n    if total <= 0.0 {\n        0.0\n    } else {\n        histogram[index] as f32 / total\n    }\n}\n",
    "fn histogram_bin_density(histogram: &[u32; 256], index: usize) -> f32 {\n    let total = histogram_total(histogram) as f32;\n    if total <= 0.0 {\n        0.0\n    } else {\n        histogram[index] as f32 / total\n    }\n}\n\nfn histogram_display_height(histogram: &[u32; 256], index: usize, graph_height: f32) -> f32 {\n    let peak = histogram_peak_density(histogram).max(f32::EPSILON);\n    histogram_bin_density(histogram, index) / peak * graph_height\n}\n",
)
replace_once(
    "src/ui/match_color.rs",
    "    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);\n    painter.rect_stroke(",
    "    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);\n    for step in 1..4 {\n        let x = egui::lerp(rect.x_range(), step as f32 / 4.0);\n        painter.line_segment(\n            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],\n            egui::Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color),\n        );\n    }\n    painter.rect_stroke(",
)
replace_once(
    "src/ui/match_color.rs",
    "    let max_density = original\n        .into_iter()\n        .chain(adjusted)\n        .chain(target)\n        .map(histogram_peak_density)\n        .fold(0.0_f32, f32::max)\n        .max(f32::EPSILON);\n    let original_color = ui.visuals().weak_text_color();",
    "    let original_color = ui.visuals().weak_text_color();",
)
replace_once(
    "src/ui/match_color.rs",
    "            let h = histogram_bin_density(bins, index) / max_density * rect.height();",
    "            let h = histogram_display_height(bins, index, rect.height());",
)
replace_once(
    "src/ui/match_color.rs",
    "            let h = histogram_bin_density(bins, index) / max_density * rect.height();",
    "            let h = histogram_display_height(bins, index, rect.height());",
)
replace_once(
    "src/ui/match_color.rs",
    "                let h = histogram_bin_density(bins, index) / max_density * rect.height();",
    "                let h = histogram_display_height(bins, index, rect.height());",
)
replace_once(
    "src/ui/match_color.rs",
    "    fn histogram_density_is_independent_of_pixel_count() {\n        let small = histogram(&[(32, 2), (128, 6), (220, 2)]);\n        let large = histogram(&[(32, 200), (128, 600), (220, 200)]);\n        assert!((histogram_peak_density(&small) - histogram_peak_density(&large)).abs() < 1e-6);\n        assert!((histogram_bin_density(&small, 128) - histogram_bin_density(&large, 128)).abs() < 1e-6);\n    }",
    "    fn histogram_density_is_independent_of_pixel_count() {\n        let small = histogram(&[(32, 2), (128, 6), (220, 2)]);\n        let large = histogram(&[(32, 200), (128, 600), (220, 200)]);\n        assert!((histogram_peak_density(&small) - histogram_peak_density(&large)).abs() < 1e-6);\n        assert!((histogram_bin_density(&small, 128) - histogram_bin_density(&large, 128)).abs() < 1e-6);\n    }\n\n    #[test]\n    fn histogram_display_normalizes_each_series_to_its_own_peak() {\n        let weak = histogram(&[(128, 4)]);\n        let strong = histogram(&[(128, 4000)]);\n        let height = 105.0;\n        assert!((histogram_display_height(&weak, 128, height) - height).abs() < 0.001);\n        assert!((histogram_display_height(&strong, 128, height) - height).abs() < 0.001);\n    }",
)

# 3) Curve histogram: normalize Before and After independently.
replace_once(
    "src/ui/curve_editor.rs",
    "fn curve_histogram_colors(\n    ui: &egui::Ui,\n    accent: Option<egui::Color32>,\n    neutral_histogram: bool,\n) -> (egui::Color32, egui::Color32) {",
    "fn curve_histogram_height(histogram: &[u32; 256], index: usize, graph_height: f32) -> f32 {\n    let peak = histogram.iter().copied().max().unwrap_or(0).max(1) as f32;\n    histogram[index] as f32 / peak * graph_height\n}\n\nfn curve_histogram_colors(\n    ui: &egui::Ui,\n    accent: Option<egui::Color32>,\n    neutral_histogram: bool,\n) -> (egui::Color32, egui::Color32) {",
)
replace_once(
    "src/ui/curve_editor.rs",
    "        let max_value = histogram_before\n            .into_iter()\n            .chain(histogram_after)\n            .flat_map(|bins| bins.iter())\n            .copied()\n            .max()\n            .unwrap_or(1)\n            .max(1) as f32;\n        let (before_base, after_base) = curve_histogram_colors(ui, accent, neutral_histogram);",
    "        let (before_base, after_base) = curve_histogram_colors(ui, accent, neutral_histogram);",
)
replace_once(
    "src/ui/curve_editor.rs",
    "                    let h = *value as f32 / max_value * rect.height();",
    "                    let h = curve_histogram_height(bins, index, rect.height());",
)
# The loop value is now only used by enumerate; simplify to avoid an unused binding warning.
replace_once(
    "src/ui/curve_editor.rs",
    "                for (index, value) in bins.iter().enumerate() {",
    "                for (index, _) in bins.iter().enumerate() {",
)
# Append a focused normalization test module.
p = Path("src/ui/curve_editor.rs")
text = p.read_text(encoding="utf-8")
addition = r'''

#[cfg(test)]
mod curve_histogram_normalization_tests {
    use super::curve_histogram_height;

    #[test]
    fn each_curve_histogram_series_reaches_full_graph_height_at_its_peak() {
        let mut weak = [0_u32; 256];
        weak[64] = 3;
        let mut strong = [0_u32; 256];
        strong[64] = 30_000;
        let height = 240.0;
        assert!((curve_histogram_height(&weak, 64, height) - height).abs() < 0.001);
        assert!((curve_histogram_height(&strong, 64, height) - height).abs() < 0.001);
    }
}
'''
if "mod curve_histogram_normalization_tests" in text:
    raise SystemExit("src/ui/curve_editor.rs: normalization tests already present")
p.write_text(text.rstrip() + addition + "\n", encoding="utf-8", newline="\n")

# Distinguish this follow-up build from the prior milestone build.
replace_once("Cargo.toml", 'version = "0.21.26"', 'version = "0.21.27"')
replace_once("VERSION", "0.21.26\n", "0.21.27\n")
p = Path("Cargo.lock")
text = p.read_text(encoding="utf-8")
pattern = r'(name = "windows-shade-editor"\nversion = ")0\.21\.26("\n)'
text2, count = re.subn(pattern, r'\g<1>0.21.27\g<2>', text, count=1)
if count != 1:
    raise SystemExit(f"Cargo.lock: expected root package version once, found {count}")
p.write_text(text2, encoding="utf-8", newline="\n")

print("UI follow-up patch applied")
