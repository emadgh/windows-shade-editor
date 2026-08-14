from pathlib import Path
import re

ROOT = Path('.')


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    return text.replace(old, new, 1)


def regex_once(text: str, pattern: str, replacement: str, label: str, flags=0) -> str:
    new, count = re.subn(pattern, replacement, text, count=1, flags=flags)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 regex match, found {count}')
    return new

# ---------------------------------------------------------------------------
# Version
# ---------------------------------------------------------------------------
cargo = (ROOT / 'Cargo.toml').read_text()
cargo = replace_once(cargo, 'version = "0.14.2"', 'version = "0.15.0"', 'Cargo.toml version')
(ROOT / 'Cargo.toml').write_text(cargo)

lock = (ROOT / 'Cargo.lock').read_text()
lock = replace_once(
    lock,
    'name = "windows-shade-editor"\nversion = "0.14.2"',
    'name = "windows-shade-editor"\nversion = "0.15.0"',
    'Cargo.lock package version',
)
(ROOT / 'Cargo.lock').write_text(lock)

# ---------------------------------------------------------------------------
# Runtime history: max 50 states + conversion to persisted snapshot history.
# ---------------------------------------------------------------------------
history = r'''use std::collections::{BTreeMap, BTreeSet};

use crate::model::{
    ChannelAdjustment, MAX_SNAPSHOT_HISTORY_STATES, SnapshotAdjustmentHistory,
    SnapshotHistoryState,
};

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub label: String,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
}

#[derive(Clone, Debug, Default)]
pub struct AdjustmentHistory {
    entries: Vec<HistoryEntry>,
    cursor: usize,
}

impl AdjustmentHistory {
    pub fn reset(
        &mut self,
        adjustments: &BTreeMap<String, ChannelAdjustment>,
        label: impl Into<String>,
    ) {
        self.entries.clear();
        self.entries.push(HistoryEntry {
            label: label.into(),
            adjustments: adjustments.clone(),
        });
        self.cursor = 0;
    }

    pub fn from_persisted(
        persisted: &SnapshotAdjustmentHistory,
        fallback: &BTreeMap<String, ChannelAdjustment>,
        fallback_label: impl Into<String>,
    ) -> Self {
        if persisted.entries.is_empty() {
            let mut history = Self::default();
            history.reset(fallback, fallback_label);
            return history;
        }
        let mut entries = persisted
            .entries
            .iter()
            .map(|entry| HistoryEntry {
                label: entry.label.clone(),
                adjustments: entry.adjustments.clone(),
            })
            .collect::<Vec<_>>();
        if entries.len() > MAX_SNAPSHOT_HISTORY_STATES {
            let overflow = entries.len() - MAX_SNAPSHOT_HISTORY_STATES;
            entries.drain(0..overflow);
        }
        let cursor = persisted.cursor.min(entries.len().saturating_sub(1));
        Self { entries, cursor }
    }

    pub fn to_persisted(&self) -> SnapshotAdjustmentHistory {
        SnapshotAdjustmentHistory {
            entries: self
                .entries
                .iter()
                .map(|entry| SnapshotHistoryState {
                    label: entry.label.clone(),
                    adjustments: entry.adjustments.clone(),
                })
                .collect(),
            cursor: self.cursor.min(self.entries.len().saturating_sub(1)),
        }
    }

    pub fn record(
        &mut self,
        adjustments: &BTreeMap<String, ChannelAdjustment>,
        label: impl Into<String>,
    ) -> bool {
        if self.entries.is_empty() {
            self.reset(adjustments, label);
            return true;
        }
        if self.entries[self.cursor].adjustments == *adjustments {
            return false;
        }
        self.entries.truncate(self.cursor + 1);
        self.entries.push(HistoryEntry {
            label: label.into(),
            adjustments: adjustments.clone(),
        });
        if self.entries.len() > MAX_SNAPSHOT_HISTORY_STATES {
            let overflow = self.entries.len() - MAX_SNAPSHOT_HISTORY_STATES;
            self.entries.drain(0..overflow);
        }
        self.cursor = self.entries.len().saturating_sub(1);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.entries.is_empty() && self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    pub fn undo(&mut self) -> Option<BTreeMap<String, ChannelAdjustment>> {
        if !self.can_undo() {
            return None;
        }
        self.cursor -= 1;
        Some(self.entries[self.cursor].adjustments.clone())
    }

    pub fn redo(&mut self) -> Option<BTreeMap<String, ChannelAdjustment>> {
        if !self.can_redo() {
            return None;
        }
        self.cursor += 1;
        Some(self.entries[self.cursor].adjustments.clone())
    }

    pub fn jump(&mut self, index: usize) -> Option<BTreeMap<String, ChannelAdjustment>> {
        if index >= self.entries.len() {
            return None;
        }
        self.cursor = index;
        Some(self.entries[index].adjustments.clone())
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn current_matches(&self, adjustments: &BTreeMap<String, ChannelAdjustment>) -> bool {
        self.entries
            .get(self.cursor)
            .is_some_and(|entry| entry.adjustments == *adjustments)
    }
}

pub fn describe_change(
    before: &BTreeMap<String, ChannelAdjustment>,
    after: &BTreeMap<String, ChannelAdjustment>,
) -> String {
    let keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut channels = Vec::new();
    let mut kinds = BTreeSet::new();

    for name in keys {
        let old = before.get(&name).cloned().unwrap_or_default();
        let new = after.get(&name).cloned().unwrap_or_default();
        if old == new {
            continue;
        }
        channels.push(name);
        if old.enabled != new.enabled {
            kinds.insert("Enable");
        }
        if old.levels != new.levels {
            kinds.insert("Levels");
        }
        if old.curve != new.curve {
            kinds.insert("Curve");
        }
        if old.mixer != new.mixer {
            kinds.insert("Mixer");
        }
    }

    let kind = if kinds.len() == 1 {
        kinds.iter().next().copied().unwrap_or("Adjustments")
    } else {
        "Adjustments"
    };
    if channels.len() == 1 {
        format!("{kind} - {}", channels[0])
    } else if channels.is_empty() {
        "Adjustments".to_owned()
    } else {
        format!("{kind} - {} channels", channels.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(gamma: f32) -> BTreeMap<String, ChannelAdjustment> {
        let mut map = BTreeMap::new();
        let mut item = ChannelAdjustment::default();
        item.levels.gamma = gamma;
        map.insert("Cyan".to_owned(), item);
        map
    }

    #[test]
    fn history_undo_redo_and_branching() {
        let mut history = AdjustmentHistory::default();
        let a = state(1.0);
        let b = state(1.2);
        let c = state(1.4);
        history.reset(&a, "Start");
        assert!(history.record(&b, "Levels - Cyan"));
        assert!(history.record(&c, "Levels - Cyan"));
        assert_eq!(history.undo().unwrap(), b);
        assert_eq!(history.undo().unwrap(), a);
        assert_eq!(history.redo().unwrap(), b);
        let d = state(1.8);
        assert!(history.record(&d, "Levels - Cyan"));
        assert!(!history.can_redo());
    }

    #[test]
    fn history_is_capped_at_fifty_states() {
        let mut history = AdjustmentHistory::default();
        history.reset(&state(1.0), "Start");
        for index in 1..80 {
            history.record(&state(1.0 + index as f32 / 100.0), format!("State {index}"));
        }
        assert_eq!(history.len(), MAX_SNAPSHOT_HISTORY_STATES);
        assert_eq!(history.cursor(), MAX_SNAPSHOT_HISTORY_STATES - 1);
    }

    #[test]
    fn persisted_history_roundtrips_cursor_and_states() {
        let mut history = AdjustmentHistory::default();
        history.reset(&state(1.0), "Start");
        history.record(&state(1.2), "Second");
        history.record(&state(1.4), "Third");
        history.undo();
        let persisted = history.to_persisted();
        let restored = AdjustmentHistory::from_persisted(&persisted, &state(9.0), "Fallback");
        assert_eq!(restored.len(), 3);
        assert_eq!(restored.cursor(), 1);
        assert!(restored.current_matches(&state(1.2)));
    }

    #[test]
    fn change_description_detects_adjustment_type() {
        let a = state(1.0);
        let b = state(1.5);
        assert_eq!(describe_change(&a, &b), "Levels - Cyan");
    }
}
'''
(ROOT / 'src/history.rs').write_text(history)

# ---------------------------------------------------------------------------
# Persisted history inside each Snapshot (.shade schema stays v9).
# ---------------------------------------------------------------------------
model_path = ROOT / 'src/model_v6.rs'
model = model_path.read_text()
model = replace_once(
    model,
    'pub const TEST_CODE_ALL_CHANNELS: &str = "__all_channels__";\n',
    'pub const TEST_CODE_ALL_CHANNELS: &str = "__all_channels__";\n'
    'pub const MAX_SNAPSHOT_HISTORY_STATES: usize = 50;\n',
    'history limit constant',
)
model = replace_once(
    model,
    '            name: "Untitled Shade".to_owned(),',
    '            name: String::new(),',
    'blank default project title',
)
model = replace_once(
    model,
    '''#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdjustmentSnapshot {
''',
    '''#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct SnapshotHistoryState {
    pub label: String,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct SnapshotAdjustmentHistory {
    pub entries: Vec<SnapshotHistoryState>,
    pub cursor: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdjustmentSnapshot {
''',
    'snapshot history structs',
)
model = replace_once(
    model,
    '''    #[serde(default)]
    pub exports: Vec<SnapshotExportRecord>,
}''',
    '''    #[serde(default)]
    pub exports: Vec<SnapshotExportRecord>,
    /// Adjustment undo/redo history owned by this Snapshot. Stored in the
    /// .shade project so switching Snapshots and reopening a project preserves
    /// each Snapshot's independent edit trail.
    #[serde(default)]
    pub history: SnapshotAdjustmentHistory,
}''',
    'snapshot history field',
)
model = replace_once(
    model,
    '''        let project: Self =
            serde_json::from_str(&text).map_err(|err| format!("Invalid .shade file: {err}"))?;
        if project.schema_version != SHADE_SCHEMA_VERSION {''',
    '''        let mut project: Self =
            serde_json::from_str(&text).map_err(|err| format!("Invalid .shade file: {err}"))?;
        if project.schema_version != SHADE_SCHEMA_VERSION {''',
    'mutable project load',
)
model = replace_once(
    model,
    '''        }
        Ok(project)
    }

    pub fn save(&self, path: &Path, resolved_face_paths: &[PathBuf]) -> Result<(), String> {''',
    '''        }
        project.ensure_snapshot_histories();
        Ok(project)
    }

    pub fn save(&self, path: &Path, resolved_face_paths: &[PathBuf]) -> Result<(), String> {''',
    'hydrate histories on load',
)
model = replace_once(
    model,
    '''        self.snapshots.push(AdjustmentSnapshot {
            id,
            name,
            created_at_unix_ms: now_unix_ms(),
            adjustments: self.adjustments.clone(),
            exports: Vec::new(),
        });''',
    '''        self.snapshots.push(AdjustmentSnapshot {
            id,
            name,
            created_at_unix_ms: now_unix_ms(),
            adjustments: self.adjustments.clone(),
            exports: Vec::new(),
            history: SnapshotAdjustmentHistory {
                entries: vec![SnapshotHistoryState {
                    label: "Snapshot created".to_owned(),
                    adjustments: self.adjustments.clone(),
                }],
                cursor: 0,
            },
        });''',
    'new snapshot history',
)
model = replace_once(
    model,
    '''    pub fn reset_adjustments(&mut self, names: &[String]) {
        self.adjustments.clear();
        ensure_adjustment_channels(&mut self.adjustments, names);
    }

    fn next_snapshot_name(&self) -> String {''',
    '''    pub fn reset_adjustments(&mut self, names: &[String]) {
        self.adjustments.clear();
        ensure_adjustment_channels(&mut self.adjustments, names);
    }

    pub fn ensure_snapshot_histories(&mut self) {
        for snapshot in &mut self.snapshots {
            if snapshot.history.entries.is_empty() {
                snapshot.history.entries.push(SnapshotHistoryState {
                    label: "Snapshot state".to_owned(),
                    adjustments: snapshot.adjustments.clone(),
                });
                snapshot.history.cursor = 0;
                continue;
            }
            if snapshot.history.entries.len() > MAX_SNAPSHOT_HISTORY_STATES {
                let overflow = snapshot.history.entries.len() - MAX_SNAPSHOT_HISTORY_STATES;
                snapshot.history.entries.drain(0..overflow);
                snapshot.history.cursor = snapshot.history.cursor.saturating_sub(overflow);
            }
            snapshot.history.cursor = snapshot
                .history
                .cursor
                .min(snapshot.history.entries.len().saturating_sub(1));
        }
    }

    fn next_snapshot_name(&self) -> String {''',
    'snapshot history sanitizer',
)
model_path.write_text(model)

# ---------------------------------------------------------------------------
# Project title field above Faces and snapshot Update flushes history first.
# ---------------------------------------------------------------------------
workflow_path = ROOT / 'src/workflow_v0103.rs'
workflow = workflow_path.read_text()
workflow = replace_once(
    workflow,
    '''pub(super) fn update_active_snapshot(app: &mut ShadeApp) {
    let Some(active_id) = app.project.active_snapshot_id else {
        return;
    };
    if app.project.update_snapshot(active_id) {''',
    '''pub(super) fn update_active_snapshot(app: &mut ShadeApp) {
    let Some(active_id) = app.project.active_snapshot_id else {
        return;
    };
    app.flush_history_now();
    app.sync_history_to_active_snapshot();
    if app.project.update_snapshot(active_id) {''',
    'snapshot update history flush',
)
workflow = replace_once(
    workflow,
    '''pub(super) fn ui_faces(app: &mut ShadeApp, ui: &mut egui::Ui) {
    ui.heading("Faces");''',
    '''pub(super) fn ui_faces(app: &mut ShadeApp, ui: &mut egui::Ui) {
    ui.label("Project title");
    if ui
        .add(
            egui::TextEdit::singleline(&mut app.project.name)
                .hint_text("Uses the .shade filename after first save")
                .desired_width(f32::INFINITY),
        )
        .changed()
    {
        app.project_dirty = true;
    }
    ui.add_space(4.0);
    ui.separator();
    ui.heading("Faces");''',
    'project title field',
)
workflow_path.write_text(workflow)

# ---------------------------------------------------------------------------
# Main UI / history lifecycle / histogram improvements.
# ---------------------------------------------------------------------------
app_path = ROOT / 'src/app_main.rs'
app = app_path.read_text()
app = replace_once(
    app,
    'const PREVIOUS_SHADE_TEXTURE_CACHE_LIMIT: usize = 64;\n',
    'const PREVIOUS_SHADE_TEXTURE_CACHE_LIMIT: usize = 64;\n'
    'const APP_WINDOW_TITLE: &str = concat!("Shader Editor v", env!("CARGO_PKG_VERSION"), " - (EmadGhasemi.ir)");\n',
    'window title constant',
)
app = replace_once(app, '.with_title("Shade Editor")', '.with_title(APP_WINDOW_TITLE)', 'viewport title')
app = replace_once(app, '        "Shade Editor",\n        native_options,', '        APP_WINDOW_TITLE,\n        native_options,', 'run_native title')
app = replace_once(
    app,
    '''    history: history::AdjustmentHistory,
    history_pending_label: Option<String>,''',
    '''    history: history::AdjustmentHistory,
    history_clear_backup: Option<(Option<u64>, history::AdjustmentHistory)>,
    history_pending_label: Option<String>,''',
    'history backup field',
)
app = replace_once(
    app,
    '''            history,
            history_pending_label: None,''',
    '''            history,
            history_clear_backup: None,
            history_pending_label: None,''',
    'history backup init',
)
app = replace_once(
    app,
    '''        self.history.reset(&self.project.adjustments, "New project");
        self.history_pending_label = None;''',
    '''        self.history.reset(&self.project.adjustments, "New project");
        self.history_clear_backup = None;
        self.history_pending_label = None;''',
    'new project history backup reset',
)
app = replace_once(
    app,
    '''        let mut project = self.project.clone();
        project.name = project_name_for_path(&project.name, &path);
        project.file_metadata = Some(build_project_file_metadata(''',
    '''        self.flush_history_now();
        self.sync_history_to_active_snapshot();
        self.project.name = project_name_for_path(&self.project.name, &path);
        let mut project = self.project.clone();
        project.ensure_snapshot_histories();
        project.file_metadata = Some(build_project_file_metadata(''',
    'save project name and history before clone',
)
app = replace_once(
    app,
    '''                    self.history
                        .reset(&self.project.adjustments, "Open project");
                    self.history_pending_label = None;''',
    '''                    self.load_history_for_active_snapshot("Open project");
                    self.history_clear_backup = None;
                    self.history_pending_label = None;''',
    'load project persisted history',
)
app = replace_once(
    app,
    '''                    self.history
                        .reset(&self.project.adjustments, "Recovered project");
                    self.history_pending_label = None;''',
    '''                    self.load_history_for_active_snapshot("Recovered project");
                    self.history_clear_backup = None;
                    self.history_pending_label = None;''',
    'recover persisted history',
)

# Snapshot switch: save current scope before switching, then restore target scope.
app = replace_once(
    app,
    '''    fn apply_snapshot_now(&mut self, id: u64) {
        if self.project.apply_snapshot(id) {''',
    '''    fn apply_snapshot_now(&mut self, id: u64) {
        self.flush_history_now();
        self.sync_history_to_active_snapshot();
        if self.project.apply_snapshot(id) {''',
    'snapshot switch history flush',
)
app = replace_once(
    app,
    '''            self.history.reset(&self.project.adjustments, history_label);
            self.history_pending_label = None;''',
    '''            self.load_history_for_active_snapshot(&history_label);
            self.history_clear_backup = None;
            self.history_pending_label = None;''',
    'snapshot switch history restore',
)

# Replace all runtime history methods/UI in one coherent block.
history_methods = r'''    fn sync_history_to_active_snapshot(&mut self) -> bool {
        let Some(active_id) = self.project.active_snapshot_id else {
            return false;
        };
        let persisted = self.history.to_persisted();
        let Some(snapshot) = self
            .project
            .snapshots
            .iter_mut()
            .find(|snapshot| snapshot.id == active_id)
        else {
            return false;
        };
        if snapshot.history == persisted {
            return false;
        }
        snapshot.history = persisted;
        self.project_dirty = true;
        true
    }

    fn load_history_for_active_snapshot(&mut self, fallback_label: &str) {
        let persisted = self.project.active_snapshot_id.and_then(|active_id| {
            self.project
                .snapshots
                .iter()
                .find(|snapshot| snapshot.id == active_id)
                .map(|snapshot| snapshot.history.clone())
        });
        self.history = if let Some(persisted) = persisted {
            history::AdjustmentHistory::from_persisted(
                &persisted,
                &self.project.adjustments,
                fallback_label,
            )
        } else {
            let mut history = history::AdjustmentHistory::default();
            history.reset(&self.project.adjustments, fallback_label);
            history
        };
        self.history_pending_label = None;
        self.history_pending_at = None;
    }

    fn flush_history_now(&mut self) {
        if let Some(label) = self.history_pending_label.take() {
            self.history_pending_at = None;
            if self.history.record(&self.project.adjustments, label) {
                self.history_clear_backup = None;
            }
        }
        self.sync_history_to_active_snapshot();
    }

    fn queue_adjustment_history(&mut self, before: &BTreeMap<String, ChannelAdjustment>) {
        if *before == self.project.adjustments {
            return;
        }
        self.history_pending_label =
            Some(history::describe_change(before, &self.project.adjustments));
        self.history_pending_at = Some(Instant::now());
    }

    fn commit_pending_history(&mut self, ctx: &egui::Context, force: bool) {
        let Some(label) = self.history_pending_label.clone() else {
            return;
        };
        let ready = force
            || (self
                .history_pending_at
                .is_some_and(|at| at.elapsed() >= HISTORY_COMMIT_DELAY)
                && !ctx.input(|input| input.pointer.any_down()));
        if !ready {
            return;
        }
        if self.history.record(&self.project.adjustments, label) {
            self.history_clear_backup = None;
            self.sync_history_to_active_snapshot();
        }
        self.history_pending_label = None;
        self.history_pending_at = None;
    }

    fn apply_history_adjustments(
        &mut self,
        adjustments: BTreeMap<String, ChannelAdjustment>,
        message: &str,
    ) {
        self.project.adjustments = adjustments;
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.history_clear_backup = None;
        self.mark_all_previews_dirty();
        self.sync_history_to_active_snapshot();
        self.report_info(message);
    }

    fn undo_adjustment(&mut self, _ctx: &egui::Context) {
        self.flush_history_now();
        if let Some(adjustments) = self.history.undo() {
            self.apply_history_adjustments(adjustments, "Undo adjustment");
        }
    }

    fn redo_adjustment(&mut self, _ctx: &egui::Context) {
        self.flush_history_now();
        if let Some(adjustments) = self.history.redo() {
            self.apply_history_adjustments(adjustments, "Redo adjustment");
        }
    }

    fn handle_history_shortcuts(&mut self, ctx: &egui::Context) {
        let (undo, redo) = ctx.input(|input| {
            let z = input.key_pressed(egui::Key::Z);
            (
                z && input.modifiers.ctrl && input.modifiers.alt && !input.modifiers.shift,
                z && input.modifiers.ctrl && input.modifiers.shift && !input.modifiers.alt,
            )
        });
        if undo {
            self.undo_adjustment(ctx);
        } else if redo {
            self.redo_adjustment(ctx);
        }
    }

    fn ui_history(&mut self, ui: &mut egui::Ui) {
        let scope = self.project.active_snapshot_id;
        let can_undo_clear = self
            .history_clear_backup
            .as_ref()
            .is_some_and(|(backup_scope, _)| *backup_scope == scope);
        let mut clear = false;
        let mut undo_clear = false;
        ui.horizontal(|ui| {
            ui.strong("History");
            if ui
                .add_enabled(self.history.can_undo(), egui::Button::new("Undo").small())
                .on_hover_text("Ctrl+Alt+Z")
                .clicked()
            {
                self.undo_adjustment(ui.ctx());
            }
            if ui
                .add_enabled(self.history.can_redo(), egui::Button::new("Redo").small())
                .on_hover_text("Ctrl+Shift+Z")
                .clicked()
            {
                self.redo_adjustment(ui.ctx());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if can_undo_clear {
                    undo_clear = ui.small_button("Undo clear").clicked();
                }
                clear = ui
                    .add_enabled(self.history.len() > 1, egui::Button::new("Clear history").small())
                    .clicked();
            });
        });
        if let Some(name) = self.project.active_snapshot_name() {
            ui.small(format!("Snapshot: {name} · up to 50 adjustment states are saved in this .shade file."));
        } else {
            ui.small("Working adjustment history. Create/select a Snapshot to keep an independent saved history.");
        }

        if clear {
            self.flush_history_now();
            self.history_clear_backup = Some((scope, self.history.clone()));
            self.history.reset(&self.project.adjustments, "Current state");
            self.sync_history_to_active_snapshot();
            self.report_info("History cleared - Undo clear is available once");
        } else if undo_clear {
            if let Some((backup_scope, backup)) = self.history_clear_backup.take() {
                if backup_scope == scope {
                    self.history = backup;
                    self.sync_history_to_active_snapshot();
                    self.report_info("Cleared history restored");
                }
            }
        }

        let rows = self
            .history
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| (index, entry.label.clone()))
            .collect::<Vec<_>>();
        let cursor = self.history.cursor();
        let mut requested = None;
        egui::ScrollArea::vertical()
            .id_salt("adjustment-history")
            .max_height(210.0)
            .show(ui, |ui| {
                for (index, label) in rows {
                    if clickable_row(ui, index == cursor, &label, None, None, 28.0).clicked() {
                        requested = Some(index);
                    }
                }
            });
        if let Some(index) = requested {
            self.flush_history_now();
            if let Some(adjustments) = self.history.jump(index) {
                self.apply_history_adjustments(adjustments, "History state selected");
            }
        }
    }

    fn poll_autosave('''
app = regex_once(
    app,
    r'    fn queue_adjustment_history\(.*?\n    fn poll_autosave\(',
    history_methods,
    'history lifecycle block',
    flags=re.S,
)

# Snapshot toolbar: put New/export controls on the right.
app = replace_once(
    app,
    '''        ui.horizontal(|ui| {
            ui.heading("Snapshots");
            new_snapshot = ui.small_button("+ New").clicked();
            export_all = ui
                .add_enabled(
                    self.job.is_none() && !all_ids.is_empty() && !self.faces.is_empty(),
                    VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                )
                .on_hover_text("Export all snapshots for the active Face")
                .clicked();
            if all_exported {
                open_all_folder = ui
                    .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                    .on_hover_text("Open the latest export folder for these snapshots")
                    .clicked();
            }
        });''',
    '''        ui.horizontal(|ui| {
            ui.heading("Snapshots");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if all_exported {
                    open_all_folder = ui
                        .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                        .on_hover_text("Open the latest export folder for these snapshots")
                        .clicked();
                }
                export_all = ui
                    .add_enabled(
                        self.job.is_none() && !all_ids.is_empty() && !self.faces.is_empty(),
                        VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                    )
                    .on_hover_text("Export all snapshots for the active Face")
                    .clicked();
                new_snapshot = ui.small_button("+ New").clicked();
            });
        });''',
    'snapshot top toolbar alignment',
)
app = replace_once(
    app,
    '''            ui.horizontal(|ui| {
                ui.strong(&day);
                if ui
                    .add_enabled(
                        self.job.is_none() && !day_ids.is_empty() && !self.faces.is_empty(),
                        VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                    )
                    .on_hover_text("Export all snapshots from this day for the active Face")
                    .clicked()
                {
                    requested_group_export = Some((day_ids.clone(), day.clone()));
                }
                if day_exported
                    && ui
                        .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                        .on_hover_text("Open the latest export folder for this day")
                        .clicked()
                {
                    requested_folder = day_latest_folder.clone();
                }
            });''',
    '''            ui.horizontal(|ui| {
                ui.strong(&day);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if day_exported
                        && ui
                            .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))
                            .on_hover_text("Open the latest export folder for this day")
                            .clicked()
                    {
                        requested_folder = day_latest_folder.clone();
                    }
                    if ui
                        .add_enabled(
                            self.job.is_none() && !day_ids.is_empty() && !self.faces.is_empty(),
                            VectorIconButton::export().min_size(egui::vec2(20.0, 20.0)),
                        )
                        .on_hover_text("Export all snapshots from this day for the active Face")
                        .clicked()
                    {
                        requested_group_export = Some((day_ids.clone(), day.clone()));
                    }
                });
            });''',
    'snapshot day toolbar alignment',
)
app = replace_once(
    app,
    '''        if new_snapshot {
            let id = self.project.create_snapshot();''',
    '''        if new_snapshot {
            self.flush_history_now();
            self.sync_history_to_active_snapshot();
            let id = self.project.create_snapshot();''',
    'new snapshot flush previous history',
)
app = replace_once(
    app,
    '''            self.project_dirty = true;
        }
        if export_all {''',
    '''            self.load_history_for_active_snapshot("Snapshot created");
            self.history_clear_backup = None;
            self.project_dirty = true;
        }
        if export_all {''',
    'new snapshot runtime history',
)
app = replace_once(
    app,
    '''        if delete && self.project.delete_snapshot(active_id) {
            self.snapshot_rename_id = None;
            self.snapshot_rename_buffer.clear();
            self.project_dirty = true;''',
    '''        if delete && self.project.delete_snapshot(active_id) {
            self.snapshot_rename_id = None;
            self.snapshot_rename_buffer.clear();
            self.history.reset(&self.project.adjustments, "Snapshot deleted");
            self.history_clear_backup = None;
            self.project_dirty = true;''',
    'delete snapshot history state',
)

# Window title / About website.
app = replace_once(
    app,
    '''                ui.hyperlink_to(
                    "GitHub repository",
                    "https://github.com/emadgh/windows-shade-editor",
                );''',
    '''                ui.hyperlink_to(
                    "GitHub repository",
                    "https://github.com/emadgh/windows-shade-editor",
                );
                ui.hyperlink_to("EmadGhasemi.ir", "https://emadghasemi.ir");''',
    'about website',
)

# ---------------------------------------------------------------------------
# Curve histogram: source/before + adjusted/after overlay, broadcast neutral.
# ---------------------------------------------------------------------------
app = replace_once(
    app,
    '''        let all_adjusted_histograms = face
            .adjusted
            .iter()
            .map(|values| render::histogram(values))
            .collect::<Vec<_>>();
        let active_histogram = all_adjusted_histograms.get(self.selected_channel).copied();''',
    '''        let all_original_histograms = face.preview.histograms.clone();
        let all_adjusted_histograms = face
            .adjusted
            .iter()
            .map(|values| render::histogram(values))
            .collect::<Vec<_>>();
        let active_original_histogram = all_original_histograms.get(self.selected_channel).copied();
        let active_adjusted_histogram = all_adjusted_histograms.get(self.selected_channel).copied();''',
    'curve before after histogram sources',
)
app = replace_once(
    app,
    '''                        &channel_names,
                        active_histogram.as_ref(),
                        control_accent,''',
    '''                        &channel_names,
                        active_original_histogram.as_ref(),
                        active_adjusted_histogram.as_ref(),
                        control_accent,''',
    'selected adjustment histogram args',
)
app = replace_once(
    app,
    '''                        &output_name,
                        &channel_names,
                        &all_adjusted_histograms,
                        control_accent,''',
    '''                        &output_name,
                        &channel_names,
                        &all_original_histograms,
                        &all_adjusted_histograms,
                        control_accent,''',
    'all adjustment histogram args',
)
app = replace_once(
    app,
    '''        channel_names: &[String],
        histogram: Option<&[u32; 256]>,
        accent: Option<egui::Color32>,''',
    '''        channel_names: &[String],
        histogram_before: Option<&[u32; 256]>,
        histogram_after: Option<&[u32; 256]>,
        accent: Option<egui::Color32>,''',
    'selected adjustment signature',
)
app = app.replace(
    'histogram.filter(|_| self.settings.show_curve_histogram),\n                        accent,',
    'histogram_before.filter(|_| self.settings.show_curve_histogram),\n                        histogram_after.filter(|_| self.settings.show_curve_histogram),\n                        accent,',
)
if app.count('histogram.filter(|_| self.settings.show_curve_histogram)') != 0:
    raise SystemExit('selected curve histogram call replacement incomplete')
app = replace_once(
    app,
    '''        channel_names: &[String],
        histograms: &[[u32; 256]],
        accent: Option<egui::Color32>,''',
    '''        channel_names: &[String],
        histograms_before: &[[u32; 256]],
        histograms_after: &[[u32; 256]],
        accent: Option<egui::Color32>,''',
    'all adjustment signature',
)
app = app.replace(
    '''                    channel_names,
                    histograms,
                    self.settings.colorize_adjustments,''',
    '''                    channel_names,
                    histograms_before,
                    histograms_after,
                    self.settings.colorize_adjustments,''',
)
if app.count('                    histograms,\n                    self.settings.colorize_adjustments,') != 0:
    raise SystemExit('all curves call replacement incomplete')

app = replace_once(
    app,
    '''fn curve_editor_graph(
    ui: &mut egui::Ui,
    curve: &mut model::Curve,
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
) -> (bool, CurvePointKind) {''',
    '''fn curve_editor_graph(
    ui: &mut egui::Ui,
    curve: &mut model::Curve,
    histogram_before: Option<&[u32; 256]>,
    histogram_after: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    neutral_histogram: bool,
) -> (bool, CurvePointKind) {''',
    'curve graph signature',
)
app = regex_once(
    app,
    r'''    if let Some\(bins\) = histogram \{.*?    \}\n    painter\.line_segment\(''',
    '''    if histogram_before.is_some() || histogram_after.is_some() {
        let max_value = histogram_before
            .into_iter()
            .chain(histogram_after)
            .flat_map(|bins| bins.iter())
            .copied()
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let before_color = ui.visuals().weak_text_color().gamma_multiply(0.20);
        let after_base = if neutral_histogram {
            ui.visuals().weak_text_color()
        } else {
            accent.unwrap_or(ui.visuals().selection.stroke.color)
        };
        let after_color = after_base.gamma_multiply(0.48);
        for (bins, color) in [
            (histogram_before, before_color),
            (histogram_after, after_color),
        ] {
            if let Some(bins) = bins {
                for (index, value) in bins.iter().enumerate() {
                    let x = egui::lerp(rect.x_range(), index as f32 / 255.0);
                    let h = *value as f32 / max_value * rect.height();
                    painter.line_segment(
                        [
                            egui::pos2(x, rect.bottom()),
                            egui::pos2(x, rect.bottom() - h),
                        ],
                        egui::Stroke::new(1.0, color),
                    );
                }
            }
        }
    }
    painter.line_segment(''',
    'curve histogram painter',
    flags=re.S,
)
app = replace_once(
    app,
    '''fn curves_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    compact_controls: bool,
) -> bool {
    with_accent(ui, accent, |ui| {
        let (graph_changed, selected) =
            curve_editor_graph(ui, &mut adjustment.curve, histogram, accent);''',
    '''fn curves_ui(
    ui: &mut egui::Ui,
    adjustment: &mut ChannelAdjustment,
    histogram_before: Option<&[u32; 256]>,
    histogram_after: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    compact_controls: bool,
    neutral_histogram: bool,
) -> bool {
    with_accent(ui, accent, |ui| {
        let (graph_changed, selected) = curve_editor_graph(
            ui,
            &mut adjustment.curve,
            histogram_before,
            histogram_after,
            accent,
            neutral_histogram,
        );
        if histogram_before.is_some() && histogram_after.is_some() {
            ui.horizontal(|ui| {
                ui.colored_label(ui.visuals().weak_text_color(), "Before");
                let after_color = if neutral_histogram {
                    ui.visuals().weak_text_color()
                } else {
                    accent.unwrap_or(ui.visuals().selection.stroke.color)
                };
                ui.colored_label(after_color, "After");
            });
        }''',
    'curves ui signature/legend',
)
app = replace_once(
    app,
    '''fn broadcast_curves_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    histogram: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    compact_controls: bool,
) -> bool {
    let mut draft = adjustments.get(template_name).cloned().unwrap_or_default();
    if !curves_ui(ui, &mut draft, histogram, accent, compact_controls) {''',
    '''fn broadcast_curves_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    histogram_before: Option<&[u32; 256]>,
    histogram_after: Option<&[u32; 256]>,
    accent: Option<egui::Color32>,
    compact_controls: bool,
) -> bool {
    let mut draft = adjustments.get(template_name).cloned().unwrap_or_default();
    if !curves_ui(
        ui,
        &mut draft,
        histogram_before,
        histogram_after,
        accent,
        compact_controls,
        true,
    ) {''',
    'broadcast curve histograms',
)
app = replace_once(
    app,
    '''fn all_curves_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    histograms: &[[u32; 256]],
    colorize: bool,''',
    '''fn all_curves_ui(
    ui: &mut egui::Ui,
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    template_name: &str,
    channel_names: &[String],
    histograms_before: &[[u32; 256]],
    histograms_after: &[[u32; 256]],
    colorize: bool,''',
    'all curves signature',
)
app = replace_once(
    app,
    '''    let broadcast_histogram = show_histogram
        .then(|| histograms.get(template_index))
        .flatten();''',
    '''    let broadcast_histogram_before = show_histogram
        .then(|| histograms_before.get(template_index))
        .flatten();
    let broadcast_histogram_after = show_histogram
        .then(|| histograms_after.get(template_index))
        .flatten();''',
    'broadcast before after histogram',
)
app = replace_once(
    app,
    '''                channel_names,
                broadcast_histogram,
                broadcast_accent,
                compact_controls,''',
    '''                channel_names,
                broadcast_histogram_before,
                broadcast_histogram_after,
                broadcast_accent,
                compact_controls,''',
    'broadcast curves call',
)
app = replace_once(
    app,
    '''                let histogram = if show_histogram {
                    histograms.get(index)
                } else {
                    None
                };
                let adjustment = adjustments.entry(name.clone()).or_default();
                changed |= curves_ui(ui, adjustment, histogram, accent, compact_controls);''',
    '''                let histogram_before = if show_histogram {
                    histograms_before.get(index)
                } else {
                    None
                };
                let histogram_after = if show_histogram {
                    histograms_after.get(index)
                } else {
                    None
                };
                let adjustment = adjustments.entry(name.clone()).or_default();
                changed |= curves_ui(
                    ui,
                    adjustment,
                    histogram_before,
                    histogram_after,
                    accent,
                    compact_controls,
                    false,
                );''',
    'per channel curve before after',
)
# Selected curve calls now need the neutral_histogram=false argument.
app = app.replace(
    '''histogram_after.filter(|_| self.settings.show_curve_histogram),
                        accent,
                        compact_curve_controls,
                    )''',
    '''histogram_after.filter(|_| self.settings.show_curve_histogram),
                        accent,
                        compact_curve_controls,
                        false,
                    )''',
)
# Catch direct selected curves call in tabs/foldout after rustfmt-independent text.
app = app.replace(
    '''histogram_after.filter(|_| self.settings.show_curve_histogram),
                            accent,
                            compact_curve_controls,
                        )''',
    '''histogram_after.filter(|_| self.settings.show_curve_histogram),
                            accent,
                            compact_curve_controls,
                            false,
                        )''',
)

app_path.write_text(app)

print('Applied v0.15.0 snapshot history / project UI migration')
