use std::collections::{BTreeMap, BTreeSet};

use crate::model::ChannelAdjustment;

const MAX_HISTORY_STATES: usize = 120;

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
    pub fn reset(&mut self, adjustments: &BTreeMap<String, ChannelAdjustment>, label: impl Into<String>) {
        self.entries.clear();
        self.entries.push(HistoryEntry {
            label: label.into(),
            adjustments: adjustments.clone(),
        });
        self.cursor = 0;
    }

    pub fn record(&mut self, adjustments: &BTreeMap<String, ChannelAdjustment>, label: impl Into<String>) -> bool {
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
        if self.entries.len() > MAX_HISTORY_STATES {
            let overflow = self.entries.len() - MAX_HISTORY_STATES;
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
    fn change_description_detects_adjustment_type() {
        let a = state(1.0);
        let b = state(1.5);
        assert_eq!(describe_change(&a, &b), "Levels - Cyan");
    }
}
