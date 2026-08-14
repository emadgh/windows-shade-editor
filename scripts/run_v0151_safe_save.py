from pathlib import Path

migration = Path('scripts/apply_v0151_safe_save.py')
exec(compile(migration.read_text(encoding='utf-8'), str(migration), 'exec'))

lib_path = Path('src/lib.rs')
lib = lib_path.read_text(encoding='utf-8')
anchor = '#[path = "palette.rs"]\npub mod palette;\n'
module = '#[path = "safe_fs.rs"]\npub mod safe_fs;\n'
if module not in lib:
    if anchor not in lib:
        raise SystemExit('src/lib.rs module anchor not found')
    lib = lib.replace(anchor, anchor + module, 1)
lib_path.write_text(lib, encoding='utf-8')
print('Registered safe_fs for the library target')
