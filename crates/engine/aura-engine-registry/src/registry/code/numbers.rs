//! [`LastNumbers`]: the highest number each code prefix has been given.

use super::{is_prefix, parse, RegisteredAgent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The highest number each code prefix has ever been given, persisted
/// with the registry so a number freed by dropping its agent is never
/// handed out again.
///
/// Invariants: every entry's prefix is three uppercase letters, an entry
/// is never lowered or removed (except by [`migrate`](super::migrate), which drops the
/// entries of prefixes older registries had, as no code can have them
/// any more), and, once the registry is loaded, no code in it has a
/// number above its prefix's entry (see [`LastNumbers::raise_to_codes`]
/// and [`assign`](super::assign)).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(in crate::registry) struct LastNumbers(BTreeMap<String, u64>);

impl LastNumbers {
    /// The numbers as stored: each prefix's highest number.
    pub(in crate::registry) fn to_map(&self) -> BTreeMap<String, u64> {
        self.0.clone()
    }

    /// The numbers stored as `map`.
    pub(in crate::registry) fn from_map(map: BTreeMap<String, u64>) -> Self {
        Self(map)
    }

    /// Whether no prefix has been given a number yet.
    pub(in crate::registry) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Drop the entries whose prefix is not three uppercase letters, such
    /// as the `RE` and `AG` of registries written when prefixes could be
    /// two letters. Returns whether any entry was dropped.
    pub(super) fn retain_valid(&mut self) -> bool {
        let before = self.0.len();
        self.0.retain(|prefix, _| is_prefix(prefix));
        self.0.len() != before
    }

    /// The highest number `prefix` has been given, `0` when none.
    pub(in crate::registry) fn last(&self, prefix: &str) -> u64 {
        self.0.get(prefix).copied().unwrap_or(0)
    }

    /// Raise `prefix`'s entry to `number` when it is lower. Returns
    /// whether it changed.
    pub(super) fn raise(&mut self, prefix: &str, number: u64) -> bool {
        if number <= self.last(prefix) {
            return false;
        }
        self.0.insert(prefix.to_string(), number);
        true
    }

    /// Raise every prefix's entry to the highest number its codes in
    /// `agents` have, as a registry written before the entries existed
    /// needs. Codes that are not `<PREFIX><N>` are skipped. Returns
    /// whether any entry changed.
    pub(in crate::registry) fn raise_to_codes(&mut self, agents: &[RegisteredAgent]) -> bool {
        let mut changed = false;
        for (prefix, number) in agents.iter().filter_map(|a| parse(a.code.as_deref()?)) {
            changed |= self.raise(prefix, number);
        }
        changed
    }
}
