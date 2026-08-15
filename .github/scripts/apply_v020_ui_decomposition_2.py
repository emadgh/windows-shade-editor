from pathlib import Path
import re

root = Path(__file__).resolve().parents[2]
main_path = root / "src" / "main.rs"
workflow_path = root / "src" / "workflow.rs"
ui_dir = root / "src" / "ui"
main = main_path.read_text(encoding="utf-8")
workflow = workflow_path.read_text(encoding="utf-8")
main_before = len(main.splitlines())
workflow_before = len(workflow.splitlines())


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
            return_i = len(text) if newline == -1 else newline + 1
            i = return_i
            continue
        if ch == "/" and nxt == "*":
            block_comment_depth = 1
            i += 2
            continue
        raw_prefix = None
        if ch == "r":
            raw_prefix = 1
        elif ch == "b" and nxt == "r":
            raw_prefix = 2
        if raw_prefix is not None:
            j = i + raw_prefix
            hashes = 0
            while j < len(text) and text[j] == "#":
                hashes += 1
                j += 1
            if j < len(text) and text[j] == '"':
                terminator = '"' + ("#" * hashes)
                end = text.find(terminator, j + 1)
                if end == -1:
                    raise SystemExit("unterminated raw string")
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
    raise SystemExit("matching brace not found")


def consume_trailing_newlines(source: str, end: int) -> int:
    while end < len(source) and source[end] in " \t\r":
        end += 1
    if end < len(source) and source[end] == "\n":
        end += 1
    if end < len(source) and source[end] == "\n":
        end += 1
    return end


def extract_method(source: str, name: str):
    pattern = re.compile(rf"(?m)^    (?:pub\([^)]*\) )?fn {re.escape(name)}\b")
    matches = list(pattern.finditer(source))
    if len(matches) != 1:
        raise SystemExit(f"{name}: expected one method, found {len(matches)}")
    start = matches[0].start()
    open_index = source.find("{", matches[0].end())
    close_index = matching_brace(source, open_index)
    end = consume_trailing_newlines(source, close_index + 1)
    block = source[start:close_index + 1]
    block = re.sub(
        rf"(?m)^    (?:pub\([^)]*\) )?fn {re.escape(name)}\b",
        f"    pub(crate) fn {name}",
        block,
        count=1,
    )
    return source[:start] + source[end:], block


def extract_free_fn(source: str, name: str):
    pattern = re.compile(rf"(?m)^(?:pub\([^)]*\) )?fn {re.escape(name)}\b")
    matches = list(pattern.finditer(source))
    if len(matches) != 1:
        raise SystemExit(f"{name}: expected one free function, found {len(matches)}")
    start = matches[0].start()
    open_index = source.find("{", matches[0].end())
    close_index = matching_brace(source, open_index)
    end = consume_trailing_newlines(source, close_index + 1)
    block = source[start:close_index + 1]
    return source[:start] + source[end:], block


def find_method_containing(source: str, needle: str) -> str:
    methods = []
    for match in re.finditer(r"(?m)^    (?:pub\([^)]*\) )?fn ([A-Za-z0-9_]+)\b", source):
        open_index = source.find("{", match.end())
        if open_index == -1:
            continue
        close_index = matching_brace(source, open_index)
        body = source[match.start():close_index + 1]
        if needle in body:
            methods.append(match.group(1))
    if len(methods) != 1:
        raise SystemExit(f"expected one method containing {needle!r}, found {methods}")
    return methods[0]


# Faces UI: move presentation out of workflow; keep relink/backend helpers in workflow.
workflow, ui_faces = extract_free_fn(workflow, "ui_faces")
ui_faces = re.sub(r"(?m)^pub\(super\) fn ui_faces\b", "pub(crate) fn ui_faces", ui_faces, count=1)
main, face_identity = extract_free_fn(main, "face_identity_key")
main, duplicate_counts = extract_free_fn(main, "duplicate_face_counts")
faces_rs = "use crate::*;\nuse crate::workflow::*;\nuse eframe::egui;\n\n" + face_identity + "\n\n" + duplicate_counts + "\n\n" + ui_faces + "\n"
(ui_dir / "faces.rs").write_text(faces_rs, encoding="utf-8")
if "workflow::ui_faces" not in main:
    raise SystemExit("main.rs call to workflow::ui_faces not found")
main = main.replace("workflow::ui_faces", "ui::faces::ui_faces")

# Curve UI: this block is deliberately bounded by the CurvePointKind marker and the
# first non-Curve helper that followed it in main.rs.
curve_start_marker = "#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\nenum CurvePointKind"
curve_end_marker = "fn build_project_file_metadata("
curve_start = main.find(curve_start_marker)
curve_end = main.find(curve_end_marker)
if curve_start == -1 or curve_end == -1 or curve_end <= curve_start:
    raise SystemExit("curve extraction boundaries not found")
curve_block = main[curve_start:curve_end]
if "fn curves_ui" not in curve_block or "fn curve_editor_graph" not in curve_block:
    raise SystemExit("curve extraction block does not contain expected editor functions")
# face_identity helpers were already removed, so the remaining content before metadata
# should be Curve UI/support only. Expose only the entry point used by adjustments UI.
curve_block = re.sub(r"(?m)^fn curves_ui\b", "pub(crate) fn curves_ui", curve_block, count=1)
main = main[:curve_start] + main[curve_end:]
(ui_dir / "curve_editor.rs").write_text("use crate::*;\nuse eframe::egui;\n\n" + curve_block.rstrip() + "\n", encoding="utf-8")

adjustments_path = ui_dir / "adjustments.rs"
adjustments = adjustments_path.read_text(encoding="utf-8")
if "use super::curve_editor::curves_ui;" not in adjustments:
    adjustments = adjustments.replace("use crate::*;\n", "use crate::*;\nuse super::curve_editor::curves_ui;\n", 1)
adjustments_path.write_text(adjustments, encoding="utf-8")

# Project navigation: extract Project View and the menu method that owns Recent projects.
recent_menu_method = find_method_containing(main, '"Recent projects"')
main, menu_block = extract_method(main, recent_menu_method)
main, project_view_block = extract_method(main, "ui_previous_shades_window")
project_navigation = "use crate::*;\nuse eframe::egui;\n\nimpl ShadeApp {\n" + menu_block + "\n\n" + project_view_block + "\n}\n"
(ui_dir / "project_navigation.rs").write_text(project_navigation, encoding="utf-8")

# Move the input-context implementation under UI, keeping a root alias so workflow.rs
# can keep using the same typed router without behavior changes.
input_path = root / "src" / "input_router.rs"
if not input_path.exists():
    raise SystemExit("src/input_router.rs missing")
(ui_dir / "input_router.rs").write_text(input_path.read_text(encoding="utf-8"), encoding="utf-8")
input_path.unlink()
if "mod input_router;\n" not in main:
    raise SystemExit("root input_router module declaration missing")
main = main.replace("mod input_router;\n", "", 1)
use_anchor = "use tiff_io::PreviewFace;\n"
if use_anchor not in main:
    raise SystemExit("root use anchor missing")
main = main.replace(use_anchor, use_anchor + "use ui::input_router;\n", 1)

main_path.write_text(main, encoding="utf-8")
workflow_path.write_text(workflow, encoding="utf-8")

# Consolidate UI module declarations and architecture regression checks.
ui_mod = '''pub(crate) mod adjustments;
pub(crate) mod curve_editor;
pub(crate) mod export_queue;
pub(crate) mod faces;
pub(crate) mod input_router;
pub(crate) mod project_navigation;
pub(crate) mod status_bar;

#[cfg(test)]
mod tests {
    #[test]
    fn decomposed_ui_does_not_regress_back_into_application_shells() {
        let main = include_str!("../main.rs");
        let workflow = include_str!("../workflow.rs");
        for method in [
            "ui_history",
            "ui_channels_histogram",
            "ui_adjustment_quick_tools",
            "ui_adjustments",
            "ui_selected_adjustment",
            "ui_export_queue_window",
            "project_save_state_label",
            "ui_status",
            "ui_previous_shades_window",
        ] {
            assert!(!main.contains(&format!("fn {method}")), "{method} regressed into main.rs");
        }
        assert!(!workflow.contains("fn ui_faces"), "Faces UI regressed into workflow.rs");
        assert!(!main.contains("enum CurvePointKind"), "Curve editor state regressed into main.rs");
        assert!(!main.contains("fn curve_editor_graph"), "Curve editor graph regressed into main.rs");
        assert!(!main.contains("\"Recent projects\""), "Recent menu regressed into main.rs");
        assert!(!main.contains("mod input_router;"), "Input router regressed to the crate root");
    }
}
'''
(ui_dir / "mod.rs").write_text(ui_mod, encoding="utf-8")

main_after = len(main.splitlines())
workflow_after = len(workflow.splitlines())
if main_after >= main_before - 250:
    raise SystemExit(f"main.rs reduction too small: {main_before} -> {main_after}")
if workflow_after >= workflow_before - 80:
    raise SystemExit(f"workflow.rs reduction too small: {workflow_before} -> {workflow_after}")

# Patch version for the second behavior-preserving architecture pass.
cargo_path = root / "Cargo.toml"
cargo = cargo_path.read_text(encoding="utf-8")
if cargo.count('version = "0.19.1"') < 1:
    raise SystemExit("Cargo.toml 0.19.1 not found")
cargo_path.write_text(cargo.replace('version = "0.19.1"', 'version = "0.19.2"', 1), encoding="utf-8")
(root / "VERSION").write_text("0.19.2\n", encoding="utf-8")
lock_path = root / "Cargo.lock"
lock = lock_path.read_text(encoding="utf-8")
needle = 'name = "windows-shade-editor"\nversion = "0.19.1"'
if lock.count(needle) != 1:
    raise SystemExit("Cargo.lock root 0.19.1 entry not found uniquely")
lock_path.write_text(lock.replace(needle, 'name = "windows-shade-editor"\nversion = "0.19.2"', 1), encoding="utf-8")

notes_path = root / "RELEASE_NOTES.md"
notes = notes_path.read_text(encoding="utf-8")
header = f'''# Shade Editor 0.19.2

- Complete the main target set from Issue #40 by extracting Faces, Curve editing, Project View/Recent navigation and the typed input router into focused `src/ui` modules.
- Keep Face relink/loading workflow logic in `workflow.rs` while moving only Faces presentation/context-menu behavior to `src/ui/faces.rs`.
- Move Curve point state, graph interaction and Curve UI support out of the application shell into `src/ui/curve_editor.rs`; the Adjustments module consumes a single Curve UI entry point.
- Move Project View plus the application menu surface that owns Recent Projects into `src/ui/project_navigation.rs`.
- Move the input context router under `src/ui/input_router.rs` without changing shortcut semantics.
- Strengthen architecture regression coverage so extracted UI cannot silently accumulate back in `main.rs` or `workflow.rs`.
- Reduce `src/main.rs` from {main_before} to {main_after} lines and `src/workflow.rs` from {workflow_before} to {workflow_after} lines in this pass.

'''
if notes.startswith("# Shade Editor 0.19.2"):
    raise SystemExit("0.19.2 release notes already exist")
notes_path.write_text(header + notes, encoding="utf-8")

architecture = f'''# UI decomposition

Issue #40 is implemented as incremental, build-gated extractions rather than a one-shot rewrite.

## Focused UI modules

- `src/ui/input_router.rs` — typed keyboard/focus context classification.
- `src/ui/curve_editor.rs` — Curve point state, graph interaction and Curve controls.
- `src/ui/adjustments.rs` — History, Channels/Histogram, relative presets and adjustment composition.
- `src/ui/faces.rs` — Face list/status/context-menu presentation; relink/loading logic remains in `workflow.rs`.
- `src/ui/export_queue.rs` — Export Queue presentation and queue interaction surface.
- `src/ui/status_bar.rs` — save-state and bottom status presentation.
- `src/ui/project_navigation.rs` — app menu/Recent Projects and Project View presentation.

The modules extend `ShadeApp` only where application-level orchestration is still required. Production TIFF/export/model safety remains in the existing controller/model/workflow boundaries; this refactor does not duplicate those rules in UI modules.

### Measured reductions

Second pass: `src/main.rs` {main_before} -> {main_after} lines; `src/workflow.rs` {workflow_before} -> {workflow_after} lines.

Further architecture work should focus on typed UI action return values for cross-domain mutations and narrower Project View/controller state, rather than moving code merely to reduce line counts.
'''
(root / "docs" / "UI_DECOMPOSITION.md").write_text(architecture, encoding="utf-8")

# One-off validation machinery does not land in the validated source tree.
Path(__file__).unlink()
workflow_bootstrap = root / ".github" / "workflows" / "apply-v020-ui-decomposition-2.yml"
if workflow_bootstrap.exists():
    workflow_bootstrap.unlink()

print(f"Recent menu owner: {recent_menu_method}")
print(f"main.rs: {main_before} -> {main_after} lines")
print(f"workflow.rs: {workflow_before} -> {workflow_after} lines")
