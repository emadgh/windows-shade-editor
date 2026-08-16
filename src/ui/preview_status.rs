use eframe::egui;
use std::time::Duration;

pub(crate) fn current_generation_is_rendering(
    current_face: usize,
    generation: u64,
    rendered_generation: u64,
    render_busy: Option<(usize, u64)>,
) -> bool {
    rendered_generation != generation && render_busy == Some((current_face, generation))
}

pub(crate) fn paint_updating_indicator(ui: &egui::Ui, viewport_rect: egui::Rect) {
    if viewport_rect.width() < 36.0 || viewport_rect.height() < 36.0 {
        return;
    }
    ui.ctx().request_repaint_after(Duration::from_millis(50));
    let time = ui.ctx().input(|input| input.time) as f32;
    let size = 30.0;
    let badge = egui::Rect::from_min_size(
        viewport_rect.right_top() + egui::vec2(-size - 12.0, 12.0),
        egui::vec2(size, size),
    );
    let painter = ui.painter();
    painter.rect_filled(badge, 7.0, egui::Color32::from_black_alpha(165));
    let center = badge.center();
    let inner = 5.0;
    let outer = 9.0;
    let phase = ((time * 10.0).floor() as i32).rem_euclid(8) as usize;
    for index in 0..8usize {
        let angle = index as f32 / 8.0 * std::f32::consts::TAU;
        let direction = egui::vec2(angle.cos(), angle.sin());
        let age = (index + 8 - phase) % 8;
        let alpha = 235u8.saturating_sub((age as u8).saturating_mul(24)).max(58);
        painter.line_segment(
            [center + direction * inner, center + direction * outer],
            egui::Stroke::new(2.0, egui::Color32::from_white_alpha(alpha)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updating_indicator_requires_exact_current_generation() {
        assert!(current_generation_is_rendering(2, 8, 7, Some((2, 8))));
        assert!(!current_generation_is_rendering(2, 8, 8, Some((2, 8))));
        assert!(!current_generation_is_rendering(2, 8, 7, Some((1, 8))));
        assert!(!current_generation_is_rendering(2, 9, 7, Some((2, 8))));
        assert!(!current_generation_is_rendering(2, 8, 7, None));
    }
}
