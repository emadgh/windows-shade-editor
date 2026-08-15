from pathlib import Path

path = Path("src/workflow.rs")
text = path.read_text(encoding="utf-8")
old = "                    app.project_dirty = true;\n"
new = "                    app.mark_project_dirty();\n"
count = text.count(old)
if count != 1:
    raise SystemExit(f"expected one direct Face-status dirty bypass, found {count}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
