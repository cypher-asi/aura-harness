//! "Not delivered" notices, at most one per sender, recipient and
//! subject until a message between them gets through or
//! [`REFUSAL_QUIET`] passes. A machine resends its mail after a
//! reconnect, and each refusal would otherwise be a notice, and so a
//! turn, of its sender.

use aura_core_types::AgentId;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a refusal keeps the same one from being told again.
pub(crate) const REFUSAL_QUIET: Duration = Duration::from_secs(10 * 60);

/// Who was told a message was refused, and when.
type Key = (AgentId, AgentId, String);

/// The refusals told lately.
#[derive(Default)]
pub(crate) struct Refusals {
    told: Mutex<HashMap<Key, Instant>>,
}

impl Refusals {
    /// Whether `from` is to hear that its message to `to` about `subject`
    /// was refused, at `now`: not when it heard so within
    /// [`REFUSAL_QUIET`]. Counts as told when it is.
    pub(crate) fn tell(&self, (from, to): (AgentId, AgentId), subject: &str, now: Instant) -> bool {
        let mut told = self.lock();
        told.retain(|_, at| now.saturating_duration_since(*at) < REFUSAL_QUIET);
        let key = (from, to, normalized(subject));
        if told.contains_key(&key) {
            return false;
        }
        told.insert(key, now);
        true
    }

    /// A message from `from` to `to` about `subject` got through: a later
    /// refusal is news again.
    pub(crate) fn delivered(&self, (from, to): (AgentId, AgentId), subject: &str) {
        self.lock().remove(&(from, to, normalized(subject)));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Instant>> {
        self.told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// `subject` as refusals are told apart by.
fn normalized(subject: &str) -> String {
    subject.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_told_once_until_mail_gets_through_or_it_goes_quiet() {
        let refusals = Refusals::default();
        let (a, b, c) = (
            AgentId::new([1; 32]),
            AgentId::new([2; 32]),
            AgentId::new([3; 32]),
        );
        let now = Instant::now();
        assert!(refusals.tell((a, b), "Report", now));
        assert!(
            !refusals.tell((a, b), " report ", now),
            "the resend after a reconnect"
        );
        assert!(refusals.tell((a, b), "Other", now), "another subject");
        assert!(refusals.tell((a, c), "Report", now), "another recipient");
        assert!(refusals.tell((c, b), "Report", now), "another sender");
        refusals.delivered((a, b), "Report");
        assert!(
            refusals.tell((a, b), "Report", now),
            "news again once one got through"
        );
        let later = now + REFUSAL_QUIET;
        assert!(
            refusals.tell((a, b), "Report", later),
            "and once it went quiet"
        );
    }
}
