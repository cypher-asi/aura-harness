//! Ending a run: the scorecard and the log, written now or later off
//! the caller's thread.

use std::sync::atomic::Ordering;

use crate::bundle::now_ms;
use crate::scorecard::RunScorecard;
use crate::RunTrace;

use super::{lock, RunRecorder};

impl RunRecorder {
    /// Flush, stamp the end time, and write the scorecard and the log.
    pub fn finish(&self) -> Option<RunScorecard> {
        let _writing = lock(&self.writing);
        self.due_ms.store(0, Ordering::Release);
        self.write_up(now_ms())
    }

    /// Note that the run may end here, for [`Self::finish_due`] to do
    /// what [`Self::finish`] does off the caller's thread: the log is
    /// rebuilt from the whole run and the runs it spawned, which takes
    /// seconds for a long one. Returns whether that call is still to be
    /// made; not while an earlier one waits to start, as it covers this
    /// one too.
    #[must_use]
    pub fn finish_later(&self) -> bool {
        self.due_ms.swap(now_ms(), Ordering::AcqRel) == 0
    }

    /// What [`Self::finish`] does, stamped with the end time the latest
    /// [`Self::finish_later`] noted, when one was noted since the last
    /// write.
    pub fn finish_due(&self) {
        let _writing = lock(&self.writing);
        let ended_ms = self.due_ms.swap(0, Ordering::AcqRel);
        if ended_ms != 0 {
            self.write_up(ended_ms);
        }
    }

    fn write_up(&self, ended_ms: u64) -> Option<RunScorecard> {
        self.flush();
        self.update_meta(|m| m.ended_at_ms = Some(ended_ms));
        let trace = RunTrace::load(&self.bundle).ok();
        let card = trace.as_ref().map(RunTrace::scorecard);
        if let Some(trace) = &trace {
            let dir = self.bundle.dir();
            let home = crate::run_log::home_of(dir);
            let load =
                |run_id: &str| home.and_then(|home| crate::run_log::bundle_loader(home)(run_id));
            let log = crate::run_log::build(home, trace, dir, &load);
            if let Err(e) = crate::run_log::write(dir, &log) {
                tracing::debug!(error = %e, "trace log write failed");
            }
        }
        if let Some(card) = &card {
            if let Err(e) = self.bundle.write_summary(card) {
                tracing::debug!(error = %e, "trace summary write failed");
            }
            let sinks = lock(&self.sinks);
            let run_id = self.run_id();
            for sink in sinks.iter() {
                sink.summary(&run_id, card);
            }
        }
        card
    }
}
