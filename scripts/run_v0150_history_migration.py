from pathlib import Path
import re

migration = Path('scripts/apply_v0150_history_project_ui.py')
source = migration.read_text(encoding='utf-8')
needle = "if app.count('histogram.filter(|_| self.settings.show_curve_histogram)') != 0:\n    raise SystemExit('selected curve histogram call replacement incomplete')"
fix = r'''app = re.sub(
    r'histogram\.filter\(\|_\| self\.settings\.show_curve_histogram\),\n(\s*)accent,',
    lambda match: (
        'histogram_before.filter(|_| self.settings.show_curve_histogram),\n'
        + match.group(1)
        + 'histogram_after.filter(|_| self.settings.show_curve_histogram),\n'
        + match.group(1)
        + 'accent,'
    ),
    app,
)
if app.count('histogram.filter(|_| self.settings.show_curve_histogram)') != 0:
    raise SystemExit('selected curve histogram call replacement incomplete')'''
if needle not in source:
    raise SystemExit('Could not patch selected Curve histogram matcher')
source = source.replace(needle, fix, 1)
exec(compile(source, str(migration), 'exec'))

# One stacked/foldout all-curves call has different indentation from the tabs
# path. Fix the only remaining old call after the main migration has generated
# app_main.rs.
app_path = Path('src/app_main.rs')
app = app_path.read_text(encoding='utf-8')
pattern = re.compile(
    r'(all_curves_ui\(\s*ui,\s*&mut self\.project\.adjustments,\s*template_name,\s*channel_names,\s*)histograms,',
    re.S,
)
app, count = pattern.subn(
    r'\1histograms_before,\n                        histograms_after,',
    app,
    count=1,
)
if count != 1:
    raise SystemExit(f'Expected one remaining old all_curves_ui histogram call, found {count}')
app_path.write_text(app, encoding='utf-8')
