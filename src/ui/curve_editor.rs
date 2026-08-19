use crate::*;
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CurvePointKind {
    Black,
    Midpoint,
    White,
}

impl CurvePointKind {
    fn label(self) -> &'static str {
        match self {
            Self::Black => "Black point",
            Self::Midpoint => "Midpoint",
            Self::White => "White point",
        }
    }
}

fn curve_point_xy(curve: model::Curve, point: CurvePointKind) -> (f32, f32) {
    match point {
        CurvePointKind::Black => (curve.input_black, curve.black),
        CurvePointKind::Midpoint => (curve.midpoint_input, curve.midpoint),
        CurvePointKind::White => (curve.input_white, curve.white),
    }
}

fn set_curve_point(curve: &mut model::Curve, point: CurvePointKind, input: f32, output: f32) {
    let gap = 1.0 / 255.0;
    let output = output.clamp(0.0, 1.0);
    match point {
        CurvePointKind::Black => {
            let max_input = if curve.midpoint_enabled {
                (curve.midpoint_input - gap).max(0.0)
            } else {
                (curve.input_white - gap).max(0.0)
            };
            curve.input_black = input.clamp(0.0, max_input);
            curve.black = output;
        }
        CurvePointKind::Midpoint => {
            curve.midpoint_input = input.clamp(
                (curve.input_black + gap).min(1.0),
                (curve.input_white - gap).max(0.0),
            );
            curve.midpoint = output;
        }
        CurvePointKind::White => {
            let min_input = if curve.midpoint_enabled {
                (curve.midpoint_input + gap).min(1.0)
            } else {
                (curve.input_black + gap).min(1.0)
            };
            curve.input_white = input.clamp(min_input, 1.0);
            curve.white = output;
        }
    }
}

pub(crate) fn tonal_display_value(value: f32, mode: TonalDisplayMode) -> f32 {
    match mode {
        TonalDisplayMode::Light => value.clamp(0.0, 1.0),
        TonalDisplayMode::Pigment => 1.0 - value.clamp(0.0, 1.0),
    }
}

fn tonal_working_value(value: f32, mode: TonalDisplayMode) -> f32 {
    // Light and Pigment are inverse presentation transforms. Pigment mirrors
    // both axes while keeping all production adjustment math in working space.
    tonal_display_value(value, mode)
}

fn curve_histogram_height(histogram: &[u32; 256], index: usize, graph_height: f32) -> f32 {
    let peak = histogram.iter().copied().max().unwrap_or(0).max(1) as f32;
    histogram[index] as f32 / peak * graph_height
}

fn curve_histogram_colors(
    ui: &egui::Ui,
    accent: Option<egui::Color32>,
    neutral_histogram: bool,
) -> (egui::Color32, egui::Color32) {
    let before = ui.visuals().weak_text_color().gamma_multiply(0.72);
    let after = if neutral_histogram {
        ui.visuals().weak_text_color()
    } else {
        accent.unwrap_or(ui.visuals().selection.stroke.color)
    };
    (before, after)
}

fn curve_legend_dot(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
    ui.small(label);
}

fn curve_point_screen(
    rect: egui::Rect,
    input: f32,
    output: f32,
    mode: TonalDisplayMode,
) -> egui::Pos2 {
    let display_input = tonal_display_value(input, mode);
    let display_output = tonal_display_value(output, mode);
    egui::pos2(
        egui::lerp(rect.x_range(), display_input),
        egui::lerp(rect.bottom()..=rect.top(), display_output),
    )
}

fn nudge_curve_point(
    curve: &mut model::Curve,
    selected: CurvePointKind,
    horizontal_units: i32,
    vertical_units: i32,
    mode: TonalDisplayMode,
) {
    let (input, output) = curve_point_xy(*curve, selected);
    let mut display_input = tonal_display_value(input, mode);
    let mut display_output = tonal_display_value(output, mode);
    display_input += horizontal_units as f32 / 255.0;
    display_output += vertical_units as f32 / 255.0;
    set_curve_point(
        curve,
        selected,
        tonal_working_value(display_input, mode),
        tonal_working_value(display_output, mode),
    );
}

fn remove_selected_curve_point(
    curve: &mut model::Curve,
    selected: CurvePointKind,
) -> (CurvePointKind, bool) {
    if selected == CurvePointKind::Midpoint && curve.midpoint_enabled {
        curve.midpoint_enabled = false;
        (CurvePointKind::Black, true)
    } else {
        (selected, false)
    }
}

fn reset_selected_curve_point(curve: &mut model::Curve, selected: CurvePointKind) {
    match selected {
        CurvePointKind::Black => set_curve_point(curve, selected, 0.0, 0.0),
        CurvePointKind::Midpoint => {
            let input = curve.midpoint_input;
            set_curve_point(curve, selected, input, input);
        }
        CurvePointKind::White => set_curve_point(curve, selected, 1.0, 1.0),
    }
}

fn curve_editor_graph(
    ui: &mut egui::Ui,
    curve: &mut model::Curve,
    histogram_before: Option<&[u32; 256]>,
    histogram_after: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    neutral_histogram: bool,
    display_mode: TonalDisplayMode,
) -> (bool, CurvePointKind) {
    // Match the Levels visual language: the histogram/curve plot is always square.
    let side = ui.available_width().min(340.0).max(150.0);
    let desired = egui::vec2(side, side);
    let (rect, graph_response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let graph_id = ui.make_persistent_id("three-point-curve-editor");
    let selection_id = graph_id.with("selected-point");
    let mut selected = ui
        .data(|data| data.get_temp::<CurvePointKind>(selection_id))
        .unwrap_or(CurvePointKind::Black);
    if !curve.midpoint_enabled && selected == CurvePointKind::Midpoint {
        selected = CurvePointKind::Black;
    }
    let mut changed = false;
    if graph_response.clicked() {
        graph_response.request_focus();
    }
    let mut midpoint_removed_this_frame = false;
    let points = [
        CurvePointKind::Black,
        CurvePointKind::Midpoint,
        CurvePointKind::White,
    ];

    for point in points {
        if point == CurvePointKind::Midpoint && !curve.midpoint_enabled {
            continue;
        }
        let (input, output) = curve_point_xy(*curve, point);
        let center = curve_point_screen(rect, input, output, display_mode);
        let hit_rect = egui::Rect::from_center_size(center, egui::vec2(22.0, 22.0));
        let response = ui.interact(
            hit_rect,
            graph_id.with(point),
            egui::Sense::click_and_drag(),
        );
        if point == CurvePointKind::Midpoint && response.double_clicked() {
            let (next, removed) = remove_selected_curve_point(curve, point);
            midpoint_removed_this_frame = removed;
            selected = next;
            ui.data_mut(|data| data.insert_temp(selection_id, selected));
            changed |= removed;
            continue;
        }
        if response.clicked() || response.drag_started() {
            selected = point;
            ui.data_mut(|data| data.insert_temp(selection_id, point));
            graph_response.request_focus();
        }
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let display_input = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let display_output = ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0);
                set_curve_point(
                    curve,
                    point,
                    tonal_working_value(display_input, display_mode),
                    tonal_working_value(display_output, display_mode),
                );
                selected = point;
                ui.data_mut(|data| data.insert_temp(selection_id, point));
                changed = true;
            }
        }
    }

    if !curve.midpoint_enabled && !midpoint_removed_this_frame && graph_response.double_clicked() {
        if let Some(pointer) = graph_response.interact_pointer_pos() {
            let display_input = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let input = tonal_working_value(display_input, display_mode);
            let gap = 1.0 / 255.0;
            if input > curve.input_black + gap && input < curve.input_white - gap {
                let output = model::curve_linear_output(input, *curve);
                let line_point = curve_point_screen(rect, input, output, display_mode);
                if pointer.distance(line_point) <= 16.0 {
                    curve.midpoint_enabled = true;
                    curve.midpoint_input = input;
                    curve.midpoint = output;
                    selected = CurvePointKind::Midpoint;
                    ui.data_mut(|data| data.insert_temp(selection_id, selected));
                    changed = true;
                }
            }
        }
    }

    if graph_response.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                graph_response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let (left, right, up, down, shift, delete, home) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.modifiers.shift,
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace),
                input.key_pressed(egui::Key::Home),
            )
        });
        if delete {
            let (next, removed) = remove_selected_curve_point(curve, selected);
            if removed {
                selected = next;
                ui.data_mut(|data| data.insert_temp(selection_id, selected));
                changed = true;
            }
        } else if home {
            reset_selected_curve_point(curve, selected);
            changed = true;
        } else if left || right || up || down {
            // Focus navigation is decided at the start of the frame, before this
            // custom graph sees the key event. Cancel that pending movement so the
            // first arrow press after selecting a point cannot escape the graph.
            ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
            let units = if shift { 10 } else { 1 };
            let horizontal = if left {
                -units
            } else if right {
                units
            } else {
                0
            };
            let vertical = if down {
                -units
            } else if up {
                units
            } else {
                0
            };
            nudge_curve_point(curve, selected, horizontal, vertical, display_mode);
            changed = true;
        }
    }

    let painter = ui.painter_at(rect);
    // Match the Levels histogram surface: dark graph background with a subtle 4x4 grid.
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let grid_color = ui
        .visuals()
        .widgets
        .noninteractive
        .bg_stroke
        .color
        .gamma_multiply(0.62);
    for step in 1..4 {
        let fraction = step as f32 / 4.0;
        let x = egui::lerp(rect.x_range(), fraction);
        let y = egui::lerp(rect.y_range(), fraction);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(0.7, grid_color),
        );
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(0.7, grid_color),
        );
    }
    painter.rect_stroke(
        rect,
        2.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    if histogram_before.is_some() || histogram_after.is_some() {
        let (before_base, after_base) = curve_histogram_colors(ui, accent, neutral_histogram);
        let before_color = before_base.gamma_multiply(0.32);
        let after_color = after_base.gamma_multiply(0.56);
        for (bins, color) in [
            (histogram_before, before_color),
            (histogram_after, after_color),
        ] {
            if let Some(bins) = bins {
                for (index, _) in bins.iter().enumerate() {
                    let x = egui::lerp(
                        rect.x_range(),
                        tonal_display_value(index as f32 / 255.0, display_mode),
                    );
                    let h = curve_histogram_height(bins, index, rect.height());
                    painter.line_segment(
                        [
                            egui::pos2(x, rect.bottom()),
                            egui::pos2(x, rect.bottom() - h),
                        ],
                        egui::Stroke::new(1.0, color),
                    );
                }
            }
        }
    }
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom()),
            egui::pos2(rect.right(), rect.top()),
        ],
        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
    );
    let curve_color = accent.unwrap_or(ui.visuals().selection.stroke.color);
    let mut last = None;
    for step in 0..=128 {
        let x = step as f32 / 128.0;
        let y = model::apply_curve(x, *curve);
        let point = curve_point_screen(rect, x, y, display_mode);
        if let Some(previous) = last {
            painter.line_segment([previous, point], egui::Stroke::new(2.0, curve_color));
        }
        last = Some(point);
    }
    for point in points {
        if point == CurvePointKind::Midpoint && !curve.midpoint_enabled {
            continue;
        }
        let (input, output) = curve_point_xy(*curve, point);
        let center = curve_point_screen(rect, input, output, display_mode);
        let is_selected = point == selected;
        let radius = if is_selected { 6.5 } else { 5.0 };
        let fill = if is_selected {
            curve_color
        } else {
            ui.visuals().extreme_bg_color
        };
        painter.circle_filled(center, radius, fill);
        painter.circle_stroke(center, radius, egui::Stroke::new(2.0, curve_color));
    }
    if graph_response.has_focus() {
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("shade-editor-curve-graph-focused"), true);
        });
    }
    (changed, selected)
}

fn curve_point_fields(
    ui: &mut egui::Ui,
    curve: &mut model::Curve,
    selected: CurvePointKind,
    display_mode: TonalDisplayMode,
) -> bool {
    let (input, output) = curve_point_xy(*curve, selected);
    let mut input_value = (tonal_display_value(input, display_mode) * 255.0).round() as i32;
    let mut output_value = (tonal_display_value(output, display_mode) * 255.0).round() as i32;
    ui.strong(selected.label());
    let mut input_changed = false;
    let mut output_changed = false;
    ui.columns(2, |columns| {
        columns[0].small("Input");
        input_changed = columns[0]
            .add(
                egui::DragValue::new(&mut input_value)
                    .range(0..=255)
                    .speed(1),
            )
            .changed();
        columns[1].small("Output");
        output_changed = columns[1]
            .add(
                egui::DragValue::new(&mut output_value)
                    .range(0..=255)
                    .speed(1),
            )
            .changed();
    });
    if input_changed || output_changed {
        set_curve_point(
            curve,
            selected,
            tonal_working_value(input_value as f32 / 255.0, display_mode),
            tonal_working_value(output_value as f32 / 255.0, display_mode),
        );
        true
    } else {
        false
    }
}

pub(crate) fn curves_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    histogram_before: Option<&[u32; 256]>,
    histogram_after: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    display_mode: TonalDisplayMode,
    compact_controls: bool,
    neutral_histogram: bool,
) -> bool {
    with_accent(ui, accent, |ui| {
        if histogram_before.is_some() || histogram_after.is_some() {
            let (before_color, after_color) = curve_histogram_colors(ui, accent, neutral_histogram);
            ui.horizontal(|ui| {
                ui.strong("Histogram");
                ui.small(format!("Mode: {}", display_mode.label()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    curve_legend_dot(ui, after_color, "After");
                    curve_legend_dot(ui, before_color, "Before");
                });
            });
        }
        let (graph_changed, selected) = curve_editor_graph(
            ui,
            &mut adjustment.curve,
            histogram_before,
            histogram_after,
            accent,
            neutral_histogram,
            display_mode,
        );
        let mut changed = graph_changed;
        if !compact_controls {
            ui.add_space(6.0);
            changed |= curve_point_fields(ui, &mut adjustment.curve, selected, display_mode);
        }
        changed
    })
}

#[cfg(test)]
mod curve_qol_tests {
    use super::*;

    #[test]
    fn delete_only_removes_optional_midpoint_and_keeps_selection_valid() {
        let mut curve = model::Curve {
            midpoint_enabled: true,
            midpoint_input: 0.5,
            midpoint: 0.6,
            ..Default::default()
        };
        let (selected, changed) = remove_selected_curve_point(&mut curve, CurvePointKind::Midpoint);
        assert!(changed);
        assert!(!curve.midpoint_enabled);
        assert_eq!(selected, CurvePointKind::Black);

        let before = curve;
        let (selected, changed) = remove_selected_curve_point(&mut curve, CurvePointKind::White);
        assert!(!changed);
        assert_eq!(selected, CurvePointKind::White);
        assert_eq!(curve, before);
    }

    #[test]
    fn home_returns_selected_point_to_identity_without_breaking_input_order() {
        let mut curve = model::Curve {
            input_black: 0.10,
            black: 0.25,
            midpoint_enabled: true,
            midpoint_input: 0.55,
            midpoint: 0.75,
            input_white: 0.90,
            white: 0.80,
        };
        reset_selected_curve_point(&mut curve, CurvePointKind::Midpoint);
        assert!((curve.midpoint - curve.midpoint_input).abs() < f32::EPSILON);
        assert!(curve.input_black < curve.midpoint_input);
        assert!(curve.midpoint_input < curve.input_white);

        reset_selected_curve_point(&mut curve, CurvePointKind::Black);
        assert_eq!((curve.input_black, curve.black), (0.0, 0.0));
        reset_selected_curve_point(&mut curve, CurvePointKind::White);
        assert_eq!((curve.input_white, curve.white), (1.0, 1.0));
    }
}

#[cfg(test)]
mod tonal_curve_interaction_tests {
    use super::{CurvePointKind, TonalDisplayMode, nudge_curve_point, tonal_display_value};
    use crate::model::Curve;

    #[test]
    fn light_and_pigment_are_inverse_display_coordinates() {
        assert!((tonal_display_value(0.25, TonalDisplayMode::Light) - 0.25).abs() < 1e-6);
        assert!((tonal_display_value(0.25, TonalDisplayMode::Pigment) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn arrow_nudge_is_one_unit_and_shift_nudge_can_be_ten() {
        let mut curve = Curve::default();
        curve.midpoint_enabled = true;
        nudge_curve_point(
            &mut curve,
            CurvePointKind::Midpoint,
            1,
            0,
            TonalDisplayMode::Light,
        );
        assert!((curve.midpoint_input - (0.5 + 1.0 / 255.0)).abs() < 1e-5);
        nudge_curve_point(
            &mut curve,
            CurvePointKind::Midpoint,
            0,
            10,
            TonalDisplayMode::Light,
        );
        assert!((curve.midpoint - (0.5 + 10.0 / 255.0)).abs() < 1e-5);
    }

    #[test]
    fn pigment_arrow_direction_matches_the_mirrored_graph() {
        let mut curve = Curve::default();
        curve.midpoint_enabled = true;
        nudge_curve_point(
            &mut curve,
            CurvePointKind::Midpoint,
            1,
            10,
            TonalDisplayMode::Pigment,
        );
        assert!((curve.midpoint_input - (0.5 - 1.0 / 255.0)).abs() < 1e-5);
        assert!((curve.midpoint - (0.5 - 10.0 / 255.0)).abs() < 1e-5);
    }
}

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
