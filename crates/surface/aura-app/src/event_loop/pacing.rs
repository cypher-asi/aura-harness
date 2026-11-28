//! Work the event loop does at most once per interval, however often it
//! is asked: the agent lists, and the loop's own saves of the inbox.
//!
//! The first ask after a quiet interval runs at once, as before. An ask
//! within the interval only marks the work due; the loop runs it once
//! the interval is over ([`Pacing::due`] wakes it), with the state as it
//! is then. So a burst of asks costs one run per interval, and the last
//! ask is never lost.
//!
//! # Invariants
//!
//! * An interval of zero runs every ask at once (most tests).
//! * Asks may come from any thread; the run is the loop's.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// How often the agent lists are rebuilt at most: each build reads
/// every agent's record head and the UI rebuilds its trees, so with
/// hundreds of agents and their turns a build per turn kept the loop busy.
pub(crate) const LIST_EVERY: Duration = Duration::from_millis(250);

/// How often the loop saves the inbox at most. Saves something waits on
/// (mail filed, shutdown) are written at once regardless.
pub(crate) const SAVE_EVERY: Duration = Duration::from_millis(200);

/// What the hub keeps for the event loop's own work: its timings, and
/// the pacing of the agent lists and of its inbox saves.
#[derive(Default)]
pub(crate) struct LoopWork {
    pub(crate) timings: super::timing::LoopTimings,
    pub(crate) lists: Pacing,
    pub(crate) saves: Pacing,
}

impl LoopWork {
    /// Paced as the app paces them; in tests, which expect each list and
    /// save at once, not paced.
    pub(crate) fn paced() -> Self {
        let paced = |every| if cfg!(test) { Duration::ZERO } else { every };
        Self {
            timings: super::timing::LoopTimings::default(),
            lists: Pacing::every(paced(LIST_EVERY)),
            saves: Pacing::every(paced(SAVE_EVERY)),
        }
    }
}

/// How often one kind of work runs at most.
#[derive(Debug, Default)]
pub(crate) struct Pacing {
    every: Mutex<Duration>,
    state: Mutex<Paced>,
    wake: Notify,
}

#[derive(Debug, Default)]
struct Paced {
    /// Every ask runs at once: the user is waiting on it
    urgent: bool,
    /// When the work last ran
    last: Option<Instant>,
    /// An ask came since
    pending: bool,
    /// Times the work ran, for the tests
    runs: u64,
}

impl Pacing {
    /// Pacing at most once per `every`.
    pub(crate) fn every(every: Duration) -> Self {
        Self {
            every: Mutex::new(every),
            ..Self::default()
        }
    }

    /// Run at most once per `every` from now on.
    #[cfg(test)]
    pub(crate) fn set_every(&self, every: Duration) {
        *self.every.lock().unwrap_or_else(PoisonError::into_inner) = every;
    }

    /// While `urgent`, every ask runs at once: the loop handles what the
    /// user just did, and the user waits to see it.
    pub(crate) fn set_urgent(&self, urgent: bool) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .urgent = urgent;
    }

    /// Whether asks within an interval wait.
    pub(crate) fn is_paced(&self) -> bool {
        !self.interval().is_zero()
    }

    fn interval(&self) -> Duration {
        *self.every.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ask for the work. Returns whether to run it now; when not, it is
    /// due once the interval is over.
    pub(crate) fn ask(&self) -> bool {
        let every = self.interval();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if every.is_zero() || state.urgent || state.last.is_none_or(|last| now >= last + every) {
            state.last = Some(now);
            state.pending = false;
            state.runs += 1;
            return true;
        }
        if !state.pending {
            state.pending = true;
            self.wake.notify_one();
        }
        false
    }

    /// Resolves once work asked for is due. The loop then calls
    /// [`Self::take_due`] and runs it.
    pub(crate) async fn due(&self) {
        loop {
            let deadline = {
                let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                state.pending.then(|| {
                    state
                        .last
                        .map_or_else(Instant::now, |last| last + self.interval())
                })
            };
            match deadline {
                Some(deadline) => {
                    tokio::time::sleep_until(deadline.into()).await;
                    return;
                }
                None => self.wake.notified().await,
            }
        }
    }

    /// Whether work is due now; when it is, it counts as run.
    pub(crate) fn take_due(&self) -> bool {
        let every = self.interval();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let due = state.pending && state.last.is_none_or(|last| now >= last + every);
        if due {
            state.last = Some(now);
            state.pending = false;
            state.runs += 1;
        }
        due
    }

    /// Times the work ran.
    #[cfg(test)]
    pub(crate) fn runs(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_burst_of_asks_runs_once_now_and_once_when_the_interval_is_over() {
        let pacing = Pacing::every(Duration::from_millis(30));
        assert!(pacing.ask(), "the first ask runs at once");
        for _ in 0..100 {
            assert!(!pacing.ask(), "asks within the interval wait");
        }
        assert!(!pacing.take_due(), "not before the interval is over");
        tokio::time::timeout(Duration::from_secs(5), pacing.due())
            .await
            .expect("due once the interval is over");
        assert!(pacing.take_due());
        assert!(!pacing.take_due(), "once");
        assert_eq!(pacing.runs(), 2);
    }

    #[tokio::test]
    async fn without_an_interval_every_ask_runs_and_nothing_is_left_due() {
        let pacing = Pacing::default();
        for _ in 0..3 {
            assert!(pacing.ask());
        }
        assert!(!pacing.take_due());
        let waited = tokio::time::timeout(Duration::from_millis(30), pacing.due()).await;
        assert!(waited.is_err(), "nothing is due");
    }
}
