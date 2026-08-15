from pathlib import Path
import re

root = Path(__file__).resolve().parents[2]
main_path = root / "src" / "main.rs"
main = main_path.read_text(encoding="utf-8")
original_main_lines = len(main.splitlines())


def matching_brace(text: str, open_index: int) -> int:
    depth = 0
    i = open_index
    block_comment_depth = 0
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""

        if block_comment_depth:
            if ch == "/" and nxt == "*":
                block_comment_depth += 1
                i += 2
                continue
            if ch == "*" and nxt == "/":
                block_comment_depth -= 1
                i += 2
                continue
            i += 1
            continue

        if ch == "/" and nxt == "/":
            newline = text.find("\n", i + 2)
            if newline == -1:
                raise SystemExit("unterminated line comment while scanning method")
            i = newline + 1
            continue
        if ch == "/" and nxt == "*":
            block_comment_depth = 1
            i += 2
            continue

        # Rust raw strings: r"...", r#"..."#, br#"..."#.
        raw_start = None
        prefix_len = 0
        if ch == "r":
            raw_start = i
            prefix_len = 1
        elif ch == "b" and nxt == "r":
            raw_start = i
            prefix_len = 2
        if raw_start is not None:
            j = raw_start + prefix_len
            hashes = 0
            while j < len(text) and text[j] == "#":
                hashes += 1
                j += 1
            if j < len(text) and text[j] == '"':
                terminator = '"' + ("#" * hashes)
                end = text.find(terminator, j + 1)
                if end == -1:
                    raise SystemExit("unterminated raw string while scanning method")
                i = end + len(terminator)
                continue

        if ch == '"':
            i += 1
            while i < len(text):
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                i += 1
            continue

        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise SystemExit("could not find matching method brace")


def extract_method(source: str, name: str):
    pattern = re.compile(rf"(?m)^    fn {re.escape(name)}\b")
    matches = list(pattern.finditer(source))
    if len(matches) != 1:
        raise SystemExit(f"{name}: expected exactly one method, found {len(matches)}")
    start = matches[0].start()
    open_index = source.find("{", matches[0].end())
    if open_index == -1:
        raise SystemExit(f"{name}: method body not found")
    close_index = matching_brace(source, open_index)
    end = close_index + 1
    while end < len(source) and source[end] in " \t":
        end += 1
    if end < len(source) and source[end] == "\r":
        end += 1
    if end < len(source) and source[end] == "\n":
        end += 1
    if end < len(source) and source[end] == "\n":
        end += 1

    block = source[start:close_index + 1]
    block = block.replace(f"    fn {name}", f"    pub(crate) fn {name}", 1)
    return source[:start] + source[end:], block


modules = {
    "adjustments": [
        "ui_history",
        "ui_channels_histogram",
        "ui_adjustment_quick_tools",
        "ui_adjustments",
        "ui_selected_adjustment",
    ],
    "export_queue": ["ui_export_queue_window"],
    "status_bar": ["project_save_state_label", "ui_status"],
}

extracted = {}
for module_name, method_names in modules.items():
    blocks = []
    for method_name in method_names:
        main, block = extract_method(main, method_name)
        blocks.append(block)
    extracted[module_name] = blocks

if "mod ui;" in main:
    raise SystemExit("src/main.rs already declares mod ui")
anchor = "mod update;\nmod validation;"
if anchor not in main:
    raise SystemExit("main module declaration anchor not found")
main = main.replace(anchor, "mod update;\nmod ui;\nmod validation;", 1)
main_path.write_text(main, encoding="utf-8")

ui_dir = root / "src" / "ui"
ui_dir.mkdir(parents=True, exist_ok=True)
for module_name, blocks in extracted.items():
    content = "use crate::*;\nuse eframe::egui;\n\nimpl ShadeApp {\n"
    content += "\n\n".join(blocks)
    content += "\n}\n"
    (ui_dir / f"{module_name}.rs").write_text(content, encoding="utf-8")

method_signatures = [name for names in modules.values() for name in names]
mod_rs = '''pub(crate) mod adjustments;
pub(crate) mod export_queue;
pub(crate) mod status_bar;

#[cfg(test)]
mod tests {
    #[test]
    fn decomposed_ui_methods_do_not_regress_back_into_main() {
        let main = include_str!("../main.rs");
        for method in [
            "ui_history",
            "ui_channels_histogram",
            "ui_adjustment_quick_tools",
            "ui_adjustments",
            "ui_selected_adjustment",
            "ui_export_queue_window",
            "project_save_state_label",
            "ui_status",
        ] {
            assert!(
                !main.contains(&format!("fn {method}")),
                "{method} should remain in a focused src/ui module, not src/main.rs"
            );
        }
    }
}
'''
(ui_dir / "mod.rs").write_text(mod_rs, encoding="utf-8")

new_main_lines = len(main.splitlines())
if new_main_lines >= original_main_lines - 250:
    raise SystemExit(
        f"decomposition was too small: main.rs {original_main_lines} -> {new_main_lines} lines"
    )

# Patch-level version bump for a behavior-preserving architecture release.
cargo = root / "Cargo.toml"
text = cargo.read_text(encoding="utf-8")
if text.count('version = "0.19.0"') < 1:
    raise SystemExit("Cargo.toml 0.19.0 version not found")
text = text.replace('version = "0.19.0"', 'version = "0.19.1"', 1)
cargo.write_text(text, encoding="utf-8")
(root / "VERSION").write_text("0.19.1\n", encoding="utf-8")

lock = root / "Cargo.lock"
text = lock.read_text(encoding="utf-8")
needle = 'name = "windows-shade-editor"\nversion = "0.19.0"'
if text.count(needle) != 1:
    raise SystemExit("Cargo.lock root package 0.19.0 entry not found uniquely")
lock.write_text(text.replace(needle, 'name = "windows-shade-editor"\nversion = "0.19.1"', 1), encoding="utf-8")

notes = root / "RELEASE_NOTES.md"
text = notes.read_text(encoding="utf-8")
header = f'''# Shade Editor 0.19.1

- Continue Issue #40 with a behavior-preserving UI decomposition pass: move History/Channels/Adjustments, Export Queue window and Status Bar methods out of the 300+ KB application shell into focused `src/ui` modules.
- Add an architecture regression test that prevents the extracted UI methods from silently accumulating back inside `src/main.rs`.
- Keep application/controller behavior unchanged; this release is intended as a structural maintainability update before further Face/Curve/Recent extraction.
- Reduce `src/main.rs` from {original_main_lines} to {new_main_lines} lines in this pass.

'''
if text.startswith("# Shade Editor 0.19.1"):
    raise SystemExit("0.19.1 release notes already exist")
notes.write_text(header + text, encoding="utf-8")

architecture = f'''# UI decomposition

Shade Editor keeps the application root responsible for application lifecycle/orchestration while progressively moving cohesive egui rendering into `src/ui`.

## Current extracted UI modules

- `src/ui/adjustments.rs`: History, Channels/Histogram, Quick Relative Adjustments and adjustment editor composition.
- `src/ui/export_queue.rs`: Export Queue window presentation and queue interaction surface.
- `src/ui/status_bar.rs`: save-state and bottom status presentation.

The extraction is intentionally incremental. These modules are descendants of the crate root and extend `ShadeApp` with `pub(crate)` inherent methods, so they can reuse existing safety/controller methods without duplicating backend logic. Cross-cutting business rules remain in controllers/model/workflow modules.

This pass reduced `src/main.rs` from {original_main_lines} to {new_main_lines} lines. Future #40 passes should continue with Faces, Curve-specific UI and Recent/Project View, then replace direct cross-domain field access with typed UI actions where that meaningfully improves boundaries.
'''
(root / "docs" / "UI_DECOMPOSITION.md").write_text(architecture, encoding="utf-8")

# One-off bootstrap files are removed only after the tree has been transformed.
Path(__file__).unlink()
workflow = root / ".github" / "workflows" / "apply-v020-ui-decomposition.yml"
if workflow.exists():
    workflow.unlink()

print(f"main.rs lines: {original_main_lines} -> {new_main_lines}")
print("extracted methods:", ", ".join(method_signatures))
