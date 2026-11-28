//! How long the event loop spends on each thing that reaches it.
//!
//! Every loop event is timed: one that takes longer than [`SLOW_EVENT`]
//! is logged with its kind, so a user's log says what held the UI up.
//! The load harness turns [`LoopTimings::enable`] on to keep every
//! sample, and sends [`UiEvent::ShowHelp`] probes (a no-op for the loop)
//! to measure how long an event waits before the loop takes it.
//!
//! # Invariants
//!
//! * With sampling off (the app), recording costs one atomic load and,
//!   for a slow event, one log line.
//! * The samples are bounded ([`MAX_SAMPLES`]): a run that forgets to
//!   take them can't grow without end.

use aura_ui::UiEvent;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// A loop event longer than this is logged as a warning.
pub(crate) const SLOW_EVENT: Duration = Duration::from_millis(100);

/// A loop event longer than this is logged at debug level.
const NOTABLE_EVENT: Duration = Duration::from_millis(20);

/// Most samples kept while sampling is on.
const MAX_SAMPLES: usize = 1_000_000;

/// The event loop's timings.
#[derive(Default)]
pub(crate) struct LoopTimings {
    enabled: AtomicBool,
    kept: Mutex<Kept>,
}

/// What sampling keeps.
#[derive(Default)]
pub(crate) struct Kept {
    /// Each loop event's kind and how long the loop spent on it
    pub(crate) events: Vec<(&'static str, Duration)>,
    /// When each probe still on its way was sent
    probes: VecDeque<Instant>,
    /// How long each probe waited for the loop
    pub(crate) waits: Vec<Duration>,
}

impl LoopTimings {
    /// Keep every sample from now on (the load harness).
    #[cfg(test)]
    pub(crate) fn enable(&self) {
        self.enabled.store(true, Ordering::Relaxed);
    }

    /// Note that the loop spent `took` on one event of `kind`, logging it
    /// when it was slow.
    pub(crate) fn record(&self, kind: &'static str, took: Duration) {
        if took >= SLOW_EVENT {
            let ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
            warn!(kind, ms, "The event loop was held up by one event");
        } else if took >= NOTABLE_EVENT {
            let ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
            debug!(kind, ms, "A slow event loop event");
        }
        if self.enabled.load(Ordering::Relaxed) {
            let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            if kept.events.len() < MAX_SAMPLES {
                kept.events.push((kind, took));
            }
        }
    }

    /// A probe is sent to the loop now ([`UiEvent::ShowHelp`]).
    #[cfg(test)]
    pub(crate) fn probe_sent(&self) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.probes.push_back(Instant::now());
    }

    /// The loop takes `event`: when it is a probe, note how long it
    /// waited.
    pub(crate) fn arrived(&self, event: &UiEvent) {
        if !matches!(event, UiEvent::ShowHelp) || !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sent) = kept.probes.pop_front() {
            if kept.waits.len() < MAX_SAMPLES {
                kept.waits.push(sent.elapsed());
            }
        }
    }

    /// Take what was kept so far.
    #[cfg(test)]
    pub(crate) fn take(&self) -> Kept {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let probes = std::mem::take(&mut kept.probes);
        Kept {
            events: std::mem::take(&mut kept.events),
            waits: std::mem::take(&mut kept.waits),
            probes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_kept_only_once_enabled_and_probes_time_their_wait() {
        let timings = LoopTimings::default();
        timings.record("work done", Duration::from_millis(3));
        timings.probe_sent();
        timings.arrived(&UiEvent::ShowHelp);
        let kept = timings.take();
        assert!(
            kept.events.is_empty() && kept.waits.is_empty(),
            "off by default"
        );

        timings.enable();
        timings.record("work done", Duration::from_millis(3));
        timings.record("inbox save", SLOW_EVENT);
        timings.probe_sent();
        timings.arrived(&UiEvent::Quit);
        timings.arrived(&UiEvent::ShowHelp);
        let kept = timings.take();
        assert_eq!(
            kept.events
                .iter()
                .map(|(kind, _)| *kind)
                .collect::<Vec<_>>(),
            ["work done", "inbox save"]
        );
        assert_eq!(kept.waits.len(), 1, "only the probe counts");
    }
}
