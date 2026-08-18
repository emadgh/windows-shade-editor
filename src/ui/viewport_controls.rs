use crate::*;
use eframe::egui;

pub(crate) fn show(app: &mut ShadeApp, ctx: &egui::Context) {
    if app.faces.is_empty() {
        return;
    }

    // This post-pass runs after CentralPanel has been laid out, so available_rect
    // is the usable image viewport after top/bottom/left/right application panels.
    let viewport = ctx.available_rect();
    if viewport.width() < 120.0 || viewport.height() < 80.0 {
        return;
    }

    let panel_width = viewport.width().min(360.0).max(220.0);
    let panel_height = 32.0;
    let pos = egui::pos2(
        viewport.center().x - panel_width * 0.5,
        viewport.bottom() - panel_height - 8.0,
    );

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
                        let slider_width = (ui.available_width() - 62.0).max(90.0);
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
