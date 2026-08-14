from pathlib import Path

path = Path('src/app_main.rs')
text = path.read_text(encoding='utf-8')

old = '''                egui::SidePanel::right("project-view-preview-pane")
                    .resizable(true)
                    .default_width(420.0)
                    .width_range(320.0..=580.0)
                    .show_inside(ui, |preview_ui| {'''
new = '''                egui::Panel::right("project-view-preview-pane")
                    .resizable(true)
                    .default_size(420.0)
                    .size_range(320.0..=580.0)
                    .show(ui, |preview_ui| {'''

if old not in text:
    raise RuntimeError('Project View panel marker not found')
text = text.replace(old, new, 1)
path.write_text(text, encoding='utf-8')
