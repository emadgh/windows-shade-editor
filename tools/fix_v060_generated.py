from pathlib import Path

p = Path(__file__).resolve().parents[1] / "src/app_main.rs"
s = p.read_text(encoding="utf-8")

old = '''                    changed |= all_curves_ui(
                        ui,
                        &mut self.project.adjustments,
                        template_name,
                        channel_names,
                        histograms,
                        self.settings.colorize_adjustments,
                        self.settings.show_curve_histogram,
                    );'''
new = '''                    changed |= all_curves_ui(
                        ui,
                        &mut self.project.adjustments,
                        template_name,
                        channel_names,
                        histograms,
                        self.settings.colorize_adjustments,
                        self.settings.show_curve_histogram,
                        palette,
                    );'''
if s.count(old) != 1:
    raise SystemExit(f"stacked all_curves target count: {s.count(old)}")
s = s.replace(old, new, 1)

old = '''                    changed |= all_mixers_ui(
                        ui,
                        &mut self.project.adjustments,
                        channel_names,
                        self.settings.colorize_adjustments,
                    );'''
new = '''                    changed |= all_mixers_ui(
                        ui,
                        &mut self.project.adjustments,
                        channel_names,
                        self.settings.colorize_adjustments,
                        palette,
                    );'''
if s.count(old) != 1:
    raise SystemExit(f"stacked all_mixers target count: {s.count(old)}")
s = s.replace(old, new, 1)

p.write_text(s, encoding="utf-8")
