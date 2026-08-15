from pathlib import Path
import re

root = Path(__file__).resolve().parents[2]
main_path = root / "src" / "main.rs"
ui_dir = root / "src" / "ui"


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


state = r'''use crate::*;
use eframe::egui;
use std::collections::{BTreeMap, VecDeque};

pub(crate) struct ProjectViewState {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) sort: previous_shades::PreviousShadesSort,
    pub(crate) selected: Option<String>,
    pub(crate) preview: Option<previous_shades::ShadeInspection>,
    pub(crate) preview_error: Option<String>,
    pub(crate) texture: Option<egui::TextureHandle>,
    pub(crate) list_textures: BTreeMap<String, egui::TextureHandle>,
    pub(crate) list_texture_lru: VecDeque<String>,
}

impl Default for ProjectViewState {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            sort: previous_shades::PreviousShadesSort::LastOpened,
            selected: None,
            preview: None,
            preview_error: None,
            texture: None,
            list_textures: BTreeMap::new(),
            list_texture_lru: VecDeque::new(),
        }
    }
}

impl ProjectViewState {
    pub(crate) fn needs_preview_load(&self, path: &str) -> bool {
        self.selected.as_deref() != Some(path)
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected = None;
        self.preview = None;
        self.preview_error = None;
        self.texture = None;
    }

    pub(crate) fn forget_path(&mut self, path: &str) {
        self.list_textures.remove(path);
        self.list_texture_lru.retain(|item| item != path);
        if self.selected.as_deref() == Some(path) {
            self.clear_selection();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_closed_and_unselected() {
        let state = ProjectViewState::default();
        assert!(!state.open);
        assert!(state.query.is_empty());
        assert_eq!(state.sort, previous_shades::PreviousShadesSort::LastOpened);
        assert!(state.selected.is_none());
        assert!(state.preview.is_none());
        assert!(state.preview_error.is_none());
        assert!(state.texture.is_none());
        assert!(state.list_textures.is_empty());
        assert!(state.list_texture_lru.is_empty());
    }

    #[test]
    fn preview_load_is_only_needed_when_selection_changes() {
        let mut state = ProjectViewState::default();
        assert!(state.needs_preview_load("a.shade"));
        state.selected = Some("a.shade".to_owned());
        assert!(!state.needs_preview_load("a.shade"));
        assert!(state.needs_preview_load("b.shade"));
    }

    #[test]
    fn forgetting_selected_path_clears_selection_and_lru_metadata() {
        let mut state = ProjectViewState::default();
        state.selected = Some("a.shade".to_owned());
        state.preview_error = Some("old preview".to_owned());
        state.list_texture_lru.push_back("a.shade".to_owned());
        state.list_texture_lru.push_back("b.shade".to_owned());
        state.forget_path("a.shade");
        assert!(state.selected.is_none());
        assert!(state.preview_error.is_none());
        assert_eq!(state.list_texture_lru.len(), 1);
        assert_eq!(state.list_texture_lru.front().map(String::as_str), Some("b.shade"));
    }
}
'''
(ui_dir / "project_view_state.rs").write_text(state, encoding="utf-8")

# Register focused state module.
ui_mod_path = ui_dir / "mod.rs"
ui_mod = ui_mod_path.read_text(encoding="utf-8")
if "pub(crate) mod project_view_state;" not in ui_mod:
    anchor = "pub(crate) mod project_navigation;\n"
    if anchor not in ui_mod:
        raise SystemExit("project_navigation module declaration missing")
    ui_mod = ui_mod.replace(anchor, anchor + "pub(crate) mod project_view_state;\n", 1)
ui_mod_path.write_text(ui_mod, encoding="utf-8")

# Collapse top-level transient Project View fields into one focused state object.
main = main_path.read_text(encoding="utf-8")
field_block = '''    show_previous_shades: bool,\n    previous_shades: previous_shades::PreviousShadesStore,\n    previous_shades_query: String,\n    previous_shades_sort: previous_shades::PreviousShadesSort,\n    previous_shades_selected: Option<String>,\n    previous_shade_preview: Option<previous_shades::ShadeInspection>,\n    previous_shade_preview_error: Option<String>,\n    previous_shade_texture: Option<egui::TextureHandle>,\n    previous_shade_list_textures: BTreeMap<String, egui::TextureHandle>,\n    previous_shade_list_texture_lru: VecDeque<String>,\n'''
new_field_block = '''    previous_shades: previous_shades::PreviousShadesStore,\n    project_view: ui::project_view_state::ProjectViewState,\n'''
if main.count(field_block) != 1:
    raise SystemExit(f"Project View field block expected once, found {main.count(field_block)}")
main = main.replace(field_block, new_field_block, 1)

init_block = '''            show_previous_shades: false,\n            previous_shades,\n            previous_shades_query: String::new(),\n            previous_shades_sort: previous_shades::PreviousShadesSort::LastOpened,\n            previous_shades_selected: None,\n            previous_shade_preview: None,\n            previous_shade_preview_error: None,\n            previous_shade_texture: None,\n            previous_shade_list_textures: BTreeMap::new(),\n            previous_shade_list_texture_lru: VecDeque::new(),\n'''
new_init_block = '''            previous_shades,\n            project_view: ui::project_view_state::ProjectViewState::default(),\n'''
if main.count(init_block) != 1:
    raise SystemExit(f"Project View init block expected once, found {main.count(init_block)}")
main = main.replace(init_block, new_init_block, 1)
main_path.write_text(main, encoding="utf-8")

# Update exact transient identifiers everywhere in Rust source, without touching method names.
identifier_map = {
    "show_previous_shades": "project_view.open",
    "previous_shades_query": "project_view.query",
    "previous_shades_sort": "project_view.sort",
    "previous_shades_selected": "project_view.selected",
    "previous_shade_preview_error": "project_view.preview_error",
    "previous_shade_preview": "project_view.preview",
    "previous_shade_texture": "project_view.texture",
    "previous_shade_list_texture_lru": "project_view.list_texture_lru",
    "previous_shade_list_textures": "project_view.list_textures",
}
for path in sorted((root / "src").rglob("*.rs")):
    if path == ui_dir / "project_view_state.rs":
        continue
    text = path.read_text(encoding="utf-8")
    original = text
    for old, new in identifier_map.items():
        text = re.sub(rf"\b{re.escape(old)}\b", new, text)
    if text != original:
        path.write_text(text, encoding="utf-8")

# Use state policy helpers in the typed action dispatcher instead of duplicating selection/cache cleanup.
actions_path = ui_dir / "actions.rs"
actions = actions_path.read_text(encoding="utf-8")
actions = actions.replace(
    '''            ProjectViewUiAction::Select(path) => {\n                if self.project_view.selected.as_deref() != Some(path.as_str()) {\n                    self.load_previous_shade_preview(ctx, &path);\n                }\n            }\n''',
    '''            ProjectViewUiAction::Select(path) => {\n                if self.project_view.needs_preview_load(&path) {\n                    self.load_previous_shade_preview(ctx, &path);\n                }\n            }\n''',
    1,
)
actions = actions.replace(
    '''                            self.project_view.list_textures.remove(&old_path);\n                            self.project_view.list_texture_lru\n                                .retain(|item| item != &old_path);\n                            self.load_previous_shade_preview(ctx, &new_display);\n''',
    '''                            self.project_view.forget_path(&old_path);\n                            self.load_previous_shade_preview(ctx, &new_display);\n''',
    1,
)
actions = actions.replace(
    '''                    self.project_view.list_textures.remove(&path);\n                    self.project_view.list_texture_lru\n                        .retain(|item| item != &path);\n                    if self.project_view.selected.as_deref() == Some(path.as_str()) {\n                        self.project_view.selected = None;\n                        self.project_view.preview = None;\n                        self.project_view.preview_error = None;\n                        self.project_view.texture = None;\n                    }\n''',
    '''                    self.project_view.forget_path(&path);\n''',
    1,
)
actions_path.write_text(actions, encoding="utf-8")

# Architecture regression: transient Project View state must remain off ShadeApp's top level.
ui_mod = ui_mod_path.read_text(encoding="utf-8")
insert = r'''

    #[test]
    fn project_view_transient_state_stays_behind_focused_state_object() {
        let main = include_str!("../main.rs");
        for legacy_field in [
            "show_previous_shades: bool",
            "previous_shades_query: String",
            "previous_shades_sort: previous_shades::PreviousShadesSort",
            "previous_shades_selected: Option<String>",
            "previous_shade_preview: Option<previous_shades::ShadeInspection>",
            "previous_shade_preview_error: Option<String>",
            "previous_shade_texture: Option<egui::TextureHandle>",
            "previous_shade_list_textures: BTreeMap<String, egui::TextureHandle>",
            "previous_shade_list_texture_lru: VecDeque<String>",
        ] {
            assert!(
                !main.contains(legacy_field),
                "Project View transient state regressed to ShadeApp: {legacy_field}"
            );
        }
        assert!(
            main.contains("project_view: ui::project_view_state::ProjectViewState"),
            "ShadeApp must own one focused ProjectViewState"
        );
    }
'''
pos = ui_mod.rfind("}\n")
if pos == -1:
    raise SystemExit("UI test module closing brace not found")
ui_mod = ui_mod[:pos] + insert + ui_mod[pos:]
ui_mod_path.write_text(ui_mod, encoding="utf-8")

# Ensure no exact legacy transient identifier remains in production source outside the architecture-test string literals.
for path in sorted((root / "src").rglob("*.rs")):
    if path == ui_mod_path:
        continue
    text = path.read_text(encoding="utf-8")
    for old in identifier_map:
        if re.search(rf"\b{re.escape(old)}\b", text):
            raise SystemExit(f"legacy Project View field identifier remains: {path}:{old}")

# Patch version.
replace_once(root / "Cargo.toml", 'version = "0.20.0"', 'version = "0.20.1"', "Cargo version")
(root / "VERSION").write_text("0.20.1\n", encoding="utf-8")
replace_once(
    root / "Cargo.lock",
    'name = "windows-shade-editor"\nversion = "0.20.0"',
    'name = "windows-shade-editor"\nversion = "0.20.1"',
    "Cargo.lock root version",
)

notes_path = root / "RELEASE_NOTES.md"
notes = notes_path.read_text(encoding="utf-8")
header = '''# Shade Editor 0.20.1\n\n- Extract Project View transient state into a focused `ProjectViewState` instead of keeping query/sort/selection/preview/texture-cache fields directly on `ShadeApp`.\n- Keep `PreviousShadesStore` as the single persistent recent-project/history owner; the new state object owns UI/session state only.\n- Centralize Project View selection and cache cleanup policy with `needs_preview_load`, `clear_selection` and `forget_path` helpers.\n- Add state unit tests plus an architecture regression guard preventing Project View transient fields from drifting back to top-level `ShadeApp`.\n- No intended Project View, project lifecycle, TIFF, color or export behavior change.\n\n'''
if notes.startswith("# Shade Editor 0.20.1"):
    raise SystemExit("0.20.1 release notes already exist")
notes_path.write_text(header + notes, encoding="utf-8")

# Update architecture doc with the new state boundary.
arch_path = root / "docs" / "UI_DECOMPOSITION.md"
arch = arch_path.read_text(encoding="utf-8")
addition = '''\n## Project View state boundary\n\n`PreviousShadesStore` remains the persistent history/cache metadata owner. `ProjectViewState` owns only transient UI state: open/query/sort/selection, loaded inspection/preview status and runtime egui texture caches. Cross-domain operations continue through typed UI actions and the existing lifecycle/workflow controllers.\n'''
if "## Project View state boundary" not in arch:
    arch_path.write_text(arch.rstrip() + "\n" + addition, encoding="utf-8")

# Bootstrap cleanup after validation applies the production tree.
Path(__file__).unlink()
workflow = root / ".github" / "workflows" / "apply-v020-project-view-state.yml"
if workflow.exists():
    workflow.unlink()
