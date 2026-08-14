from pathlib import Path
import re

path = Path("src/main.rs")
text = path.read_text(encoding="utf-8")

# Any legacy render site must use the canonical project-owned color config so
# future proof fields cannot be forgotten in hand-written initializers.
pattern = re.compile(
    r"let color_config = PreviewColorConfig \{.*?assigned_profile_path:.*?\.map\(PathBuf::from\),\s*\};",
    re.S,
)
text, color_count = pattern.subn(
    "let color_config = PreviewColorConfig::from_project(&self.project);",
    text,
)

# Old filename-preview/export call sites did not know the new source/date tokens.
# They are retained for compatibility, so provide conservative fallbacks there;
# the new queue paths supply real source/date values.
context_pattern = re.compile(r"export_batch::ExportNameContext \{(?P<body>.*?)\n(?P<indent>\s*)\}", re.S)

def complete_context(match: re.Match[str]) -> str:
    body = match.group("body")
    indent = match.group("indent")
    if "source_name:" in body and "date:" in body:
        return match.group(0)
    field_indent = indent + "    "
    additions = []
    if "source_name:" not in body:
        additions.append(f'{field_indent}source_name: "source",')
    if "date:" not in body:
        additions.append(f'{field_indent}date: "",')
    return "export_batch::ExportNameContext {" + body + "\n" + "\n".join(additions) + "\n" + indent + "}"

text, context_count = context_pattern.subn(complete_context, text)
path.write_text(text, encoding="utf-8", newline="\n")
print(f"Post-upgrade compile cleanup: color={color_count}, export_contexts={context_count}")
