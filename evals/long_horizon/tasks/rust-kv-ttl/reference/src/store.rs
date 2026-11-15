use crate::clock::{Clock, SystemClock};
use std::collections::HashMap;

#[derive(Debug)]
struct Entry {
    value: String,
    expires_at_ms: Option<u64>,
}

/// String keys to string values, with optional expiry.
#[derive(Debug)]
pub struct Store<C: Clock = SystemClock> {
    data: HashMap<String, Entry>,
    clock: C,
}

impl Default for Store<SystemClock> {
    fn default() -> Self {
        Self::new()
    }
}

impl Store<SystemClock> {
    pub fn new() -> Self {
        Self::with_clock(SystemClock)
    }
}

impl<C: Clock> Store<C> {
    pub fn with_clock(clock: C) -> Self {
        Self {
            data: HashMap::new(),
            clock,
        }
    }

    fn live(&self, key: &str) -> Option<&Entry> {
        let now = self.clock.now_ms();
        self.data
            .get(key)
            .filter(|e| e.expires_at_ms.map_or(true, |at| now < at))
    }

    fn purge(&mut self, key: &str) {
        if self.data.contains_key(key) && self.live(key).is_none() {
            self.data.remove(key);
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.live(key).map(|e| e.value.as_str())
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.data.insert(
            key.into(),
            Entry {
                value: value.into(),
                expires_at_ms: None,
            },
        );
    }

    /// Remove `key`. Returns whether it existed.
    pub fn del(&mut self, key: &str) -> bool {
        self.purge(key);
        self.data.remove(key).is_some()
    }

    pub fn expire(&mut self, key: &str, seconds: u64) -> bool {
        self.purge(key);
        let at = self
            .clock
            .now_ms()
            .saturating_add(seconds.saturating_mul(1000));
        match self.data.get_mut(key) {
            Some(entry) => {
                entry.expires_at_ms = Some(at);
                true
            }
            None => false,
        }
    }

    pub fn ttl(&self, key: &str) -> i64 {
        match self.live(key) {
            None => -2,
            Some(Entry {
                expires_at_ms: None,
                ..
            }) => -1,
            Some(Entry {
                expires_at_ms: Some(at),
                ..
            }) => {
                let remaining = at - self.clock.now_ms();
                i64::try_from(remaining.div_ceil(1000)).unwrap_or(i64::MAX)
            }
        }
    }

    pub fn len(&self) -> usize {
        self.data.keys().filter(|k| self.live(k).is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
