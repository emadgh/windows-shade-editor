from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if new in text:
        return text
    if old not in text:
        raise SystemExit(f"{label}: expected source pattern not found")
    return text.replace(old, new, 1)


Path("VERSION").write_text("0.20.8\n", encoding="utf-8")
p = Path("Cargo.toml")
s = p.read_text(encoding="utf-8")
s = s.replace('version = "0.20.7"', 'version = "0.20.8"', 1)
p.write_text(s, encoding="utf-8")

p = Path("src/main.rs")
s = p.read_text(encoding="utf-8")
s = replace_once(
    s,
    '''    let painter = ui.painter_at(rect);
    painter.rect_stroke(
''',
    '''    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
''',
    "standalone histogram dark background",
)
p.write_text(s, encoding="utf-8")

p = Path("RELEASE_NOTES.md")
notes = p.read_text(encoding="utf-8")
if not notes.startswith("# Shade Editor 0.20.8"):
    prefix = '''# Shade Editor 0.20.8\n\n- Match standalone Channels histogram graph backgrounds to the darker Levels and Curve surfaces.\n- Preserve histogram dimensions, Light/Pigment presentation and channel accent behavior.\n\n'''
    p.write_text(prefix + notes, encoding="utf-8")
