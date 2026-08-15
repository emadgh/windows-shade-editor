from pathlib import Path

path = Path("src/export_queue.rs")
text = path.read_text(encoding="utf-8")
bad = 'text.push_str(" · \');'
count = text.count(bad)
if count < 1:
    raise SystemExit("malformed queue metrics separator was not found")
text = text.replace(bad, 'text.push_str(" · ");')
path.write_text(text, encoding="utf-8")
print(f"fixed {count} queue metrics separator(s)")
