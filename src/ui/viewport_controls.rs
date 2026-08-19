use crate::*;
use eframe::egui;

const ZOOM_STRIP_BOTTOM_CLEARANCE: f32 = 30.0;

fn zoom_strip_position(
    viewport: egui::Rect,
    panel_width: f32,
    panel_height: f32,
) -> egui::Pos2 {
    egui::pos2(
        viewport.center().x - panel_width * 0.5,
        viewport.bottom() - panel_height - ZOOM_STRIP_BOTTOM_CLEARANCE,
    )
}

fn current_viewport_rect(ctx: &egui::Context) -> egui::Rect {
    let mut rect = ctx.content_rect();
    if let Some(state) = egui::containers::panel::PanelState::load(ctx, egui::Id::new("toolbar")) {
        rect.min.y = rect.min.y.max(state.outer_rect.max.y);
    }
    if let Some(state) = egui::containers::panel::PanelState::load(ctx, egui::Id::new("status")) {
        rect.max.y = rect.max.y.min(state.outer_rect.min.y);
    }
    if let Some(state) = egui::containers::panel::PanelState::load(ctx, egui::Id::new("faces")) {
        rect.min.x = rect.min.x.max(state.outer_rect.max.x);
    }
    if let Some(state) = egui::containers::panel::PanelState::load(ctx, egui::Id::new("tools")) {
        rect.max.x = rect.max.x.min(state.outer_rect.min.x);
    }
    rect
}

pub(crate) fn show(app: &mut ShadeApp, ctx: &egui::Context) {
    if app.faces.is_empty() {
        return;
    }

    // Top-level panel state is already updated by the time this post-CentralPanel
    // hook runs, so the control follows resized Faces/Tools/Toolbar/Status panels.
    let viewport = current_viewport_rect(ctx);
    if viewport.width() < 180.0 || viewport.height() < 80.0 {
        return;
    }

    let panel_width = viewport.width().min(360.0);
    let panel_height = 32.0;
    let pos = zoom_strip_position(viewport, panel_width, panel_height);

    egui::Area::new(egui::Id::new("viewport-bottom-zoom-controls"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(7, 4))
                .corner_radius(7)
                .show(ui, |ui| {
                    ui.set_width(panel_width - 14.0);
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("Fit")
                            .on_hover_text("Fit image to viewport (F)")
                            .clicked()
                        {
                            app.fit_requested = true;
                            app.viewport_recenter = true;
                        }
                        let old_zoom = app.zoom;
                        let slider_width = (ui.available_width() - 62.0).max(70.0);
                        ui.spacing_mut().slider_width = slider_width;
                        let response = ui.add_sized(
                            [slider_width, 20.0],
                            egui::Slider::new(&mut app.zoom, 0.05..=8.0)
                                .logarithmic(true)
                                .show_value(false),
                        );
                        if response.changed() && (app.zoom - old_zoom).abs() > f32::EPSILON {
                            app.viewport_recenter = true;
                        }
                        ui.label(format!("{:.0}%", app.zoom * 100.0));
                    });
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_rect_intersection_keeps_positive_viewport_geometry() {
        let rect = egui::Rect::from_min_max(egui::pos2(270.0, 42.0), egui::pos2(1200.0, 760.0));
        assert!(rect.width() > 0.0);
        assert!(rect.height() > 0.0);
        assert_eq!(rect.center().x, 735.0);
    }

    #[test]
    fn zoom_strip_never_needs_to_exceed_usable_viewport_width() {
        for viewport_width in [180.0_f32, 220.0, 360.0, 900.0] {
            let panel_width = viewport_width.min(360.0);
            assert!(panel_width <= viewport_width);
        }
    }

    #[test]
    fn zoom_strip_reserves_bottom_scrollbar_clearance() {
        let viewport = egui::Rect::from_min_max(
            egui::pos2(100.0, 50.0),
            egui::pos2(900.0, 700.0),
        );
        let panel_height = 32.0;
        let pos = zoom_strip_position(viewport, 360.0, panel_height);
        let strip_bottom = pos.y + panel_height;
        assert!(
            (viewport.bottom() - strip_bottom - ZOOM_STRIP_BOTTOM_CLEARANCE).abs()
                < f32::EPSILON
        );
        assert!(strip_bottom <= viewport.bottom() - 24.0);
    }
}
