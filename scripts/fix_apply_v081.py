from pathlib import Path

path = Path("scripts/apply_v081.py")
text = path.read_text(encoding="utf-8")
old = '''if not save.rstrip().endswith("});"):
    raise SystemExit("unexpected save_project ending")
save = save.rstrip() + "\\n        true\\n    }\\n\\n"'''
new = '''save = save.rstrip()
if not save.endswith("    }"):
    raise SystemExit("unexpected save_project ending")
save = save[:-5].rstrip() + "\\n        true\\n    }\\n\\n"'''
if old not in text:
    raise SystemExit("save helper anchor not found")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
