use super::curve_editor::tonal_display_value;
use crate::model::{self, ChannelAdjustment};
use crate::palette::ChannelPalette;
use crate::settings::TonalDisplayMode;
use crate::{channel_color, channel_display_name, with_accent};
use eframe::egui;

const LEVEL_SAMPLE_MAX: f32 = 255.0;
const INPUT_BLACK_MAX_SAMPLE: i32 = 250;
const INPUT_WHITE_MIN_SAMPLE: i32 = 5;
const INPUT_MIN_GAP_SAMPLES: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum LevelMarker {
    Black,
    Gamma,
    White,
}

fn level_to_sample(value: f32) -> i32 {
    (value.clamp(0.0, 1.0) * LEVEL_SAMPLE_MAX).round() as i32
}

fn sample_to_level(value: i32) -> f32 {
    (value as f32 / LEVEL_SAMPLE_MAX).clamp(0.0, 1.0)
}

fn quantize_level(value: f32) -> f32 {
    sample_to_level(level_to_sample(value))
}

fn coefficient_to_percent(value: f32, min: i32, max: i32) -> i32 {
    (value * 100.0).round().clamp(min as f32, max as f32) as i32
}

fn percent_to_coefficient(value: i32) -> f32 {
    value as f32 / 100.0
}

fn gamma_marker_fraction(gamma: f32) -> f32 {
    let gamma = gamma.clamp(0.1, 4.0);
    if gamma <= 1.0 {
        0.5 * (gamma / 0.1).ln() / 10.0_f32.ln()
    } else {
        0.5 + 0.5 * gamma.ln() / 4.0_f32.ln()
    }
}

fn gamma_from_marker_fraction(fraction: f32) -> f32 {
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= 0.5 {
        0.1 * 10.0_f32.powf(fraction * 2.0)
    } else {
        4.0_f32.powf((fraction - 0.5) * 2.0)
    }
    .clamp(0.1, 4.0)
}

fn display_to_working(value: f32, mode: TonalDisplayMode) -> f32 {
    tonal_display_value(value, mode)
}

fn legend_dot(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
    ui.small(label);
}

fn draw_levels_histogram(
    ui: &mut egui::Ui,
    before: Option<&[u32; 256]>,
    after: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    display_mode: TonalDisplayMode,
) {
    let before_color = ui.visuals().weak_text_color().gamma_multiply(0.72);
    let after_color = accent.unwrap_or(ui.visuals().selection.stroke.color);
    ui.horizontal(|ui| {
        ui.strong("Histogram");
        ui.small(format!("Mode: {}", display_mode.label()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            legend_dot(ui, after_color, "After");
            legend_dot(ui, before_color, "Before");
        });
    });

    let width = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 118.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
        rect,
        3.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    for step in 1..4 {
        let x = egui::lerp(rect.x_range(), step as f32 / 4.0);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
    }
    let max_value = before
        .into_iter()
        .flat_map(|bins| bins.iter())
        .chain(after.into_iter().flat_map(|bins| bins.iter()))
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    for index in 0..256 {
        let x = egui::lerp(
            rect.x_range(),
            tonal_display_value(index as f32 / 255.0, display_mode),
        );
        if let Some(bins) = before {
            let h = bins[index] as f32 / max_value * rect.height();
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.bottom() - h),
                ],
                egui::Stroke::new(1.0, before_color),
            );
        }
        if let Some(bins) = after {
            let h = bins[index] as f32 / max_value * rect.height();
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.bottom() - h),
                ],
                egui::Stroke::new(1.15, after_color),
            );
        }
    }
}

fn paint_triangle(
    painter: &egui::Painter,
    x: f32,
    top: f32,
    fill: egui::Color32,
    stroke: egui::Color32,
) {
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x, top),
            egui::pos2(x - 5.0, top + 8.0),
            egui::pos2(x + 5.0, top + 8.0),
        ],
        fill,
        egui::Stroke::new(1.0, stroke),
    ));
}

fn input_marker_positions(
    levels: model::Levels,
    display_mode: TonalDisplayMode,
) -> [(LevelMarker, f32); 3] {
    let gamma_working = egui::lerp(
        levels.input_black..=levels.input_white,
        gamma_marker_fraction(levels.gamma),
    );
    [
        (
            LevelMarker::Black,
            tonal_display_value(levels.input_black, display_mode),
        ),
        (
            LevelMarker::Gamma,
            tonal_display_value(gamma_working, display_mode),
        ),
        (
            LevelMarker::White,
            tonal_display_value(levels.input_white, display_mode),
        ),
    ]
}

fn apply_input_marker_drag(
    levels: &mut model::Levels,
    marker: LevelMarker,
    display_fraction: f32,
    display_mode: TonalDisplayMode,
) {
    let working = quantize_level(display_to_working(display_fraction, display_mode));
    let gap = INPUT_MIN_GAP_SAMPLES as f32 / 255.0;
    match marker {
        LevelMarker::Black => {
            levels.input_black = working.clamp(0.0, (levels.input_white - gap).max(0.0));
        }
        LevelMarker::White => {
            levels.input_white = working.clamp((levels.input_black + gap).min(1.0), 1.0);
        }
        LevelMarker::Gamma => {
            let span = (levels.input_white - levels.input_black).max(gap);
            let fraction = ((working - levels.input_black) / span).clamp(0.0, 1.0);
            levels.gamma = gamma_from_marker_fraction(fraction);
        }
    }
}

fn input_levels_marker_strip(
    ui: &mut egui::Ui,
    levels: &mut model::Levels,
    display_mode: TonalDisplayMode,
) -> bool {
    let before = *levels;
    let width = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
    let graph_id = ui.make_persistent_id("levels-input-marker-strip");

    for (marker, display) in input_marker_positions(*levels, display_mode) {
        let x = egui::lerp(rect.x_range(), display);
        let hit =
            egui::Rect::from_center_size(egui::pos2(x, rect.top() + 9.0), egui::vec2(24.0, 22.0));
        let response = ui.interact(hit, graph_id.with(marker), egui::Sense::click_and_drag());
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let display_fraction = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                apply_input_marker_drag(levels, marker, display_fraction, display_mode);
            }
        }
    }

    let painter = ui.painter();
    let y = rect.top() + 2.0;
    painter.line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    let stroke = ui.visuals().widgets.noninteractive.fg_stroke.color;
    for (marker, display) in input_marker_positions(*levels, display_mode) {
        let fill = match marker {
            LevelMarker::Black => egui::Color32::BLACK,
            LevelMarker::Gamma => ui.visuals().weak_text_color(),
            LevelMarker::White => egui::Color32::WHITE,
        };
        paint_triangle(
            painter,
            egui::lerp(rect.x_range(), display),
            y + 2.0,
            fill,
            stroke,
        );
    }
    *levels != before
}

fn paint_output_gradient(
    painter: &egui::Painter,
    rect: egui::Rect,
    display_mode: TonalDisplayMode,
) {
    let steps = 64;
    for step in 0..steps {
        let x0 = egui::lerp(rect.x_range(), step as f32 / steps as f32);
        let x1 = egui::lerp(rect.x_range(), (step + 1) as f32 / steps as f32);
        let display = (step as f32 + 0.5) / steps as f32;
        let working = display_to_working(display, display_mode);
        let gray = (working * 255.0).round() as u8;
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            egui::Color32::from_gray(gray),
        );
    }
}

fn output_levels_strip(
    ui: &mut egui::Ui,
    levels: &mut model::Levels,
    display_mode: TonalDisplayMode,
    width: f32,
) -> bool {
    let before = *levels;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(80.0), 22.0), egui::Sense::hover());
    let id = ui.make_persistent_id("levels-output-marker-strip");
    let positions = [
        (
            LevelMarker::Black,
            tonal_display_value(levels.output_black, display_mode),
        ),
        (
            LevelMarker::White,
            tonal_display_value(levels.output_white, display_mode),
        ),
    ];

    for (marker, display) in positions {
        let x = egui::lerp(rect.x_range(), display);
        let hit = egui::Rect::from_center_size(
            egui::pos2(x, rect.bottom() - 6.0),
            egui::vec2(24.0, 22.0),
        );
        let response = ui.interact(hit, id.with(marker), egui::Sense::click_and_drag());
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let display_fraction = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let working = quantize_level(display_to_working(display_fraction, display_mode));
                match marker {
                    LevelMarker::Black => levels.output_black = working,
                    LevelMarker::White => levels.output_white = working,
                    LevelMarker::Gamma => {}
                }
            }
        }
    }

    let painter = ui.painter();
    let bar = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 1.0),
        egui::pos2(rect.right(), rect.top() + 12.0),
    );
    paint_output_gradient(painter, bar, display_mode);
    painter.rect_stroke(
        bar,
        0.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let stroke = ui.visuals().widgets.noninteractive.fg_stroke.color;
    for (marker, display) in [
        (
            LevelMarker::Black,
            tonal_display_value(levels.output_black, display_mode),
        ),
        (
            LevelMarker::White,
            tonal_display_value(levels.output_white, display_mode),
        ),
    ] {
        let fill = if marker == LevelMarker::Black {
            egui::Color32::BLACK
        } else {
            egui::Color32::WHITE
        };
        paint_triangle(
            painter,
            egui::lerp(rect.x_range(), display),
            rect.top() + 13.0,
            fill,
            stroke,
        );
    }

    *levels != before
}

pub(crate) fn levels_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    histogram_before: Option<&[u32; 256]>,
    histogram_after: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    display_mode: TonalDisplayMode,
) -> bool {
    with_accent(ui, accent, |ui| {
        draw_levels_histogram(ui, histogram_before, histogram_after, accent, display_mode);
        ui.add_space(5.0);
        let levels = &mut adjustment.levels;
        let before = *levels;

        ui.horizontal(|ui| {
            ui.strong("Input Levels");
            ui.small("drag markers or edit 0–255 values");
        });
        input_levels_marker_strip(ui, levels, display_mode);

        let mut black = level_to_sample(levels.input_black);
        let mut gamma = levels.gamma;
        let mut white = level_to_sample(levels.input_white);
        let mut black_changed = false;
        let mut white_changed = false;
        ui.columns(3, |columns| {
            columns[0].small("Black");
            black_changed = columns[0]
                .add(
                    egui::DragValue::new(&mut black)
                        .range(0..=INPUT_BLACK_MAX_SAMPLE)
                        .speed(1.0),
                )
                .changed();
            columns[1].small("Midtone / Gamma");
            columns[1].add(
                egui::DragValue::new(&mut gamma)
                    .range(0.1..=4.0)
                    .speed(0.01),
            );
            columns[2].small("White");
            white_changed = columns[2]
                .add(
                    egui::DragValue::new(&mut white)
                        .range(INPUT_WHITE_MIN_SAMPLE..=255)
                        .speed(1.0),
                )
                .changed();
        });
        black = black.clamp(0, INPUT_BLACK_MAX_SAMPLE);
        white = white.clamp(INPUT_WHITE_MIN_SAMPLE, 255);
        if white - black < INPUT_MIN_GAP_SAMPLES {
            if black_changed && !white_changed {
                black = (white - INPUT_MIN_GAP_SAMPLES).max(0);
            } else {
                white = (black + INPUT_MIN_GAP_SAMPLES).min(255);
            }
        }
        levels.input_black = sample_to_level(black);
        levels.gamma = gamma.clamp(0.1, 4.0);
        levels.input_white = sample_to_level(white);

        ui.add_space(7.0);
        ui.horizontal(|ui| {
            ui.strong("Output Levels:");
            let mut output_black = level_to_sample(levels.output_black);
            let mut output_white = level_to_sample(levels.output_white);
            if ui
                .add_sized(
                    [44.0, 20.0],
                    egui::DragValue::new(&mut output_black)
                        .range(0..=255)
                        .speed(1.0),
                )
                .changed()
            {
                levels.output_black = sample_to_level(output_black);
            }
            let strip_width = (ui.available_width() - 52.0).max(84.0);
            output_levels_strip(ui, levels, display_mode, strip_width);
            if ui
                .add_sized(
                    [44.0, 20.0],
                    egui::DragValue::new(&mut output_white)
                        .range(0..=255)
                        .speed(1.0),
                )
                .changed()
            {
                levels.output_white = sample_to_level(output_white);
            }
        });

        *levels != before
    })
}

#[derive(Clone, Copy, Debug)]
struct MixerRowLayout {
    label_width: f32,
    slider_width: f32,
    value_width: f32,
}

fn mixer_row_layout(available_width: f32, spacing: f32) -> MixerRowLayout {
    let value_width = 58.0;
    let label_width = (available_width * 0.22).clamp(72.0, 112.0);
    let reserved = label_width + value_width + spacing * 2.0;
    let slider_width = (available_width - reserved).max(72.0);
    MixerRowLayout {
        label_width,
        slider_width,
        value_width,
    }
}

fn mixer_percent_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    min_percent: i32,
    max_percent: i32,
    color: Option<egui::Color32>,
) -> bool {
    let mut percent = coefficient_to_percent(*value, min_percent, max_percent);
    let before = percent;
    with_accent(ui, color, |ui| {
        let row_width = ui.available_width().max(220.0);
        let spacing = ui.spacing().item_spacing.x;
        let layout = mixer_row_layout(row_width, spacing);
        ui.allocate_ui_with_layout(
            egui::vec2(row_width, 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let label_widget = if let Some(color) = color {
                    egui::Label::new(egui::RichText::new(label).color(color)).truncate()
                } else {
                    egui::Label::new(label).truncate()
                };
                ui.add_sized([layout.label_width, 20.0], label_widget);
                ui.add_sized(
                    [layout.slider_width, 20.0],
                    egui::Slider::new(&mut percent, min_percent..=max_percent)
                        .step_by(1.0)
                        .show_value(false)
                        .trailing_fill(true),
                );
                ui.add_sized(
                    [layout.value_width, 20.0],
                    egui::DragValue::new(&mut percent)
                        .range(min_percent..=max_percent)
                        .speed(1.0)
                        .suffix("%"),
                );
            },
        );
    });
    if percent != before {
        *value = percent_to_coefficient(percent);
        true
    } else {
        false
    }
}

pub(crate) fn mixer_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    output_name: &str,
    channel_names: &[String],
    accent: Option<egui::Color32>,
    palette: Option<&ChannelPalette>,
) -> bool {
    let output_index = channel_names
        .iter()
        .position(|name| name == output_name)
        .unwrap_or(0);
    let output_display = channel_display_name(palette, output_name, output_index);
    if let Some(color) = accent {
        ui.colored_label(color, format!("Output: {output_display}"));
    } else {
        ui.label(format!("Output: {output_display}"));
    }
    ui.add_space(4.0);
    let mut changed = false;
    for (index, name) in channel_names.iter().enumerate() {
        let default = if name == output_name { 1.0 } else { 0.0 };
        let coefficient = adjustment
            .mixer
            .coefficients
            .entry(name.clone())
            .or_insert(default);
        let row_color = accent.map(|_| channel_color(palette, name, index));
        changed |= mixer_percent_row(
            ui,
            channel_display_name(palette, name, index),
            coefficient,
            -200,
            200,
            row_color,
        );
    }
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(5.0);
    changed |= mixer_percent_row(
        ui,
        "Constant",
        &mut adjustment.mixer.constant,
        -100,
        100,
        accent,
    );
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_row_slider_expands_with_panel_width() {
        let narrow = mixer_row_layout(280.0, 8.0);
        let wide = mixer_row_layout(520.0, 8.0);
        assert!(narrow.slider_width >= 72.0);
        assert!(wide.slider_width > narrow.slider_width + 180.0);
        assert_eq!(narrow.value_width, wide.value_width);
        assert!(wide.label_width >= narrow.label_width);
    }

    #[test]
    fn mixer_percent_round_trip_matches_normalized_storage() {
        for (coefficient, expected) in [
            (-2.0, -200),
            (-0.25, -25),
            (0.0, 0),
            (0.05, 5),
            (0.9, 90),
            (1.0, 100),
            (2.0, 200),
        ] {
            let percent = coefficient_to_percent(coefficient, -200, 200);
            assert_eq!(percent, expected);
            assert!((percent_to_coefficient(percent) - coefficient).abs() < 0.0001);
        }
    }

    #[test]
    fn level_sample_scale_round_trips_endpoints_and_midpoint() {
        for value in [0.0, 0.5, 1.0] {
            let round_trip = sample_to_level(level_to_sample(value));
            assert!((round_trip - value).abs() <= 1.0 / 255.0);
        }
    }

    #[test]
    fn gamma_marker_round_trip_is_stable() {
        for gamma in [0.1, 0.25, 0.5, 1.0, 2.0, 4.0] {
            let round_trip = gamma_from_marker_fraction(gamma_marker_fraction(gamma));
            assert!((round_trip - gamma).abs() < 0.0001);
        }
    }

    #[test]
    fn dragged_input_markers_quantize_to_sample_units_and_keep_gap() {
        let mut levels = model::Levels::default();
        apply_input_marker_drag(
            &mut levels,
            LevelMarker::Black,
            0.8,
            TonalDisplayMode::Light,
        );
        assert!(levels.input_white - levels.input_black >= INPUT_MIN_GAP_SAMPLES as f32 / 255.0);
        let sample = level_to_sample(levels.input_black);
        assert!((levels.input_black - sample_to_level(sample)).abs() < 0.0001);
    }
}
