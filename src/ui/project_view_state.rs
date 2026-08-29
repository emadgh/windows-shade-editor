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
        if file_observer::rescan(Path::new(path)).is_some_and(|observed| observed.is_changed()) {
            // The caller immediately reloads inspection data when this returns true. Accept the
            // current filesystem fingerprint as that inspection attempt's baseline so a sticky
            // change event cannot cause an endless reload loop. Rescan deliberately does not
            // create a Project subscription: ownership belongs to the reconciled UI scopes.
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

    /// Reconcile every Project path touched by this render frame before applying the
    /// window-open lifecycle boundary. This matters on the exact frame the user closes
    /// Project View: render-time `observe()` calls have already happened, so those paths
    /// must first become owned before the View scope can release them deterministically.
    pub(crate) fn finish_view_observer_frame<I>(&mut self, open: bool, paths: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.reconcile_view_observers(paths);
        if !open {
            self.clear_view_observers();
        }
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PROJECT_PATH: AtomicU64 = AtomicU64::new(1);

    fn unique_project_path(label: &str) -> String {
        let id = NEXT_PROJECT_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "shade-project-view-{label}-{}-{id}.shade",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned()
    }

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
        let a = unique_project_path("preview-a");
        let b = unique_project_path("preview-b");
        assert!(state.needs_preview_load(&a));
        state.selected = Some(a.clone());
        assert!(!state.needs_preview_load(&a));
        assert!(state.needs_preview_load(&b));
        assert!(file_observer::snapshot(Path::new(&a)).is_none());
        assert!(file_observer::snapshot(Path::new(&b)).is_none());
    }

    #[test]
    fn needs_preview_load_does_not_create_unowned_project_subscription() {
        let state = ProjectViewState::default();
        let path = unique_project_path("preview-unowned");
        assert!(file_observer::snapshot(Path::new(&path)).is_none());
        assert!(state.needs_preview_load(&path));
        assert!(
            file_observer::snapshot(Path::new(&path)).is_none(),
            "preview reload checks must not create anonymous Project subscriptions"
        );
    }

    #[test]
    fn closing_frame_releases_project_first_observed_during_that_frame() {
        let mut state = ProjectViewState::default();
        let path = unique_project_path("close-frame");

        // Simulate the direct render-time observation that occurs before egui reports
        // that the window was closed in this same frame.
        file_observer::observe(Path::new(&path), ExternalFileRole::Project);
        assert!(file_observer::snapshot(Path::new(&path)).is_some());

        state.finish_view_observer_frame(false, [path.clone()]);
        assert!(
            file_observer::snapshot(Path::new(&path)).is_none(),
            "the closing frame must adopt and then release every View-only observation"
        );
    }

    #[test]
    fn closing_frame_preserves_path_still_owned_by_recent_projects() {
        let mut state = ProjectViewState::default();
        let path = unique_project_path("close-shared-recent");
        state.reconcile_recent_observers([path.clone()]);

        // The View renders the same path and then closes. The role is shared logically,
        // so closing only the View scope must keep the Recent scope alive.
        file_observer::observe(Path::new(&path), ExternalFileRole::Project);
        state.finish_view_observer_frame(false, [path.clone()]);
        assert!(file_observer::snapshot(Path::new(&path)).is_some());

        state.reconcile_recent_observers(std::iter::empty::<String>());
        assert!(file_observer::snapshot(Path::new(&path)).is_none());
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
