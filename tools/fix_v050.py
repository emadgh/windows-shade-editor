from pathlib import Path

p = Path(__file__).resolve().parents[1] / "src/app_main.rs"
s = p.read_text(encoding="utf-8")
old = '''                match self.adjustment_scope {
                    AdjustmentScope::Selected => self.ui_selected_adjustment(
                        ui,
                        &output_name,
                        &channel_names,
                        &all_adjusted_histograms,
                        control_accent,
                    ),
                    AdjustmentScope::All => self.ui_all_adjustments(
                        ui,
                        &output_name,
                        &channel_names,
                        active_histogram.as_ref(),
                        control_accent,
                    ),
                }'''
new = '''                match self.adjustment_scope {
                    AdjustmentScope::Selected => self.ui_selected_adjustment(
                        ui,
                        &output_name,
                        &channel_names,
                        active_histogram.as_ref(),
                        control_accent,
                    ),
                    AdjustmentScope::All => self.ui_all_adjustments(
                        ui,
                        &output_name,
                        &channel_names,
                        &all_adjusted_histograms,
                        control_accent,
                    ),
                }'''
if old not in s:
    raise SystemExit("adjustment wiring marker missing")
s = s.replace(old, new, 1)
s = s.replace('''            if !ui.available_rect_before_wrap().is_negative() {
                ui.add_space(2.0);
            }''', '''            ui.add_space(2.0);''')
s = s.replace('egui::Button::new("⇧").small()', 'egui::Button::new("⇧").min_size(egui::vec2(20.0, 20.0))')
s = s.replace('.frame(false).sense(egui::Sense::click())', '.frame(false)')
p.write_text(s, encoding="utf-8")
