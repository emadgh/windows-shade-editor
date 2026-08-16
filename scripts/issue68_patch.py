from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if new in text:
        return text
    if old not in text:
        raise SystemExit(f"{label}: expected source pattern not found")
    return text.replace(old, new, 1)


# Version for this independent UI/QoL slice.
Path("VERSION").write_text("0.20.7\n", encoding="utf-8")
p = Path("Cargo.toml")
s = p.read_text(encoding="utf-8")
s = s.replace('version = "0.20.6"', 'version = "0.20.7"', 1)
p.write_text(s, encoding="utf-8")

# Responsive Mixer row: explicitly allocate the complete row width before laying out children.
p = Path("src/ui/levels_mixer.rs")
s = p.read_text(encoding="utf-8")
anchor = '''fn mixer_percent_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    min_percent: i32,
    max_percent: i32,
    color: Option<egui::Color32>,
) -> bool {
'''
helper = '''#[derive(Clone, Copy, Debug)]
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

'''+anchor
s = replace_once(s, anchor, helper, "Mixer responsive layout helper")
old = '''    with_accent(ui, color, |ui| {
        ui.horizontal(|ui| {
            if let Some(color) = color {
                ui.add_sized(
                    [86.0, 20.0],
                    egui::Label::new(egui::RichText::new(label).color(color)),
                );
            } else {
                ui.add_sized([86.0, 20.0], egui::Label::new(label));
            }
            let slider_width = (ui.available_width() - 58.0).max(54.0);
            ui.add_sized(
                [slider_width, 20.0],
                egui::Slider::new(&mut percent, min_percent..=max_percent)
                    .step_by(1.0)
                    .show_value(false)
                    .trailing_fill(true),
            );
            ui.add(
                egui::DragValue::new(&mut percent)
                    .range(min_percent..=max_percent)
                    .speed(1.0)
                    .suffix("%"),
            );
        });
    });
'''
new = '''    with_accent(ui, color, |ui| {
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
'''
s = replace_once(s, old, new, "Responsive Mixer row")
test_marker = '''    #[test]
    fn mixer_percent_round_trip_matches_normalized_storage() {
'''
test = '''    #[test]
    fn mixer_row_slider_expands_with_panel_width() {
        let narrow = mixer_row_layout(280.0, 8.0);
        let wide = mixer_row_layout(520.0, 8.0);
        assert!(narrow.slider_width >= 72.0);
        assert!(wide.slider_width > narrow.slider_width + 180.0);
        assert_eq!(narrow.value_width, wide.value_width);
        assert!(wide.label_width >= narrow.label_width);
    }

'''+test_marker
s = replace_once(s, test_marker, test, "Mixer layout regression test")
p.write_text(s, encoding="utf-8")

# Non-interactive overlay presentation is isolated from the viewport controller.
preview_status = r'''use eframe::egui;
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
'''
Path("src/ui/preview_status.rs").write_text(preview_status, encoding="utf-8")

p = Path("src/ui/mod.rs")
s = p.read_text(encoding="utf-8")
s = replace_once(
    s,
    "pub(crate) mod project_view_state;\n",
    "pub(crate) mod preview_status;\npub(crate) mod project_view_state;\n",
    "preview status module",
)
p.write_text(s, encoding="utf-8")

p = Path("src/main.rs")
s = p.read_text(encoding="utf-8")
face_clone_old = '''        let texture = face.texture.clone();
        let original_texture = face.original_texture.clone();
'''
face_clone_new = '''        let texture = face.texture.clone();
        let preview_is_updating = ui::preview_status::current_generation_is_rendering(
            self.current_face,
            face.generation,
            face.rendered_generation,
            self.render_busy,
        );
        let original_texture = face.original_texture.clone();
'''
s = replace_once(s, face_clone_old, face_clone_new, "preview generation predicate")
viewport_old = '''        let visible = ui.available_size().max(egui::vec2(1.0, 1.0));
        if self.fit_requested {
'''
viewport_new = '''        let preview_viewport_rect = ui.available_rect_before_wrap();
        let visible = ui.available_size().max(egui::vec2(1.0, 1.0));
        if self.fit_requested {
'''
s = replace_once(s, viewport_old, viewport_new, "preview overlay rect")
finish_old = '''        let _ = output;
        if recenter {
            self.viewport_recenter = false;
        }
    }
'''
finish_new = '''        let _ = output;
        if preview_is_updating {
            ui::preview_status::paint_updating_indicator(ui, preview_viewport_rect);
        }
        if recenter {
            self.viewport_recenter = false;
        }
    }
'''
s = replace_once(s, finish_old, finish_new, "preview updating overlay")
p.write_text(s, encoding="utf-8")

p = Path("RELEASE_NOTES.md")
notes = p.read_text(encoding="utf-8")
if not notes.startswith("# Shade Editor 0.20.7"):
    prefix = '''# Shade Editor 0.20.7\n\n- Make Channel Mixer sliders consume the available adjustment-panel width while preserving compact labels and percent fields.\n- Add a small non-interactive loading indicator over the current preview only while its exact render generation is being updated.\n- Keep the previously rendered preview fully sharp and unchanged during updates; no blur, dimming or interaction blocker is introduced.\n\n'''
    p.write_text(prefix + notes, encoding="utf-8")
