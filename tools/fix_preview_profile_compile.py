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

def remove_after_marker(source: str, marker: str, target: str, label: str) -> str:
    before, sep, after = source.partition(marker)
    if not sep:
        raise RuntimeError(f"Missing {label} marker")
    if target not in after:
        raise RuntimeError(f"Missing unused binding in {label}")
    after = after.replace(target, "", 1)
    return before + sep + after

text = remove_after_marker(
    text,
    "fn stream_spool_u8",
    "    let names = &stream.metadata.channel_names;\n",
    "stream_spool_u8",
)
text = remove_after_marker(
    text,
    "fn stream_spool_u16",
    "    let names = &stream.metadata.channel_names;\n",
    "stream_spool_u16",
)
text = remove_after_marker(
    text,
    "fn stream_spool_regions",
    "    let names = &metadata.channel_names;\n",
    "stream_spool_regions",
)
export.write_text(text, encoding="utf-8", newline="\n")

print("Compile fix and scoped unused export bindings applied.")
