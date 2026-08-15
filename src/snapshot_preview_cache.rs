use std::collections::{BTreeMap, VecDeque};

pub const DEFAULT_MAX_ENTRIES: usize = 32;
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotPreviewKey {
    pub snapshot_id: u64,
    pub face_index: usize,
    pub solo_channel: Option<usize>,
}

impl SnapshotPreviewKey {
    pub fn new(snapshot_id: u64, face_index: usize, solo_channel: Option<usize>) -> Self {
        Self {
            snapshot_id,
            face_index,
            solo_channel,
        }
    }
}

struct CacheValue<V> {
    value: V,
    estimated_bytes: usize,
}

pub struct SnapshotPreviewCache<V> {
    entries: BTreeMap<SnapshotPreviewKey, CacheValue<V>>,
    lru: VecDeque<SnapshotPreviewKey>,
    estimated_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl<V> Default for SnapshotPreviewCache<V> {
    fn default() -> Self {
        Self::with_limits(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }
}

impl<V> SnapshotPreviewCache<V> {
    pub fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            lru: VecDeque::new(),
            estimated_bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1),
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
        self.estimated_bytes = 0;
    }

    pub fn remove_snapshot(&mut self, snapshot_id: u64) {
        let keys = self
            .entries
            .keys()
            .copied()
            .filter(|key| key.snapshot_id == snapshot_id)
            .collect::<Vec<_>>();
        for key in keys {
            self.remove_key(key);
        }
    }

    pub fn insert(&mut self, key: SnapshotPreviewKey, value: V, estimated_bytes: usize) {
        self.remove_key(key);
        let estimated_bytes = estimated_bytes.max(1);
        self.estimated_bytes = self.estimated_bytes.saturating_add(estimated_bytes);
        self.entries.insert(
            key,
            CacheValue {
                value,
                estimated_bytes,
            },
        );
        self.touch(key);
        self.evict_to_limits();
    }

    fn remove_key(&mut self, key: SnapshotPreviewKey) {
        if let Some(old) = self.entries.remove(&key) {
            self.estimated_bytes = self.estimated_bytes.saturating_sub(old.estimated_bytes);
        }
        if let Some(position) = self.lru.iter().position(|candidate| *candidate == key) {
            self.lru.remove(position);
        }
    }

    fn touch(&mut self, key: SnapshotPreviewKey) {
        if let Some(position) = self.lru.iter().position(|candidate| *candidate == key) {
            self.lru.remove(position);
        }
        self.lru.push_back(key);
    }

    fn evict_to_limits(&mut self) {
        while self.entries.len() > 1
            && (self.entries.len() > self.max_entries || self.estimated_bytes > self.max_bytes)
        {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            if let Some(old) = self.entries.remove(&oldest) {
                self.estimated_bytes = self.estimated_bytes.saturating_sub(old.estimated_bytes);
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }
}

impl<V: Clone> SnapshotPreviewCache<V> {
    pub fn get_cloned(&mut self, key: SnapshotPreviewKey) -> Option<V> {
        let value = self.entries.get(&key)?.value.clone();
        self.touch(key);
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(snapshot_id: u64) -> SnapshotPreviewKey {
        SnapshotPreviewKey::new(snapshot_id, 0, None)
    }

    #[test]
    fn revisiting_entry_makes_it_recent() {
        let mut cache = SnapshotPreviewCache::with_limits(2, 100);
        cache.insert(key(1), "one", 10);
        cache.insert(key(2), "two", 10);
        assert_eq!(cache.get_cloned(key(1)), Some("one"));
        cache.insert(key(3), "three", 10);
        assert_eq!(cache.get_cloned(key(1)), Some("one"));
        assert_eq!(cache.get_cloned(key(2)), None);
        assert_eq!(cache.get_cloned(key(3)), Some("three"));
    }

    #[test]
    fn byte_budget_evicts_oldest_but_keeps_latest_entry() {
        let mut cache = SnapshotPreviewCache::with_limits(8, 15);
        cache.insert(key(1), 1, 10);
        cache.insert(key(2), 2, 10);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get_cloned(key(2)), Some(2));
        assert_eq!(cache.estimated_bytes(), 10);

        cache.insert(key(3), 3, 30);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get_cloned(key(3)), Some(3));
        assert_eq!(cache.estimated_bytes(), 30);
    }

    #[test]
    fn remove_snapshot_removes_all_faces_and_display_modes() {
        let mut cache = SnapshotPreviewCache::with_limits(8, 1000);
        cache.insert(SnapshotPreviewKey::new(7, 0, None), 1, 10);
        cache.insert(SnapshotPreviewKey::new(7, 1, Some(2)), 2, 10);
        cache.insert(SnapshotPreviewKey::new(8, 0, None), 3, 10);
        cache.remove_snapshot(7);
        assert_eq!(cache.get_cloned(SnapshotPreviewKey::new(7, 0, None)), None);
        assert_eq!(
            cache.get_cloned(SnapshotPreviewKey::new(7, 1, Some(2))),
            None
        );
        assert_eq!(
            cache.get_cloned(SnapshotPreviewKey::new(8, 0, None)),
            Some(3)
        );
    }

    #[test]
    fn replacing_key_updates_accounting() {
        let mut cache = SnapshotPreviewCache::with_limits(8, 1000);
        cache.insert(key(1), 1, 100);
        cache.insert(key(1), 2, 25);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.estimated_bytes(), 25);
        assert_eq!(cache.get_cloned(key(1)), Some(2));
    }
}
