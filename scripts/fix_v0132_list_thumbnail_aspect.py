from pathlib import Path

path = Path('src/app_main.rs')
text = path.read_text(encoding='utf-8')
old = '''    if let Some(texture) = thumbnail {
        ui.painter().image(
            texture.id(),
            thumb_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
'''
new = '''    if let Some(texture) = thumbnail {
        let natural = texture.size_vec2();
        let scale = if natural.x > 0.0 && natural.y > 0.0 {
            (thumb_rect.width() / natural.x)
                .min(thumb_rect.height() / natural.y)
                .min(1.0)
        } else {
            1.0
        };
        let image_rect = egui::Rect::from_center_size(thumb_rect.center(), natural * scale);
        ui.painter().rect_filled(
            thumb_rect,
            4.0,
            ui.visuals().extreme_bg_color,
        );
        ui.painter().image(
            texture.id(),
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
'''
if text.count(old) != 1:
    raise RuntimeError(f'expected one list thumbnail paint block, found {text.count(old)}')
path.write_text(text.replace(old, new, 1), encoding='utf-8')
print('Previous Shades list thumbnail aspect ratio preserved')
