//! A bounded least-recently-used store.

use rustc_hash::FxHashMap;

/// A least-recently-used store. Entries are found by a hash of their key
/// and checked against it, so a lookup allocates nothing.
pub(super) struct Lru<K, V> {
    entries: FxHashMap<u64, (K, V, u64)>,
    clock: u64,
    limit: usize,
}

impl<K, V> Lru<K, V> {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            entries: FxHashMap::default(),
            clock: 0,
            limit: limit.max(2),
        }
    }

    pub(super) fn get(&mut self, hash: u64, is: impl FnOnce(&K) -> bool) -> Option<&V> {
        self.clock += 1;
        let (key, value, used) = self.entries.get_mut(&hash)?;
        if !is(key) {
            return None;
        }
        *used = self.clock;
        Some(value)
    }

    /// Store `value`, dropping the half used longest ago when full.
    pub(super) fn put(&mut self, hash: u64, key: K, value: V) {
        if self.entries.len() >= self.limit {
            let mut used: Vec<u64> = self.entries.values().map(|entry| entry.2).collect();
            used.sort_unstable();
            let cutoff = used[used.len() / 2];
            self.entries.retain(|_, entry| entry.2 > cutoff);
        }
        self.clock += 1;
        self.entries.insert(hash, (key, value, self.clock));
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}
