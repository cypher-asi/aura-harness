//! Catching the attempt log up on comparisons recorded before it had
//! their runs: each run's attempts are rebuilt from its record and the
//! session saved for it.

use super::copy_at;
use crate::record::for_each_batch;
use crate::runtime::in_background;
use crate::table::GateLive;
use aura_core_types::{
    AgentId, ArmSpec, CompareArm, ComparePayload, CompareRun, Hash, OutcomePayload, RecordEntry,
    RouteLevel,
};
use aura_gate_attempts::{Attempt, AttemptLog, Kind, Line, Terms, Truth, TruthSource};
use aura_gate_evidence::transcript::said::is_nudge;
use aura_gate_evidence::Evidence;
use aura_gate_followup::REPAIR_OPENING;
use aura_model::{ContentBlock, Message, Role};
use aura_store_db::ReadStore;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{info, warn};

/// Entries looked back through for a comparison's latest state.
const COMPARE_SCAN: usize = 200;

/// The sessions saved for agents, which a run's messages are read from.
pub trait SavedSessions {
    /// The messages of the session saved for `agent`; `None` when none is.
    fn messages(&self, agent: AgentId) -> Option<Vec<Message>>;
}

/// What [`catch_up`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct CaughtUp {
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

impl GateLive {
    /// `catch_up` on this table's attempt log, reading the runs'
    /// messages from `sessions`, on a blocking thread so startup doesn't
    /// wait for it. Without a Tokio runtime it runs here.
    pub fn catch_up_in_background<S: SavedSessions + Send + 'static>(
        self: &Arc<Self>,
        store: Arc<dyn ReadStore>,
        hosts: Vec<(AgentId, String)>,
        sessions: S,
    ) {
        let live = Arc::clone(self);
        in_background(move || {
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
        });
    }
}

/// Write to `log` the attempts and check results of the comparisons held
/// by `hosts` (each with its name) that it does not have yet, rebuilt
/// from `store` and the sessions saved in `sessions`. Only runs whose
/// check ran are taken: the others have nothing to score an attempt
/// against. A run whose check result is logged without the place of its
/// copy gets the result again with it, so what the run changed can be
/// read back while the copy is kept.
pub(super) fn catch_up(
    store: &dyn ReadStore,
    hosts: &[(AgentId, String)],
    sessions: &dyn SavedSessions,
    log: &AttemptLog,
) -> CaughtUp {
    let read = match log.read() {
        Ok(read) => read,
        Err(e) => {
            warn!(path = %log.path().display(), error = %e, "Could not read Gate's attempt log");
            return CaughtUp::default();
        }
    };
    let mut catching = CatchingUp {
        store,
        sessions,
        log,
        logged: Logged::of(&read.lines),
        caught: CaughtUp::default(),
    };
    for (host, name) in hosts {
        let Some(payload) = latest_compare(store, *host) else {
            continue;
        };
        for run in &payload.runs {
            if catching.run(&payload, name, run).is_err() {
                return catching.caught;
            }
        }
    }
    catching.caught
}

/// What the log already holds: the key of every line, and the last check
/// result logged for each run.
struct Logged<'a> {
    keys: HashSet<(&'a str, &'a str, Kind)>,
    judged: HashMap<&'a str, &'a Truth>,
}

impl<'a> Logged<'a> {
    fn of(lines: &'a [Line]) -> Self {
        let judged = lines
            .iter()
            .filter_map(|line| match line {
                Line::Truth(truth) if truth.source == TruthSource::TaskCheck => {
                    Some((truth.agent.as_str(), truth))
                }
                _ => None,
            })
            .collect();
        Self {
            keys: lines.iter().map(Line::key).collect(),
            judged,
        }
    }
}

/// A line could not be written, which stops catching up.
struct Stopped;

/// [`catch_up`] as it goes.
struct CatchingUp<'a> {
    store: &'a dyn ReadStore,
    sessions: &'a dyn SavedSessions,
    log: &'a AttemptLog,
    logged: Logged<'a>,
    caught: CaughtUp,
}

impl CatchingUp<'_> {
    /// Write what the log is missing of `run`, a run of the comparison
    /// `payload` named `name`.
    fn run(
        &mut self,
        payload: &ComparePayload,
        name: &str,
        run: &CompareRun,
    ) -> Result<(), Stopped> {
        if run.check_passed.is_none() {
            return Ok(());
        }
        let agent = run.agent_id.to_hex();
        if let Some(&logged) = self.logged.judged.get(agent.as_str()) {
            if logged.copy.is_none() {
                let placed = Truth {
                    copy: Some(copy_at(&run.copy)),
                    ..logged.clone()
                };
                self.append(&Line::Truth(placed))?;
                self.caught.placed += 1;
            }
            return Ok(());
        }
        self.caught.runs += 1;
        let lines = self
            .sessions
            .messages(run.agent_id)
            .and_then(|messages| rebuilt(self.store, payload, name, run, &messages));
        let Some(lines) = lines else {
            self.caught.skipped += 1;
            return Ok(());
        };
        for line in lines {
            let key = line.key();
            if self.logged.keys.contains(&key) {
                continue;
            }
            self.append(&line)?;
            if key.2 == Kind::Attempt {
                self.caught.attempts += 1;
            } else {
                self.caught.truths += 1;
            }
        }
        Ok(())
    }

    fn append(&self, line: &Line) -> Result<(), Stopped> {
        self.log.append(line).map_err(|e| {
            warn!(path = %self.log.path().display(), error = %e, "Could not write Gate's attempt log");
            Stopped
        })
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
    for_each_batch(store, agent, |entries| all.extend(entries)).ok()?;
    Some(all)
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
        CompareArm::A => &payload.arm_a,
        CompareArm::B => &payload.arm_b,
    };
    let routed = matches!(spec, ArmSpec::Gate { .. });
    let attempts = attempts_of(&entries, messages, routed)?;
    let last = attempts.last()?;
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
    let mut lines = Vec::with_capacity(attempts.len() + 1);
    lines.extend(attempts.into_iter().map(Line::Attempt));
    lines.extend(truth.map(Line::Truth));
    Some(lines)
}

/// The text of a message the user (or the harness in the user's place)
/// sent; `None` for tool results and the model's messages.
pub fn sent_text(message: &Message) -> Option<String> {
    let sent = message.role == Role::User
        && !message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
    sent.then(|| message.text_content())
}

/// A prompt that starts an attempt: where its messages start, the terms
/// it was worked under, and the first `Outcome` recorded for it.
struct Started {
    at: usize,
    terms: Terms,
    ended: Option<(u64, OutcomePayload)>,
}

/// The attempts in a record's `entries`, each with the evidence of its
/// part of the session's `messages`. An attempt starts at a prompt that
/// has an `Outcome` (the user's, or the one recorded when Gate moved the
/// turn on); the first `Outcome` recorded for it is what Gate made of it.
/// `None` when a prompt is not among the messages in the record's order,
/// as after the session was compacted.
pub(super) fn attempts_of(
    entries: &[RecordEntry],
    messages: &[Message],
    routed: bool,
) -> Option<Vec<Attempt>> {
    let started = started_in(entries, messages, routed)?;
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

/// The prompts in `entries` that start attempts, in the record's order,
/// each with the first `Outcome` recorded for it. `None` when a prompt is
/// not among `messages` in that order.
fn started_in(entries: &[RecordEntry], messages: &[Message], routed: bool) -> Option<Vec<Started>> {
    let mut started: Vec<Started> = Vec::new();
    let mut by_prompt: HashMap<Hash, usize> = HashMap::new();
    let mut cursor = 0;
    for entry in entries {
        if let Some(prompt) = entry.tx.prompt_payload() {
            if prompt.route.level != RouteLevel::Turn {
                continue;
            }
            let at = cursor + sent_at(messages.get(cursor..)?, &prompt.text)?;
            cursor = at + 1;
            // Gate's own messages to the model carry on the attempt they
            // are sent in.
            if is_nudge(&prompt.text) || prompt.text.contains(REPAIR_OPENING) {
                continue;
            }
            // A prompt recorded for a move to a stronger model carries no
            // label of its own.
            let first = started.first().map(|first| first.terms);
            by_prompt.entry(entry.tx.hash).or_insert(started.len());
            started.push(Started {
                at,
                terms: Terms {
                    difficulty: prompt.difficulty.or(first.and_then(|t| t.difficulty)),
                    led: prompt.led.or(first.and_then(|t| t.led)),
                    routed,
                },
                ended: None,
            });
        } else if let Some(outcome) = entry.tx.outcome_payload() {
            if let Some(&i) = by_prompt.get(&outcome.prompt_tx) {
                started[i].ended.get_or_insert((entry.tx.ts_ms, outcome));
            }
        }
    }
    Some(started)
}

/// Where in `messages` the user sent `text`.
fn sent_at(messages: &[Message], text: &str) -> Option<usize> {
    let wanted = text.trim();
    messages
        .iter()
        .position(|message| sent_text(message).is_some_and(|sent| sent.trim() == wanted))
}
