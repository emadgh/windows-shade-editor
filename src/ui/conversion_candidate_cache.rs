use std::collections::VecDeque;

pub(crate) struct CandidateLru<T> {
    entries: VecDeque<CacheEntry<T>>,
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
}

struct CacheEntry<T> {
    key: String,
    bytes: usize,
    value: T,
}

impl<T> CandidateLru<T> {
    pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1),
            bytes: 0,
        }
    }

    pub(crate) fn insert(&mut self, key: String, bytes: usize, value: T) {
        self.remove(&key);
        if bytes > self.max_bytes {
            return;
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.push_front(CacheEntry { key, bytes, value });
        self.trim();
    }

    pub(crate) fn take(&mut self, key: &str) -> Option<T> {
        let index = self.entries.iter().position(|entry| entry.key == key)?;
        let entry = self.entries.remove(index)?;
        self.bytes = self.bytes.saturating_sub(entry.bytes);
        Some(entry.value)
    }

    pub(crate) fn remove(&mut self, key: &str) -> bool {
        let Some(index) = self.entries.iter().position(|entry| entry.key == key) else {
            return false;
        };
        if let Some(entry) = self.entries.remove(index) {
            self.bytes = self.bytes.saturating_sub(entry.bytes);
            true
        } else {
            false
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes
    }

    fn trim(&mut self) {
        while self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            let Some(entry) = self.entries.pop_back() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select(cache: &mut CandidateLru<&'static str>, key: &str, renders: &mut usize) -> &'static str {
        if let Some(value) = cache.take(key) {
            cache.insert(key.to_owned(), 8, value);
            return value;
        }
        *renders += 1;
        let rendered = match key {
            "A" => "rendered-A",
            "B" => "rendered-B",
            _ => "rendered-other",
        };
        cache.insert(key.to_owned(), 8, rendered);
        rendered
    }

    #[test]
    fn a_b_a_reuses_completed_candidate_without_third_render() {
        let mut cache = CandidateLru::new(4, 1024);
        let mut renders = 0;
        assert_eq!(select(&mut cache, "A", &mut renders), "rendered-A");
        assert_eq!(select(&mut cache, "B", &mut renders), "rendered-B");
        assert_eq!(select(&mut cache, "A", &mut renders), "rendered-A");
        assert_eq!(renders, 2);
    }

    #[test]
    fn cache_is_bounded_by_entry_count_and_estimated_bytes() {
        let mut cache = CandidateLru::new(2, 12);
        cache.insert("A".to_owned(), 6, 1);
        cache.insert("B".to_owned(), 6, 2);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.bytes(), 12);
        cache.insert("C".to_owned(), 6, 3);
        assert_eq!(cache.len(), 2);
        assert!(cache.take("A").is_none());
        assert_eq!(cache.take("C"), Some(3));
    }

    #[test]
    fn oversized_candidate_is_not_cached() {
        let mut cache = CandidateLru::new(4, 10);
        cache.insert("huge".to_owned(), 11, 9);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.bytes(), 0);
    }
}
