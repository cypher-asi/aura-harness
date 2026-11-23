//! How much of the active agent's work went to subtasks: its turns and
//! model calls, and those of the subagents it led, read from their
//! records and shown overall and for its last turns.

use super::{money, percent, GateView};
use crate::event_loop::subagents::{lock, AgentHub};
use crate::record_loader::{format_local_time, model_call};
use aura_core_types::{AgentId, RecordEntry, RouteLevel, RouteReason};
use aura_gate::compare::list_price;
use aura_gate::lead::delegation::{self, Call, Delegation, Share, Subtask};
use aura_store_db::ReadStore;
use std::collections::HashMap;
use std::fmt::Write;
use tracing::warn;

/// Record entries read per scan for the delegation figures.
const SCAN_BATCH: usize = 500;

/// Turns of the lead `/gate` lists one by one.
const RECENT_TURNS: usize = 5;

/// What one agent's record says for the delegation figures.
#[derive(Default)]
struct Record {
    /// When each of its turns' own prompts was recorded, not counting
    /// the messages sent while a turn ran
    turns: Vec<u64>,
    /// When its first subtask prompt was recorded, and why it was routed
    /// where it was
    subtask: Option<(u64, RouteReason)>,
    /// Its model calls: when, and their list-price cost
    calls: Vec<Call>,
}

impl Record {
    /// Take in `entry`, the record's next.
    fn take(&mut self, entry: &RecordEntry) {
        let ts_ms = entry.tx.ts_ms;
        if let Some(prompt) = entry.tx.prompt_payload() {
            match prompt.route.level {
                RouteLevel::Turn if !prompt.route.model.is_empty() => self.turns.push(ts_ms),
                RouteLevel::Subtask if self.subtask.is_none() => {
                    self.subtask = Some((ts_ms, prompt.route.reason));
                }
                _ => {}
            }
        } else if let Some((model, usage)) = model_call(&entry.tx) {
            self.calls.push(Call {
                ts_ms,
                cost_usd: list_price(&model, &usage).unwrap_or(0.0),
            });
        }
    }

    /// The agent's work as one subtask of its lead; `None` when it has no
    /// subtask prompt.
    fn subtask(&self) -> Option<Subtask> {
        let (ts_ms, reason) = self.subtask?;
        Some(Subtask {
            ts_ms,
            escalated: reason == RouteReason::Escalate,
            calls: u32::try_from(self.calls.len()).unwrap_or(u32::MAX),
            cost_usd: self.calls.iter().map(|c| c.cost_usd).sum(),
        })
    }
}

/// What `agent`'s record says for the delegation figures, as far as the
/// store could read it.
fn read_record(store: &dyn ReadStore, agent: AgentId) -> Record {
    let mut record = Record::default();
    let mut from = 1;
    loop {
        let entries = match store.scan_record(agent, from, SCAN_BATCH) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(%agent, error = %e, "Could not read a record for /gate");
                break;
            }
        };
        for entry in &entries {
            record.take(entry);
        }
        match entries.last() {
            Some(last) if entries.len() == SCAN_BATCH => from = last.seq + 1,
            _ => break,
        }
    }
    record
}

/// How much of `lead`'s work went to subtasks: its turns and model
/// calls from its record, and each subagent below it whose nearest
/// agent with turns of its own is `lead` (a `/compare` run leads its own
/// subtasks) as one subtask, from that subagent's record.
pub(crate) fn delegation_of(hub: &AgentHub, lead: AgentId) -> Delegation {
    let store = hub.factory.store.as_ref();
    let below: Vec<(AgentId, Vec<AgentId>)> = {
        let registry = lock(&hub.registry);
        registry
            .descendants(lead)
            .into_iter()
            .map(|agent| (agent, registry.ancestry(agent)))
            .collect()
    };
    let own = read_record(store, lead);
    let records: HashMap<AgentId, Record> = below
        .iter()
        .map(|&(agent, _)| (agent, read_record(store, agent)))
        .collect();
    let leads =
        |agent: AgentId| agent == lead || records.get(&agent).is_some_and(|r| !r.turns.is_empty());
    let subtasks: Vec<Subtask> = below
        .iter()
        .filter(|(_, ancestry)| ancestry.iter().skip(1).find(|&&a| leads(a)) == Some(&lead))
        .filter_map(|(agent, _)| records[agent].subtask())
        .collect();
    delegation::summarize(&own.turns, &own.calls, &subtasks)
}

impl GateView<'_> {
    /// The share of the active agent's cost and model calls spent in
    /// subtasks, overall and for its last turns.
    pub(super) fn render_delegation(&self, out: &mut String) {
        let delegation = self.delegation;
        if delegation.turns.is_empty() {
            return;
        }
        let overall = &delegation.overall;
        let _ = write!(
            out,
            "\n\nDelegation in this agent's {} turn{}: {:.1} subtasks per turn, escalation rate {}\n  {}",
            delegation.turns.len(),
            if delegation.turns.len() == 1 { "" } else { "s" },
            delegation.subtasks_per_turn().unwrap_or(0.0),
            rate(overall.escalation_rate(), overall.escalations, overall.subtasks),
            share_line(overall),
        );
        let start = delegation.turns.len().saturating_sub(RECENT_TURNS);
        for turn in &delegation.turns[start..] {
            let _ = write!(
                out,
                "\n  {}  {} subtask{} · {}",
                format_local_time(turn.ts_ms),
                turn.share.subtasks,
                if turn.share.subtasks == 1 { "" } else { "s" },
                share_line(&turn.share),
            );
        }
    }
}

/// `40% (2 of 5)`, or `-` when there is nothing to count.
fn rate(share: Option<f64>, part: u32, whole: u32) -> String {
    share.map_or_else(
        || "-".to_string(),
        |share| format!("{} ({part} of {whole})", percent(share)),
    )
}

/// Cost and model calls in subtasks against the lead's.
fn share_line(share: &Share) -> String {
    let cost = share.cost_share().map_or_else(
        || "no cost".to_string(),
        |part| {
            format!(
                "{} of cost in subtasks ({} of {})",
                percent(part),
                money(share.subtask_cost_usd),
                money(share.lead_cost_usd + share.subtask_cost_usd)
            )
        },
    );
    let calls = rate(
        share.call_share(),
        share.subtask_calls,
        share.lead_calls + share.subtask_calls,
    );
    format!("{cost}, {calls} of model calls")
}
