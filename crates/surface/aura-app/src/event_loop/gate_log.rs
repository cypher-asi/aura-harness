//! Writing Gate's attempt log ([`aura_agent_gate::attempt_log`]) while
//! the app runs.
//!
//! - an `attempt` line is written where a turn's `Outcome` is recorded
//!   with the attempt's messages in hand ([`GateLive::log_attempt`]): the
//!   turns of agents Gate routes, and both arms of `/compare`
//! - a `truth` line is written when a benchmark task's own check has run
//!   on a run's work ([`GateLive::log_truth`]), and when the user rewinds
//!   a turn, sends it again to a stronger model or picks between runs
//!   ([`GateLive::log_user_signal`], from the record as it is committed)
//! - a `review` line is written when Gate's judge has gone through work
//!   that was about to stand ([`GateLive::log_review`])
//! - at startup the comparisons already recorded are caught up on
//!   ([`catch_up`]): their runs' attempts are rebuilt from the record and
//!   the saved sessions, so the log covers work done before it existed
//!
//! The log is read back for one thing while the app runs: what each sign
//! of failure has earned on the attempts it was raised on
//! ([`GateLive::learn_tiers`]), which decides the signs that have work
//! reviewed. A line that cannot be written is logged and dropped.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aura_agent::AgentLoopResult;
use aura_agent_gate::attempt_log::{
    self, Attempt, AttemptLog, CopyAt, Kind, Line, Review, Terms, Truth, TruthSource,
};
use aura_agent_gate::evidence::Evidence;
use aura_agent_gate::followup::{NUDGE_PROMPT, REPAIR_OPENING};
use aura_core_types::{
    AgentId, ArmSpec, CompareCopy, ComparePayload, CompareRun, Hash, OutcomePayload, RecordEntry,
    RouteLevel,
};
use aura_model::{ContentBlock, Message, Role};
use aura_store_db::ReadStore;
use tracing::{info, warn};

use super::gate_live::GateLive;
use super::processes::now_ms;
use crate::session_checkpoint::Checkpoints;

/// Record entries read per scan of a run's record.
const SCAN_BATCH: usize = 500;

/// Entries looked back through for a comparison's latest state.
const COMPARE_SCAN: usize = 200;

/// The evidence of the attempt that ended in `result`, whose messages
/// start at `turn_start`.
pub(crate) fn evidence(result: &AgentLoopResult, turn_start: usize) -> Evidence {
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
pub(crate) fn evidence_of(
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
pub(crate) fn copy_at(copy: &CompareCopy) -> CopyAt {
    CopyAt {
        root: copy.root.clone(),
        git_dir: copy.git_dir.clone(),
        baseline: copy.baseline.clone(),
    }
}

/// Where a truth's attempt ran, for a run of `/compare`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunOf<'a> {
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
        let _ = self.log.set(log);
        self.learn_tiers();
    }

    /// Read the attempt log and take from it what each sign of failure
    /// has earned ([`attempt_log::tiers`]). A log that cannot be read
    /// leaves the tiers as they are.
    pub(crate) fn learn_tiers(&self) {
        let Some(log) = self.log.get() else {
            return;
        };
        match log.read() {
            Ok(read) => {
                let tiers = attempt_log::tiers(&read.lines);
                *self
                    .tiers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = tiers;
            }
            Err(e) => {
                warn!(path = %log.path().display(), error = %e, "Could not read Gate's attempt log");
            }
        }
    }

    /// Log what Gate's judge made of work that was about to stand.
    pub(crate) fn log_review(&self, review: Review) {
        self.append(&Line::Review(review));
    }

    /// The attempt log, when attempts are logged.
    pub(crate) fn attempt_log(&self) -> Option<&AttemptLog> {
        self.log.get()
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
    pub(crate) fn log_attempt(
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
    pub(crate) fn log_truth(&self, agent: AgentId, prompt_tx: Hash, passed: bool, of: RunOf<'_>) {
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
    pub(super) fn log_user_signal(&self, entry: &RecordEntry) {
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

/// What [`catch_up`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CaughtUp {
    /// Runs with a check result whose lines were missing
    pub runs: usize,
    /// Attempt lines written
    pub attempts: usize,
    /// Truth lines written
    pub truths: usize,
    /// Runs left out: no saved session, or one that does not line up
    /// with the record
    pub skipped: usize,
    /// Check results written again with the place of the run's copy
    pub placed: usize,
}

/// Write to `log` the attempts and check results of the comparisons held
/// by `hosts` (each with its name) that it does not have yet, rebuilt
/// from `store` and the sessions saved in `sessions`. Only runs whose
/// check ran are taken: the others have nothing to score an attempt
/// against. A run whose check result is logged without the place of its
/// copy gets the result again with it, so what the run changed can be
/// read back while the copy is kept.
pub(crate) fn catch_up(
    store: &dyn ReadStore,
    hosts: &[(AgentId, String)],
    sessions: &Checkpoints,
    log: &AttemptLog,
) -> CaughtUp {
    let mut caught = CaughtUp::default();
    let read = match log.read() {
        Ok(read) => read,
        Err(e) => {
            warn!(path = %log.path().display(), error = %e, "Could not read Gate's attempt log");
            return caught;
        }
    };
    let have: HashSet<(String, String, Kind)> = read
        .lines
        .iter()
        .map(|line| {
            let (agent, prompt, kind) = line.key();
            (agent.to_string(), prompt.to_string(), kind)
        })
        .collect();
    // The last check result logged for each run.
    let judged: HashMap<&str, &Truth> = read
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::Truth(truth) if truth.source == TruthSource::TaskCheck => {
                Some((truth.agent.as_str(), truth))
            }
            _ => None,
        })
        .collect();
    for (host, name) in hosts {
        let Some(payload) = latest_compare(store, *host) else {
            continue;
        };
        for run in &payload.runs {
            let agent = run.agent_id.to_hex();
            if run.check_passed.is_none() {
                continue;
            }
            if let Some(logged) = judged.get(agent.as_str()) {
                if logged.copy.is_none() {
                    let placed = Truth {
                        copy: Some(copy_at(&run.copy)),
                        ..(*logged).clone()
                    };
                    if let Err(e) = log.append(&Line::Truth(placed)) {
                        warn!(path = %log.path().display(), error = %e, "Could not write Gate's attempt log");
                        return caught;
                    }
                    caught.placed += 1;
                }
                continue;
            }
            caught.runs += 1;
            let lines = sessions
                .load_current(run.agent_id)
                .and_then(|saved| rebuilt(store, &payload, name, run, &saved.messages));
            let Some(lines) = lines else {
                caught.skipped += 1;
                continue;
            };
            for line in lines {
                let (agent, prompt, kind) = line.key();
                if have.contains(&(agent.to_string(), prompt.to_string(), kind)) {
                    continue;
                }
                if let Err(e) = log.append(&line) {
                    warn!(path = %log.path().display(), error = %e, "Could not write Gate's attempt log");
                    return caught;
                }
                if kind == Kind::Attempt {
                    caught.attempts += 1;
                } else {
                    caught.truths += 1;
                }
            }
        }
    }
    caught
}

impl GateLive {
    /// [`catch_up`] on this table's attempt log, on a blocking thread so
    /// startup doesn't wait for it. Without a Tokio runtime it runs here.
    pub(crate) fn catch_up_in_background(
        self: &Arc<Self>,
        store: Arc<dyn ReadStore>,
        hosts: Vec<(AgentId, String)>,
        sessions: Checkpoints,
    ) {
        let live = Arc::clone(self);
        let work = move || {
            let Some(log) = live.attempt_log() else {
                return;
            };
            let caught = catch_up(store.as_ref(), &hosts, &sessions, log);
            if caught.attempts + caught.truths + caught.placed > 0 {
                info!(
                    runs = caught.runs,
                    attempts = caught.attempts,
                    truths = caught.truths,
                    skipped = caught.skipped,
                    placed = caught.placed,
                    "Gate's attempt log caught up on recorded comparisons"
                );
                live.learn_tiers();
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(work);
            }
            Err(_) => work(),
        }
    }
}

fn latest_compare(store: &dyn ReadStore, host: AgentId) -> Option<ComparePayload> {
    let head = store.get_head_seq(host).ok()?;
    store
        .scan_record_descending(host, head, COMPARE_SCAN)
        .ok()?
        .iter()
        .find_map(|entry| entry.tx.compare_payload())
}

fn record_of(store: &dyn ReadStore, agent: AgentId) -> Option<Vec<RecordEntry>> {
    let mut all = Vec::new();
    let mut from = 1;
    loop {
        let entries = store.scan_record(agent, from, SCAN_BATCH).ok()?;
        let last = entries.last().map(|e| e.seq)?;
        let full = entries.len() == SCAN_BATCH;
        all.extend(entries);
        if !full {
            return Some(all);
        }
        from = last + 1;
    }
}

/// The lines of `run`, a run of the comparison `payload` named `name`
/// whose saved session holds `messages`: its attempts, then its check's
/// result. `None` when its record cannot be read or does not line up
/// with the session.
fn rebuilt(
    store: &dyn ReadStore,
    payload: &ComparePayload,
    name: &str,
    run: &CompareRun,
    messages: &[Message],
) -> Option<Vec<Line>> {
    let entries = record_of(store, run.agent_id)?;
    let spec = match run.arm {
        aura_core_types::CompareArm::A => &payload.arm_a,
        aura_core_types::CompareArm::B => &payload.arm_b,
    };
    let routed = matches!(spec, ArmSpec::Gate { .. });
    let attempts = attempts_of(&entries, messages, routed)?;
    let last = attempts.last()?;
    let mut lines = Vec::with_capacity(attempts.len() + 1);
    // As when the check ran: a run its provider cut short did not finish
    // the work the check looks at, and the check left no outcome of it.
    let outcomes = entries
        .iter()
        .filter_map(|entry| entry.tx.outcome_payload())
        .filter(|outcome| outcome.prompt_tx.to_hex() == last.prompt_tx)
        .count();
    let truth = run
        .check_passed
        .filter(|_| outcomes > 1)
        .map(|passed| Truth {
            at_ms: last.at_ms,
            agent: last.agent.clone(),
            prompt_tx: last.prompt_tx.clone(),
            source: TruthSource::TaskCheck,
            passed,
            compare: Some(name.to_string()),
            run: Some(run.name.clone()),
            task: run.task.clone(),
            copy: Some(copy_at(&run.copy)),
        });
    lines.extend(attempts.into_iter().map(Line::Attempt));
    lines.extend(truth.map(Line::Truth));
    Some(lines)
}

/// The text of a message the user (or the harness in the user's place)
/// sent; `None` for tool results and the model's messages.
pub(crate) fn sent_text(message: &Message) -> Option<String> {
    let sent = message.role == Role::User
        && !message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
    sent.then(|| message.text_content())
}

/// The attempts in a record's `entries`, each with the evidence of its
/// part of the session's `messages`. An attempt starts at a prompt that
/// has an `Outcome` (the user's, or the one recorded when Gate moved the
/// turn on); the first `Outcome` recorded for it is what Gate made of it.
/// `None` when a prompt is not among the messages in the record's order,
/// as after the session was compacted.
fn attempts_of(
    entries: &[RecordEntry],
    messages: &[Message],
    routed: bool,
) -> Option<Vec<Attempt>> {
    struct Started {
        prompt_tx: Hash,
        at: usize,
        terms: Terms,
        ended: Option<(u64, OutcomePayload)>,
    }
    let mut started: Vec<Started> = Vec::new();
    let mut cursor = 0;
    for entry in entries {
        if let Some(prompt) = entry.tx.prompt_payload() {
            if prompt.route.level != RouteLevel::Turn {
                continue;
            }
            let wanted = prompt.text.trim();
            let at = cursor
                + messages.get(cursor..)?.iter().position(|message| {
                    sent_text(message).is_some_and(|text| text.trim() == wanted)
                })?;
            cursor = at + 1;
            // Gate's own messages to the model carry on the attempt they
            // are sent in.
            if prompt.text.contains(NUDGE_PROMPT) || prompt.text.contains(REPAIR_OPENING) {
                continue;
            }
            // A prompt recorded for a move to a stronger model carries no
            // label of its own.
            let first = started.first().map(|first| first.terms);
            started.push(Started {
                prompt_tx: entry.tx.hash,
                at,
                terms: Terms {
                    difficulty: prompt.difficulty.or(first.and_then(|t| t.difficulty)),
                    led: prompt.led.or(first.and_then(|t| t.led)),
                    routed,
                },
                ended: None,
            });
        } else if let Some(outcome) = entry.tx.outcome_payload() {
            if let Some(attempt) = started
                .iter_mut()
                .find(|attempt| attempt.prompt_tx == outcome.prompt_tx)
            {
                attempt.ended.get_or_insert((entry.tx.ts_ms, outcome));
            }
        }
    }
    let agent = entries.first()?.tx.agent_id.to_hex();
    let ends: Vec<usize> = started
        .iter()
        .skip(1)
        .map(|next| next.at)
        .chain([messages.len()])
        .collect();
    let attempts = started
        .into_iter()
        .zip(ends)
        .filter_map(|(attempt, end)| {
            let (at_ms, outcome) = attempt.ended?;
            let evidence = Evidence::of(&messages[attempt.at..end]);
            let mut line = Attempt::new(at_ms, agent.clone(), &outcome, attempt.terms, evidence);
            line.backfilled = true;
            Some(line)
        })
        .collect();
    Some(attempts)
}

#[cfg(test)]
mod tests;
