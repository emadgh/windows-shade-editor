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

safe_path = Path('src/safe_fs.rs')
safe = safe_path.read_text(encoding='utf-8')
safe = safe.replace(
    'use std::fs::{self, File, OpenOptions};',
    'use std::fs::{self, OpenOptions};',
    1,
)
final_sync = '''        File::open(path)\n            .and_then(|file| file.sync_all())\n            .map_err(|err| format!("Cannot sync saved file {}: {err}", path.display()))?;\n'''
if final_sync not in safe:
    raise SystemExit('final read-only sync block not found')
safe = safe.replace(final_sync, '', 1)
copy_sync = '''    File::open(&temp)\n        .and_then(|file| file.sync_all())\n        .map_err(|err| format!("Cannot sync staged backup {}: {err}", temp.display()))?;\n'''
copy_sync_fixed = '''    OpenOptions::new()\n        .write(true)\n        .open(&temp)\n        .and_then(|file| file.sync_all())\n        .map_err(|err| format!("Cannot sync staged backup {}: {err}", temp.display()))?;\n'''
if copy_sync not in safe:
    raise SystemExit('backup read-only sync block not found')
safe = safe.replace(copy_sync, copy_sync_fixed, 1)
safe_path.write_text(safe, encoding='utf-8')

print('Registered safe_fs and fixed Windows writable sync handles')
