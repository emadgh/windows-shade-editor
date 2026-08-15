use crate::*;
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
        assert_eq!(
            state.list_texture_lru.front().map(String::as_str),
            Some("b.shade")
        );
    }
}
