from pathlib import Path

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

exec(compile(source, str(migration), 'exec'))
