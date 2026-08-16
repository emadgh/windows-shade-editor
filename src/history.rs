use std::collections::{BTreeMap, BTreeSet};

use crate::model::{
    ChannelAdjustment, DEFAULT_HISTORY_STEPS, MASTER_ADJUSTMENT_KEY, MAX_SNAPSHOT_HISTORY_STATES,
    SnapshotAdjustmentHistory, SnapshotHistoryState,
};

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub label: String,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
}

#[derive(Clone, Debug)]
pub struct AdjustmentHistory {
    entries: Vec<HistoryEntry>,
    cursor: usize,
    limit: usize,
}

impl Default for AdjustmentHistory {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
            limit: DEFAULT_HISTORY_STEPS,
        }
    }
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
        Self {
            entries,
            cursor,
            limit: DEFAULT_HISTORY_STEPS,
        }
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
        self.trim_to_limit();
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

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.clamp(1, MAX_SNAPSHOT_HISTORY_STATES);
        self.trim_to_limit();
    }

    fn trim_to_limit(&mut self) {
        if self.entries.len() <= self.limit {
            return;
        }
        let start = (self.cursor + 1).saturating_sub(self.limit);
        if start > 0 {
            self.entries.drain(0..start);
            self.cursor = self.cursor.saturating_sub(start);
        }
        if self.entries.len() > self.limit {
            self.entries.truncate(self.limit);
            self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        }
    }

    pub fn current_matches(&self, adjustments: &BTreeMap<String, ChannelAdjustment>) -> bool {
        self.entries
            .get(self.cursor)
            .is_some_and(|entry| entry.adjustments == *adjustments)
    }

    /// Discard working history states that happened after a saved Snapshot state.
    /// This keeps undo history up to the saved adjustment map while ensuring a
    /// discarded branch cannot be restored accidentally through Redo.
    pub fn discard_to_state(
        &mut self,
        adjustments: &BTreeMap<String, ChannelAdjustment>,
        fallback_label: impl Into<String>,
    ) {
        if let Some(index) = self
            .entries
            .iter()
            .rposition(|entry| entry.adjustments == *adjustments)
        {
            self.entries.truncate(index + 1);
            self.cursor = index;
        } else {
            self.reset(adjustments, fallback_label);
        }
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
        channels.push(if name == MASTER_ADJUSTMENT_KEY {
            "Master".to_owned()
        } else {
            name
        });
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
        format!("{kind} · {}", channels[0])
    } else if channels.is_empty() {
        "Adjustments".to_owned()
    } else {
        format!("{kind} · {} channels", channels.len())
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
        assert!(history.record(&b, "Levels · Cyan"));
        assert!(history.record(&c, "Levels · Cyan"));
        assert_eq!(history.undo().unwrap(), b);
        assert_eq!(history.undo().unwrap(), a);
        assert_eq!(history.redo().unwrap(), b);
        let d = state(1.8);
        assert!(history.record(&d, "Levels · Cyan"));
        assert!(!history.can_redo());
    }

    #[test]
    fn discard_to_saved_state_removes_redo_branch() {
        let mut history = AdjustmentHistory::default();
        let a = state(1.0);
        let b = state(1.2);
        let c = state(1.4);
        history.reset(&a, "Start");
        history.record(&b, "Second");
        history.record(&c, "Third");
        history.discard_to_state(&b, "Snapshot state");
        assert_eq!(history.len(), 2);
        assert!(history.current_matches(&b));
        assert!(!history.can_redo());
    }

    #[test]
    fn history_is_capped_at_fifty_states() {
        let mut history = AdjustmentHistory::default();
        history.reset(&state(1.0), "Start");
        for index in 1..80 {
            history.record(&state(1.0 + index as f32 / 100.0), format!("State {index}"));
        }
        assert_eq!(history.len(), DEFAULT_HISTORY_STEPS);
        assert_eq!(history.cursor(), DEFAULT_HISTORY_STEPS - 1);
    }

    #[test]
    fn reducing_history_limit_preserves_current_state_and_future_cap() {
        let mut history = AdjustmentHistory::default();
        history.reset(&state(1.0), "Start");
        for index in 1..12 {
            history.record(&state(1.0 + index as f32 / 100.0), format!("State {index}"));
        }
        history.undo();
        history.undo();
        let current = history.entries()[history.cursor()].adjustments.clone();
        history.set_limit(5);
        assert_eq!(history.limit(), 5);
        assert!(history.len() <= 5);
        assert_eq!(history.entries()[history.cursor()].adjustments, current);
        for index in 20..30 {
            history.record(&state(1.0 + index as f32 / 100.0), format!("State {index}"));
        }
        assert_eq!(history.len(), 5);
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
    fn master_change_description_uses_all_channels_label() {
        let mut before = BTreeMap::new();
        before.insert(
            MASTER_ADJUSTMENT_KEY.to_owned(),
            ChannelAdjustment::default(),
        );
        let mut after = before.clone();
        after.get_mut(MASTER_ADJUSTMENT_KEY).unwrap().curve.midpoint = 0.6;
        assert_eq!(describe_change(&before, &after), "Curve · Master");
    }

    #[test]
    fn change_description_detects_adjustment_type() {
        let a = state(1.0);
        let b = state(1.5);
        assert_eq!(describe_change(&a, &b), "Levels · Cyan");
    }
}
