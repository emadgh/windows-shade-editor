use crate::model::{ChannelAdjustment, Levels};
use crate::settings::TonalDisplayMode;
use crate::tiff_io;
use eframe::egui;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub(crate) struct MatchColorTarget {
    pub path: PathBuf,
    pub channel_names: Vec<String>,
    pub histograms: Vec<[u32; 256]>,
}

impl MatchColorTarget {
    pub(crate) fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MatchColorReport {
    pub matched_channels: usize,
    pub zeroed_source_only_channels: usize,
    pub ignored_target_only_channels: usize,
    pub changed: bool,
}

#[derive(Default)]
struct MatchColorState {
    target: Option<MatchColorTarget>,
    overlay_visible: bool,
}

static MATCH_COLOR_STATE: OnceLock<Mutex<MatchColorState>> = OnceLock::new();

fn runtime_state() -> &'static Mutex<MatchColorState> {
    MATCH_COLOR_STATE.get_or_init(|| Mutex::new(MatchColorState::default()))
}

fn with_state<R>(read: impl FnOnce(&mut MatchColorState) -> R) -> R {
    let mut guard = runtime_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read(&mut guard)
}

pub(crate) fn target_snapshot() -> Option<MatchColorTarget> {
    with_state(|state| state.target.clone())
}

pub(crate) fn overlay_visible() -> bool {
    with_state(|state| state.overlay_visible && state.target.is_some())
}

pub(crate) fn set_overlay_visible(visible: bool) {
    with_state(|state| {
        state.overlay_visible = visible && state.target.is_some();
    });
}

pub(crate) fn clear_target() {
    with_state(|state| {
        state.target = None;
        state.overlay_visible = false;
    });
}

fn store_target(target: MatchColorTarget) {
    with_state(|state| {
        state.target = Some(target);
        state.overlay_visible = true;
    });
}

pub(crate) fn choose_target(max_preview_dimension: u32) -> Result<Option<MatchColorTarget>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("TIFF images", &["tif", "tiff"])
        .set_title("Choose Match Color target")
        .pick_file()
    else {
        return Ok(None);
    };
    load_target(&path, max_preview_dimension).map(|target| {
        store_target(target.clone());
        Some(target)
    })
}

fn load_target(path: &Path, max_preview_dimension: u32) -> Result<MatchColorTarget, String> {
    let preview = tiff_io::load_preview(path, max_preview_dimension)
        .map_err(|err| format!("Cannot load Match Color target '{}': {err}", path.display()))?;
    if preview.histograms.is_empty() {
        return Err(format!(
            "Match Color target '{}' has no readable channel histograms.",
            path.display()
        ));
    }
    Ok(MatchColorTarget {
        path: path.to_path_buf(),
        channel_names: preview.metadata.channel_names,
        histograms: preview.histograms,
    })
}

pub(crate) fn apply_histogram_match_levels(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    source_channel_names: &[String],
    source_histograms: &[[u32; 256]],
    target: &MatchColorTarget,
) -> MatchColorReport {
    let mut report = MatchColorReport {
        ignored_target_only_channels: target
            .histograms
            .len()
            .saturating_sub(source_channel_names.len()),
        ..MatchColorReport::default()
    };

    for (index, channel_name) in source_channel_names.iter().enumerate() {
        let Some(source_histogram) = source_histograms.get(index) else {
            continue;
        };
        let adjustment = adjustments.entry(channel_name.clone()).or_default();
        let previous_enabled = adjustment.enabled;
        let previous_levels = adjustment.levels;
        adjustment.enabled = true;

        if let Some(target_histogram) = target.histograms.get(index) {
            adjustment.levels = fit_levels_from_histograms(source_histogram, target_histogram);
            report.matched_channels += 1;
        } else {
            // Source-only separations must contribute no ink after Match Color.
            // Keep Curve/Mixer intact so the operation stays isolated to Levels.
            adjustment.levels = zero_output_levels();
            report.zeroed_source_only_channels += 1;
        }

        report.changed |= previous_enabled != adjustment.enabled || previous_levels != adjustment.levels;
    }

    report
}

fn zero_output_levels() -> Levels {
    Levels {
        output_black: 0.0,
        output_white: 0.0,
        ..Levels::default()
    }
}

pub(crate) fn fit_levels_from_histograms(
    source: &[u32; 256],
    target: &[u32; 256],
) -> Levels {
    if histogram_total(source) == 0 || histogram_total(target) == 0 {
        return Levels::default();
    }

    // Robust endpoints avoid letting a handful of clipped pixels dominate the fit.
    let source_low = histogram_quantile(source, 0.01);
    let source_high = histogram_quantile(source, 0.99);
    let target_low = histogram_quantile(target, 0.01);
    let target_high = histogram_quantile(target, 0.99);
    let target_mid = histogram_quantile(target, 0.50);
    let epsilon = 1.0 / 255.0;

    if target_high - target_low < epsilon {
        return Levels {
            output_black: target_mid,
            output_white: target_mid,
            ..Levels::default()
        };
    }

    if source_high - source_low < epsilon {
        // A nearly flat source cannot reproduce a distribution through Levels alone.
        // Mapping it to the target median is the least surprising editable result.
        return Levels {
            output_black: target_mid,
            output_white: target_mid,
            ..Levels::default()
        };
    }

    let mut sum_xx = 0.0_f32;
    let mut sum_xy = 0.0_f32;
    for quantile in [0.10_f32, 0.25, 0.50, 0.75, 0.90] {
        let source_value = histogram_quantile(source, quantile);
        let target_value = histogram_quantile(target, quantile);
        let x = ((source_value - source_low) / (source_high - source_low)).clamp(0.0001, 0.9999);
        let y = ((target_value - target_low) / (target_high - target_low)).clamp(0.0001, 0.9999);
        let log_x = x.ln();
        let log_y = y.ln();
        if log_x.is_finite() && log_y.is_finite() {
            sum_xx += log_x * log_x;
            sum_xy += log_x * log_y;
        }
    }

    // Levels uses y = x^(1/gamma), so the least-squares log-space slope is 1/gamma.
    let gamma = if sum_xx > f32::EPSILON {
        let inverse_gamma = sum_xy / sum_xx;
        if inverse_gamma.is_finite() && inverse_gamma > 0.0 {
            (1.0 / inverse_gamma).clamp(0.05, 10.0)
        } else {
            1.0
        }
    } else {
        1.0
    };

    let mut input_black = source_low.clamp(0.0, 0.9999);
    let mut input_white = source_high.clamp(0.0001, 1.0);
    if input_white <= input_black + 0.0001 {
        input_black = 0.0;
        input_white = 1.0;
    }

    Levels {
        input_black,
        gamma,
        input_white,
        output_black: target_low.clamp(0.0, 1.0),
        output_white: target_high.clamp(0.0, 1.0),
    }
}

fn histogram_total(histogram: &[u32; 256]) -> u64 {
    histogram.iter().map(|&count| u64::from(count)).sum()
}

fn histogram_quantile(histogram: &[u32; 256], quantile: f32) -> f32 {
    let total = histogram_total(histogram);
    if total == 0 {
        return 0.0;
    }
    let rank = (quantile.clamp(0.0, 1.0) as f64 * (total.saturating_sub(1)) as f64).round() as u64;
    let mut cumulative = 0_u64;
    for (index, &count) in histogram.iter().enumerate() {
        cumulative = cumulative.saturating_add(u64::from(count));
        if cumulative > rank {
            return index as f32 / 255.0;
        }
    }
    1.0
}

fn histogram_peak_density(histogram: &[u32; 256]) -> f32 {
    let total = histogram_total(histogram) as f32;
    if total <= 0.0 {
        return 0.0;
    }
    histogram
        .iter()
        .copied()
        .max()
        .unwrap_or(0) as f32
        / total
}

fn histogram_bin_density(histogram: &[u32; 256], index: usize) -> f32 {
    let total = histogram_total(histogram) as f32;
    if total <= 0.0 {
        0.0
    } else {
        histogram[index] as f32 / total
    }
}

pub(crate) fn target_overlay_color(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(255, 177, 66)
    } else {
        egui::Color32::from_rgb(190, 104, 0)
    }
}

pub(crate) fn draw_histogram_with_target(
    ui: &mut egui::Ui,
    original: Option<&[u32; 256]>,
    adjusted: Option<&[u32; 256]>,
    target: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    display_mode: TonalDisplayMode,
) {
    let desired = egui::vec2(ui.available_width().max(80.0), 105.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
        rect,
        2.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );

    // Histograms can come from differently-sized images. Compare probability
    // density rather than raw pixel counts so the target overlay keeps its true shape.
    let max_density = original
        .into_iter()
        .chain(adjusted)
        .chain(target)
        .map(histogram_peak_density)
        .fold(0.0_f32, f32::max)
        .max(f32::EPSILON);
    let original_color = ui.visuals().weak_text_color();
    let adjusted_color = accent.unwrap_or(ui.visuals().selection.stroke.color);

    for index in 0..256 {
        let x = egui::lerp(
            rect.x_range(),
            super::curve_editor::tonal_display_value(index as f32 / 255.0, display_mode),
        );
        if let Some(bins) = original {
            let h = histogram_bin_density(bins, index) / max_density * rect.height();
            painter.line_segment(
                [egui::pos2(x, rect.bottom()), egui::pos2(x, rect.bottom() - h)],
                egui::Stroke::new(1.0, original_color),
            );
        }
        if let Some(bins) = adjusted {
            let h = histogram_bin_density(bins, index) / max_density * rect.height();
            painter.line_segment(
                [egui::pos2(x, rect.bottom()), egui::pos2(x, rect.bottom() - h)],
                egui::Stroke::new(1.0, adjusted_color),
            );
        }
    }

    if let Some(bins) = target {
        let points = (0..256)
            .map(|index| {
                let x = egui::lerp(
                    rect.x_range(),
                    super::curve_editor::tonal_display_value(index as f32 / 255.0, display_mode),
                );
                let h = histogram_bin_density(bins, index) / max_density * rect.height();
                egui::pos2(x, rect.bottom() - h)
            })
            .collect::<Vec<_>>();
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.6, target_overlay_color(ui)),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{apply_levels, Curve};

    fn histogram(points: &[(u8, u32)]) -> [u32; 256] {
        let mut result = [0_u32; 256];
        for &(value, count) in points {
            result[value as usize] = count;
        }
        result
    }

    #[test]
    fn identical_histograms_fit_near_identity_levels() {
        let source = histogram(&[(8, 3), (64, 20), (128, 40), (192, 20), (245, 3)]);
        let levels = fit_levels_from_histograms(&source, &source);
        assert!((levels.gamma - 1.0).abs() < 0.05);
        assert!((apply_levels(0.5, levels) - 0.5).abs() < 0.08);
    }

    #[test]
    fn histogram_density_is_independent_of_pixel_count() {
        let small = histogram(&[(32, 2), (128, 6), (220, 2)]);
        let large = histogram(&[(32, 200), (128, 600), (220, 200)]);
        assert!((histogram_peak_density(&small) - histogram_peak_density(&large)).abs() < 1e-6);
        assert!((histogram_bin_density(&small, 128) - histogram_bin_density(&large, 128)).abs() < 1e-6);
    }

    #[test]
    fn source_only_channels_are_zeroed_and_target_only_channels_are_ignored() {
        let source_histograms = vec![
            histogram(&[(32, 10), (128, 30), (220, 10)]),
            histogram(&[(64, 20), (180, 20)]),
            histogram(&[(100, 40)]),
        ];
        let target = MatchColorTarget {
            path: PathBuf::from("target.tif"),
            channel_names: vec!["C".into(), "M".into()],
            histograms: vec![
                histogram(&[(24, 10), (120, 30), (210, 10)]),
                histogram(&[(40, 20), (160, 20)]),
            ],
        };
        let names = vec!["C".to_owned(), "M".to_owned(), "Spot".to_owned()];
        let mut adjustments = BTreeMap::new();
        adjustments.entry("C".to_owned()).or_insert_with(|| {
            let mut adjustment = ChannelAdjustment::default();
            adjustment.curve = Curve {
                midpoint_enabled: true,
                midpoint: 0.62,
                ..Curve::default()
            };
            adjustment
        });

        let report = apply_histogram_match_levels(
            &mut adjustments,
            &names,
            &source_histograms,
            &target,
        );
        assert_eq!(report.matched_channels, 2);
        assert_eq!(report.zeroed_source_only_channels, 1);
        assert_eq!(report.ignored_target_only_channels, 0);
        let extra = &adjustments["Spot"].levels;
        assert_eq!(extra.output_black, 0.0);
        assert_eq!(extra.output_white, 0.0);
        assert!(adjustments["C"].curve.midpoint_enabled);
        assert!((adjustments["C"].curve.midpoint - 0.62).abs() < f32::EPSILON);
    }

    #[test]
    fn target_only_channels_do_not_create_source_adjustments() {
        let source_histograms = vec![histogram(&[(80, 20), (160, 20)])];
        let target = MatchColorTarget {
            path: PathBuf::from("target-five-channel.tif"),
            channel_names: vec!["C".into(), "M".into()],
            histograms: vec![
                histogram(&[(60, 20), (150, 20)]),
                histogram(&[(20, 10), (220, 10)]),
            ],
        };
        let names = vec!["C".to_owned()];
        let mut adjustments = BTreeMap::new();
        let report = apply_histogram_match_levels(
            &mut adjustments,
            &names,
            &source_histograms,
            &target,
        );
        assert_eq!(report.matched_channels, 1);
        assert_eq!(report.ignored_target_only_channels, 1);
        assert_eq!(adjustments.len(), 1);
        assert!(adjustments.contains_key("C"));
    }
}