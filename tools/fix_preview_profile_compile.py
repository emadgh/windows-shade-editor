from pathlib import Path

color = Path("src/color_management.rs")
text = color.read_text(encoding="utf-8")
old = "#[derive(Clone, Copy, Debug)]\npub struct PreviewColorConfig"
new = "#[derive(Clone, Debug)]\npub struct PreviewColorConfig"
if old not in text:
    raise RuntimeError("PreviewColorConfig Copy derive marker not found")
color.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")

export = Path("src/export.rs")
text = export.read_text(encoding="utf-8")
old_stream = "    let names = &stream.metadata.channel_names;\n"
if text.count(old_stream) < 2:
    raise RuntimeError("Expected two unused stream channel-name bindings")
text = text.replace(old_stream, "", 2)
old_metadata = "    let names = &metadata.channel_names;\n"
if old_metadata not in text:
    raise RuntimeError("Expected unused region channel-name binding")
text = text.replace(old_metadata, "", 1)
export.write_text(text, encoding="utf-8", newline="\n")

print("Compile fix and unused export bindings applied.")
