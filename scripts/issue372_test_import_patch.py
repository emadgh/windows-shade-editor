from pathlib import Path

path = Path("src/ui/conversion_candidate_preview.rs")
text = path.read_text(encoding="utf-8")
old = '''#[cfg(test)]
mod tests {
    use super::*;
'''
new = '''#[cfg(test)]
mod tests {
    use super::*;
    use windows_shade_editor::color_conversion::TargetChannelDefinition;
'''
if text.count(old) != 1:
    raise SystemExit(f"expected one candidate test module anchor, found {text.count(old)}")
path.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")
print("patched candidate test-only TargetChannelDefinition import")
