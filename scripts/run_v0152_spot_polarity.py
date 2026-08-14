from pathlib import Path
import re

migration = Path('scripts/apply_v0151_spot_polarity.py')
source = migration.read_text(encoding='utf-8')

old = '''cargo = replace_once(cargo, 'version = "0.15.0"', 'version = "0.15.1"', 'Cargo.toml version')'''
new = '''cargo = replace_once(cargo, 'version = "0.15.1"', 'version = "0.15.2"', 'Cargo.toml version')'''
if old not in source:
    raise SystemExit('Could not adapt Cargo.toml migration version')
source = source.replace(old, new, 1)

old = '''    r'(\\[\\[package\\]\\]\\nname = "windows-shade-editor"\\nversion = ")0\\.15\\.0(")',
    r'\\g<1>0.15.1\\2','''
new = '''    r'(\\[\\[package\\]\\]\\nname = "windows-shade-editor"\\nversion = ")0\\.15\\.1(")',
    r'\\g<1>0.15.2\\2','''
if old not in source:
    raise SystemExit('Could not adapt Cargo.lock migration version')
source = source.replace(old, new, 1)

pattern = r'''# Three production call sites\.\nfor label, old, new in \[.*?\n    export = replace_once\(export, old, new, label\)\n'''
replacement = '''# Three production call sites.\nold_call = 'adjusted_strip(input, channels, names, project)'\nif export.count(old_call) != 3:\n    raise SystemExit(f'adjusted_strip production calls: expected 3 matches, found {export.count(old_call)}')\nexport = export.replace(old_call, 'adjusted_strip(input, &stream.metadata, project)', 2)\nexport = export.replace(old_call, 'adjusted_strip(input, metadata, project)', 1)\n'''
source, count = re.subn(pattern, replacement, source, count=1, flags=re.S)
if count != 1:
    raise SystemExit('Could not adapt adjusted_strip call-site migration')

exec(compile(source, str(migration), 'exec'))
