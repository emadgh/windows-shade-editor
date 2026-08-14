from pathlib import Path

path = Path('scripts/apply_v0132_previous_shades_ui.py')
text = path.read_text(encoding='utf-8')
old = '''previous = replace_once(
    previous,
    '                    existing.snapshots = entry.snapshots.clone();\\n                    existing.test_code_text = entry.test_code_text.clone();',
    '                    existing.snapshots = entry.snapshots.clone();\\n                    existing.test_code_text = entry.test_code_text.clone();\\n                    existing.face_count = entry.face_count;\\n                    existing.total_source_bytes = entry.total_source_bytes;\\n                    existing.thumbnail = entry.thumbnail.clone();',
    'sanitize new cache fields newest',
)
previous = replace_once(
    previous,
    '                    existing.snapshots = entry.snapshots.clone();\\n                    existing.test_code_text = entry.test_code_text.clone();\\n                }\\n                existing.open_count',
    '                    existing.snapshots = entry.snapshots.clone();\\n                    existing.test_code_text = entry.test_code_text.clone();\\n                    existing.face_count = entry.face_count;\\n                    existing.total_source_bytes = entry.total_source_bytes;\\n                    existing.thumbnail = entry.thumbnail.clone();\\n                }\\n                existing.open_count',
    'sanitize new cache fields version',
)
'''
new = '''previous = replace_count(
    previous,
    '                    existing.snapshots = entry.snapshots.clone();\\n                    existing.test_code_text = entry.test_code_text.clone();',
    '                    existing.snapshots = entry.snapshots.clone();\\n                    existing.test_code_text = entry.test_code_text.clone();\\n                    existing.face_count = entry.face_count;\\n                    existing.total_source_bytes = entry.total_source_bytes;\\n                    existing.thumbnail = entry.thumbnail.clone();',
    2,
    'sanitize new cache fields',
)
'''
if text.count(old) != 1:
    raise RuntimeError(f'expected one matcher block, found {text.count(old)}')
path.write_text(text.replace(old, new, 1), encoding='utf-8')
print('patch matcher fixed')
