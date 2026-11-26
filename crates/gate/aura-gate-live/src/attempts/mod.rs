//! Writing Gate's attempt log ([`aura_gate_attempts`]) while the app
//! runs.
//!
//! - an `attempt` line is written where a turn's `Outcome` is recorded
//!   with the attempt's messages in hand ([`GateLive::log_attempt`]): the
//!   turns of agents Gate routes, and both arms of `/compare`
//! - a `truth` line is written when a benchmark task's own check has run
//!   on a run's work ([`GateLive::log_truth`]), and when the user rewinds
//!   a turn, sends it again to a stronger model or picks between runs
//!   (from the record as it is committed)
//! - a `review` line is written when Gate's judge has gone through work
//!   that was about to stand ([`GateLive::log_review`])
//! - at startup the comparisons already recorded are caught up on
//!   ([`GateLive::catch_up_in_background`]): their runs' attempts are
//!   rebuilt from the record and the saved sessions, so the log covers
//!   work done before it existed
//!
//! The log is read back for one thing while the app runs: what each sign
//! of failure has earned on the attempts it was raised on
//! ([`GateLive::learn_tiers`]), which decides the signs that have work
//! reviewed. A line that cannot be written is logged and dropped.

mod catch_up;
#[cfg(test)]
mod tests;

pub use catch_up::{sent_text, SavedSessions};

use crate::table::{lock, GateLive};
use aura_agent::AgentLoopResult;
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, CompareCopy, Hash, OutcomePayload, RecordEntry};
use aura_gate_attempts::{Attempt, AttemptLog, CopyAt, Line, Review, Terms, Truth, TruthSource};
use aura_gate_evidence::Evidence;
use tracing::warn;

/// The evidence of the attempt that ended in `result`, whose messages
/// start at `turn_start`.
pub fn evidence(result: &AgentLoopResult, turn_start: usize) -> Evidence {
    let mut evidence = Evidence::of(result.messages.get(turn_start..).unwrap_or_default());
    // The harness counts the files of the attempt's last run; one that
    // was asked to check its change edited the rest before.
    let counted = u32::try_from(result.file_changes.len()).unwrap_or(u32::MAX);
    let edited = u32::try_from(evidence.files_edited.len()).unwrap_or(u32::MAX);
    evidence.files_changed = Some(counted.max(edited));
    evidence.hit_limit = Some(result.budget_exhausted || result.stalled || result.output_truncated);
    evidence
}

/// [`evidence`], with the files the attempt changed by any means when
/// the harness compared the workspace before and after (`changed`).
pub fn evidence_of(
    result: &AgentLoopResult,
    turn_start: usize,
    changed: Option<&[String]>,
) -> Evidence {
    let mut evidence = evidence(result, turn_start);
    if let Some(changed) = changed {
        evidence.note_changed(changed);
    }
    evidence
}

/// Where the kept copy `copy` of a run is, for the log.
pub fn copy_at(copy: &CompareCopy) -> CopyAt {
    CopyAt {
        root: copy.root.clone(),
        git_dir: copy.git_dir.clone(),
        baseline: copy.baseline.clone(),
    }
}

/// Where a truth's attempt ran, for a run of `/compare`.
#[derive(Debug, Clone, Copy)]
pub struct RunOf<'a> {
    /// The comparison, e.g. `Compare #22`
    pub compare: &'a str,
    /// The run, e.g. `a-4`
    pub run: &'a str,
    /// The benchmark task it did
    pub task: Option<&'a str>,
    /// The copy of the workspace it worked in
    pub copy: Option<&'a CompareCopy>,
}

impl GateLive {
    /// Log attempts to `log` from here on. The first call decides.
    pub fn log_attempts_to(&self, log: AttemptLog) {
        let _ = self.log.set(std::sync::Arc::new(log));
        self.learn_tiers();
    }

    /// Read the attempt log and take from it what each sign of failure
    /// has earned ([`aura_gate_attempts::tiers`]). A log that cannot be
    /// read leaves the tiers as they are.
    pub fn learn_tiers(&self) {
        let Some(log) = self.log.get() else {
            return;
        };
        match log.read() {
            Ok(read) => *lock(&self.tiers) = aura_gate_attempts::tiers(&read.lines),
            Err(e) => {
                warn!(path = %log.path().display(), error = %e, "Could not read Gate's attempt log");
            }
        }
    }

    /// Log what Gate's judge made of work that was about to stand.
    pub fn log_review(&self, review: Review) {
        self.append(&Line::Review(review));
    }

    /// The attempt log, when attempts are logged.
    pub fn attempt_log(&self) -> Option<&AttemptLog> {
        self.log.get().map(|log| &**log)
    }

    fn append(&self, line: &Line) {
        let Some(log) = self.log.get() else {
            return;
        };
        if let Err(e) = log.append(line) {
            warn!(path = %log.path().display(), error = %e, "Could not write Gate's attempt log");
        }
    }

    /// Log the attempt of `agent` that ended as `outcome` says, leaving
    /// `evidence`; an attempt that ended in an error leaves none.
    pub fn log_attempt(
        &self,
        agent: AgentId,
        outcome: &OutcomePayload,
        terms: Terms,
        evidence: Option<Evidence>,
    ) {
        if self.log.get().is_none() {
            return;
        }
        self.append(&Line::Attempt(Attempt::new(
            now_ms(),
            agent.to_hex(),
            outcome,
            terms,
            evidence.unwrap_or_default(),
        )));
    }

    /// Log that the check of the task `of` ran did on the work of the
    /// attempt of `agent` answering `prompt_tx`, and whether it passed.
    pub fn log_truth(&self, agent: AgentId, prompt_tx: Hash, passed: bool, of: RunOf<'_>) {
        self.append(&Line::Truth(Truth {
            at_ms: now_ms(),
            agent: agent.to_hex(),
            prompt_tx: prompt_tx.to_hex(),
            source: TruthSource::TaskCheck,
            passed,
            compare: Some(of.compare.to_string()),
            run: Some(of.run.to_string()),
            task: of.task.map(str::to_string),
            copy: of.copy.map(copy_at),
        }));
    }

    /// Log the user's verdict on an attempt, when `entry` is one: a
    /// rewind, a retry on a stronger model, or a pick between runs.
    pub(crate) fn log_user_signal(&self, entry: &RecordEntry) {
        if self.log.get().is_none() {
            return;
        }
        let Some(outcome) = entry.tx.outcome_payload() else {
            return;
        };
        let (Some(source), Some(passed)) = (TruthSource::of_user(outcome.signal), outcome.success)
        else {
            return;
        };
        self.append(&Line::Truth(Truth {
            at_ms: now_ms(),
            agent: entry.tx.agent_id.to_hex(),
            prompt_tx: outcome.prompt_tx.to_hex(),
            source,
            passed,
            compare: None,
            run: None,
            task: None,
            copy: None,
        }));
    }
}
