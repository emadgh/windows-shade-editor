use super::match_color;
use crate::*;
use eframe::egui;

pub(crate) fn ui_reference_file(app: &mut ShadeApp, ui: &mut egui::Ui) {
    let ctrl_right_click = ui.ctx().input(|input| {
        input.modifiers.ctrl && input.pointer.button_clicked(egui::PointerButton::Secondary)
    });
    if ctrl_right_click && match_color::target_snapshot().is_some() {
        match_color::set_preview_visible(true);
    }

    let target = match_color::target_snapshot();
    ui.horizontal(|ui| {
        ui.strong("Reference File");
        let field_width = (ui.available_width() - 92.0).clamp(90.0, 190.0);
        let text = target
            .as_ref()
            .map(|target| target.display_name())
            .unwrap_or_else(|| "No reference".to_owned());
        let response = ui.add_sized(
            [field_width, 20.0],
            egui::Label::new(egui::RichText::new(text).small()).truncate(),
        );
        if let Some(target) = target.as_ref() {
            response.on_hover_text(format!(
                "{}\nCtrl + Right Click: quick in-app preview",
                target.path.display()
            ));
            if !target.is_available() {
                ui.colored_label(egui::Color32::YELLOW, "missing")
                    .on_hover_text("The Reference file has been moved or deleted.");
            }
        }

        if ui
            .small_button("…")
            .on_hover_text("Select or replace Reference image")
            .clicked()
        {
            match match_color::choose_target(app.settings.max_preview_dimension) {
                Ok(Some(target)) => {
                    app.report_info(format!("Reference image: {}", target.display_name()));
                }
                Ok(None) => {}
                Err(err) => app.report_error(err),
            }
        }
        if ui
            .add_enabled(
                target.as_ref().is_some_and(|item| item.is_available()),
                egui::Button::new("↗").small(),
            )
            .on_hover_text("Reveal Reference file in Explorer")
            .clicked()
        {
            if let Some(target) = target.as_ref() {
                if let Err(err) = reveal_in_explorer(&target.path) {
                    app.report_error(err);
                }
            }
        }
        if ui
            .add_enabled(target.is_some(), egui::Button::new("×").small())
            .on_hover_text("Clear Reference image and histogram; applied Match Color Levels remain")
            .clicked()
        {
            match_color::clear_target();
            app.report_info("Reference image cleared; applied adjustments were kept.");
        }
    });

    ui_reference_preview(ui.ctx());
}

fn ui_reference_preview(ctx: &egui::Context) {
    if !match_color::preview_visible() {
        return;
    }
    let Some(target) = match_color::target_snapshot() else {
        match_color::set_preview_visible(false);
        return;
    };
    let mut open = true;
    egui::Window::new(format!("Reference · {}", target.display_name()))
        .open(&mut open)
        .resizable(true)
        .default_size([720.0, 620.0])
        .show(ctx, |ui| {
            if !target.is_available() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Reference file is unavailable. Reselect it to refresh preview and histogram data.",
                );
                ui.small(target.path.display().to_string());
                return;
            }
            if target.preview_width == 0
                || target.preview_height == 0
                || target.preview_rgba.len()
                    != target
                        .preview_width
                        .saturating_mul(target.preview_height)
                        .saturating_mul(4)
            {
                ui.label("Reference preview data is unavailable.");
                return;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [target.preview_width, target.preview_height],
                &target.preview_rgba,
            );
            let texture = ctx.load_texture(
                format!("reference-preview:{}", target.path.display()),
                image,
                egui::TextureOptions::LINEAR,
            );
            let natural = texture.size_vec2();
            let available = ui.available_size().max(egui::vec2(1.0, 1.0));
            let scale = (available.x / natural.x)
                .min(available.y / natural.y)
                .min(1.0);
            ui.centered_and_justified(|ui| {
                ui.add(egui::Image::from_texture(&texture).fit_to_exact_size(natural * scale));
            });
        });
    match_color::set_preview_visible(open);
}
