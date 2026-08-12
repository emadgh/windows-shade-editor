from pathlib import Path

path = Path('src/app_main.rs')
text = path.read_text(encoding='utf-8')
old = '    id_salt: impl std::hash::Hash,\n'
new = '    id_salt: impl std::hash::Hash + std::fmt::Debug,\n'
if text.count(old) != 1:
    raise SystemExit(f'expected one id_salt bound, found {text.count(old)}')
path.write_text(text.replace(old, new, 1), encoding='utf-8')
