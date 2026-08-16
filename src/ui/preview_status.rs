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

fn updating_indicator_badge(viewport_rect: egui::Rect) -> Option<egui::Rect> {
    if viewport_rect.width() < 52.0 || viewport_rect.height() < 52.0 {
        return None;
    }
    let size = 44.0;
    Some(egui::Rect::from_center_size(
        viewport_rect.center(),
        egui::vec2(size, size),
    ))
}

pub(crate) fn paint_updating_indicator(ui: &egui::Ui, viewport_rect: egui::Rect) {
    let Some(badge) = updating_indicator_badge(viewport_rect) else {
        return;
    };
    ui.ctx().request_repaint_after(Duration::from_millis(50));
    let time = ui.ctx().input(|input| input.time) as f32;
    let painter = ui.painter();
    painter.rect_filled(badge, 10.0, egui::Color32::from_black_alpha(188));
    painter.rect_stroke(
        badge,
        10.0,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(36)),
        egui::StrokeKind::Inside,
    );
    let center = badge.center();
    let inner = 7.0;
    let outer = 14.0;
    let phase = ((time * 10.0).floor() as i32).rem_euclid(8) as usize;
    for index in 0..8usize {
        let angle = index as f32 / 8.0 * std::f32::consts::TAU;
        let direction = egui::vec2(angle.cos(), angle.sin());
        let age = (index + 8 - phase) % 8;
        let alpha = 245u8.saturating_sub((age as u8).saturating_mul(25)).max(62);
        painter.line_segment(
            [center + direction * inner, center + direction * outer],
            egui::Stroke::new(2.6, egui::Color32::from_white_alpha(alpha)),
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

    #[test]
    fn updating_indicator_is_centered_in_viewport() {
        let viewport = egui::Rect::from_min_max(egui::pos2(30.0, 50.0), egui::pos2(830.0, 650.0));
        let badge = updating_indicator_badge(viewport).expect("viewport is large enough");
        assert_eq!(badge.center(), viewport.center());
        assert_eq!(badge.size(), egui::vec2(44.0, 44.0));
    }

    #[test]
    fn updating_indicator_skips_tiny_viewports() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(40.0, 40.0));
        assert!(updating_indicator_badge(viewport).is_none());
    }
}
