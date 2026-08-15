from pathlib import Path

path = Path("src/production_acceptance.rs")
text = path.read_text(encoding="utf-8")
old = '''            "The active Face source TIFF is missing",\n            "Export all requires every Face source TIFF to be available",\n'''
new = '''            "The active Face source TIFF is missing",\n            "Export all requires every Accepted Face source TIFF to be available",\n            "status.is_rejected()",\n            "excluded {rejected_count} Rejected Face(s)",\n'''
if text.count(old) != 1:
    raise SystemExit(f"expected one legacy Export All production guard, found {text.count(old)}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
