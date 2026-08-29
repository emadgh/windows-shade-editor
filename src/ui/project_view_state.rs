use crate::*;
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use windows_shade_editor::file_observer::{self, ExternalFileRole};

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
    recent_observed_paths: BTreeSet<String>,
    view_observed_paths: BTreeSet<String>,
    registered_project_observers: BTreeSet<String>,
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
            recent_observed_paths: BTreeSet::new(),
            view_observed_paths: BTreeSet::new(),
            registered_project_observers: BTreeSet::new(),
        }
    }
}

impl ProjectViewState {
    pub(crate) fn needs_preview_load(&self, path: &str) -> bool {
        let observed = file_observer::observe(Path::new(path), ExternalFileRole::Project);
        if observed.is_changed() {
            // The caller immediately reloads inspection data when this returns true. Accept the
            // current filesystem fingerprint as that inspection attempt's baseline so a sticky
            // change event cannot cause an endless reload loop.
            file_observer::acknowledge(Path::new(path));
            return true;
        }
        self.selected.as_deref() != Some(path)
    }

    pub(crate) fn reconcile_recent_observers<I>(&mut self, paths: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.recent_observed_paths = paths.into_iter().collect();
        self.reconcile_project_observers();
    }

    pub(crate) fn reconcile_view_observers<I>(&mut self, paths: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.view_observed_paths = paths.into_iter().collect();
        self.reconcile_project_observers();
    }

    pub(crate) fn clear_view_observers(&mut self) {
        if self.view_observed_paths.is_empty() {
            return;
        }
        self.view_observed_paths.clear();
        self.reconcile_project_observers();
    }

    pub(crate) fn forget_observed_path(&mut self, path: &str) {
        self.recent_observed_paths.remove(path);
        self.view_observed_paths.remove(path);
        self.reconcile_project_observers();
    }

    fn reconcile_project_observers(&mut self) {
        let desired = self
            .recent_observed_paths
            .union(&self.view_observed_paths)
            .cloned()
            .collect::<BTreeSet<_>>();

        for path in desired.difference(&self.registered_project_observers) {
            file_observer::observe(Path::new(path), ExternalFileRole::Project);
        }
        for path in self.registered_project_observers.difference(&desired) {
            file_observer::release(Path::new(path), ExternalFileRole::Project);
        }
        self.registered_project_observers = desired;
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
        self.forget_observed_path(path);
        if self.selected.as_deref() == Some(path) {
            self.clear_selection();
        }
    }
}

impl Drop for ProjectViewState {
    fn drop(&mut self) {
        for path in std::mem::take(&mut self.registered_project_observers) {
            file_observer::release(Path::new(&path), ExternalFileRole::Project);
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
        assert!(state.recent_observed_paths.is_empty());
        assert!(state.view_observed_paths.is_empty());
        assert!(state.registered_project_observers.is_empty());
    }

    #[test]
    fn preview_load_is_only_needed_when_selection_changes_without_external_change() {
        let mut state = ProjectViewState::default();
        assert!(state.needs_preview_load("a.shade"));
        state.selected = Some("a.shade".to_owned());
        assert!(!state.needs_preview_load("a.shade"));
        assert!(state.needs_preview_load("b.shade"));
        file_observer::release(Path::new("a.shade"), ExternalFileRole::Project);
        file_observer::release(Path::new("b.shade"), ExternalFileRole::Project);
    }

    #[test]
    fn recent_and_view_scopes_share_one_project_role_until_both_release() {
        let mut state = ProjectViewState::default();
        state.reconcile_recent_observers(["shared.shade".to_owned(), "recent.shade".to_owned()]);
        state.reconcile_view_observers(["shared.shade".to_owned(), "view.shade".to_owned()]);
        assert_eq!(state.registered_project_observers.len(), 3);
        assert!(
            file_observer::snapshot(Path::new("shared.shade")).is_some(),
            "the shared Project role must remain observed while either scope owns it"
        );

        state.reconcile_recent_observers(std::iter::empty::<String>());
        assert!(state.registered_project_observers.contains("shared.shade"));
        assert!(!state.registered_project_observers.contains("recent.shade"));
        assert!(file_observer::snapshot(Path::new("shared.shade")).is_some());

        state.clear_view_observers();
        assert!(state.registered_project_observers.is_empty());
        assert!(file_observer::snapshot(Path::new("shared.shade")).is_none());
    }

    #[test]
    fn forgetting_selected_path_clears_selection_lru_and_observer_ownership() {
        let mut state = ProjectViewState::default();
        state.selected = Some("a.shade".to_owned());
        state.preview_error = Some("old preview".to_owned());
        state.list_texture_lru.push_back("a.shade".to_owned());
        state.list_texture_lru.push_back("b.shade".to_owned());
        state.reconcile_view_observers(["a.shade".to_owned()]);
        state.forget_path("a.shade");
        assert!(state.selected.is_none());
        assert!(state.preview_error.is_none());
        assert_eq!(state.list_texture_lru.len(), 1);
        assert_eq!(
            state.list_texture_lru.front().map(String::as_str),
            Some("b.shade")
        );
        assert!(!state.registered_project_observers.contains("a.shade"));
    }
}
