from pathlib import Path

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
