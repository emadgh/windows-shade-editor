from pathlib import Path

path = Path("src/workflow.rs")
text = path.read_text(encoding="utf-8")
old = "        let remove;\n"
new = "        let mut remove = false;\n"
if text.count(old) != 1:
    raise SystemExit(f"expected one uninitialized remove binding, found {text.count(old)}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
