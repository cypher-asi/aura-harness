//! Join between a run's steps and the kernel record log.
//!
//! The kernel's record observer hands every committed entry to
//! [`RecordRouter`], which appends a compact [`RecordRow`] to the owning
//! run's `record.jsonl`. [`join`] then fills each step's
//! `links.record_seqs`: tool steps match on `tool_use_id`, user prompts
//! match in order.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use aura_agent_kernel::{QueuedObserver, RecordObserver};
use aura_core_types::trace::{Step, StepAction, TokenUsage};
use aura_core_types::{AgentId, RecordEntry, TransactionType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::recorder::RunRecorder;

/// One kernel record entry, reduced to what the join needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordRow {
    /// Record-log sequence number.
    pub seq: u64,
    /// Transaction type (`user_prompt`, `tool_proposal`, ...).
    pub tx_type: String,
    /// Transaction timestamp.
    pub at_ms: u64,
    /// Tool-use id named by the payload, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Tool named by the payload, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Gate verdict of a policy decision (`approved`, `denied`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Who answered the approval prompt behind a policy decision
    /// (`user`, `hook`); `None` when nobody was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approver: Option<String>,
    /// Rule behind a policy decision (`source.rule`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Why a policy decision was made, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A model call made beside the agent's own work, from a `reasoning`
    /// entry that names what the call was for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<CallRow>,
    /// Where Gate sent a prompt's work, from a `user_prompt` entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<RouteRow>,
}

/// A model call made beside the agent's own work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRow {
    /// Model asked.
    pub model: String,
    /// What the call was for (`classify`, `criteria`, `judge`, `title`).
    pub purpose: String,
    /// Provider stop reason, `error` when the call failed.
    pub stop_reason: String,
    /// Tokens the call used; `None` when it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// Wall time of the call, when it returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Why the call failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Where Gate sent a prompt's work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRow {
    /// What the prompt asks for.
    pub category: String,
    /// How hard the classifier judged the work, when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<String>,
    /// Model the work went to.
    pub model: String,
    /// Reasoning effort Gate chose for it, when it chose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Why the work went there (`default`, `proven`, `explore`, ...).
    pub reason: String,
    /// Whether Gate explored instead of taking its pick.
    #[serde(default)]
    pub explored: bool,
}

fn payload_str(payload: &Value, pointer: &str) -> Option<String> {
    payload
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn call_row(payload: &Value) -> Option<CallRow> {
    let field = |name: &str| payload_str(payload, name);
    let count = |name: &str| payload.get(name).and_then(Value::as_u64);
    field("/purpose").map(|purpose| CallRow {
        model: field("/model").unwrap_or_default(),
        purpose,
        stop_reason: snake_case(&field("/stop_reason").unwrap_or_default()),
        usage: count("input_tokens").map(|input| TokenUsage {
            input,
            output: count("output_tokens").unwrap_or_default(),
            cache_write: count("cache_creation_input_tokens").unwrap_or_default(),
            cache_read: count("cache_read_input_tokens").unwrap_or_default(),
        }),
        duration_ms: count("duration_ms"),
        error: field("/error"),
    })
}

fn route_row(payload: &Value) -> Option<RouteRow> {
    let field = |name: &str| payload_str(payload, name);
    field("/category").map(|category| RouteRow {
        category,
        difficulty: field("/difficulty"),
        model: field("/route/model").unwrap_or_default(),
        effort: field("/route/effort"),
        reason: field("/route/reason").unwrap_or_default(),
        explored: payload
            .pointer("/route/explored")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    })
}

impl RecordRow {
    /// Reduce `entry`.
    #[must_use]
    pub fn from_entry(entry: &RecordEntry) -> Self {
        let payload: Value = serde_json::from_slice(&entry.tx.payload).unwrap_or(Value::Null);
        let field = |name: &str| {
            payload
                .pointer(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let policy = entry.tx.tx_type == TransactionType::PolicyDecision;
        let rule = payload.get("decided_by").and_then(|by| {
            let part = |key: &str| by.get(key).and_then(Value::as_str).unwrap_or("");
            (!part("rule").is_empty()).then(|| format!("{}.{}", part("source"), part("rule")))
        });
        let call = call_row(&payload).filter(|_| entry.tx.tx_type == TransactionType::Reasoning);
        let route = route_row(&payload).filter(|_| entry.tx.tx_type == TransactionType::UserPrompt);
        Self {
            seq: entry.seq,
            tx_type: serde_json::to_value(entry.tx.tx_type)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            at_ms: entry.tx.ts_ms,
            tool_use_id: field("/tool_use_id"),
            tool: field("/tool"),
            verdict: field("/verdict"),
            approver: field("/approval/approver/kind"),
            rule: rule.filter(|_| policy),
            reason: field("/reason").filter(|_| policy),
            call,
            route,
        }
    }

    fn is_user_prompt(&self) -> bool {
        serde_json::to_value(TransactionType::UserPrompt)
            .ok()
            .is_some_and(|v| v.as_str() == Some(self.tx_type.as_str()))
    }
}

/// `EndTurn` as `end_turn`. The kernel records a stop reason by its
/// variant name, the agent loop by its wire name.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Fill `links.record_seqs` on `steps` from `rows`. Returns how many
/// steps were linked.
pub fn join(steps: &mut [Step], rows: &[RecordRow]) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let mut by_tool: HashMap<&str, Vec<u64>> = HashMap::new();
    let mut prompts = Vec::new();
    for row in rows {
        if let Some(id) = &row.tool_use_id {
            by_tool.entry(id.as_str()).or_default().push(row.seq);
        } else if row.is_user_prompt() {
            prompts.push(row.seq);
        }
    }
    let mut prompts = prompts.into_iter();
    let mut linked = 0;
    for step in steps {
        let mut seqs: BTreeSet<u64> = step.links.record_seqs.iter().copied().collect();
        let before = seqs.len();
        match &step.action {
            StepAction::UserPrompt { .. } => seqs.extend(prompts.next()),
            StepAction::ToolResult { tool_use_id, .. } => {
                seqs.extend(by_tool.get(tool_use_id.as_str()).into_iter().flatten());
            }
            StepAction::Model(action) => {
                for call in &action.tool_calls {
                    seqs.extend(by_tool.get(call.tool_use_id.as_str()).into_iter().flatten());
                }
            }
            _ => {}
        }
        for id in &step.links.tool_use_ids {
            seqs.extend(by_tool.get(id.as_str()).into_iter().flatten());
        }
        if seqs.len() > before {
            linked += 1;
            step.links.record_seqs = seqs.into_iter().collect();
        }
    }
    linked
}

/// Kernel record observer that forwards entries to the run recording
/// the entry's agent.
///
/// Rows are written on a thread of the router's own, so committing
/// kernels don't wait on trace files.
#[derive(Debug)]
pub struct RecordRouter {
    routes: Arc<Routes>,
    queue: Arc<QueuedObserver>,
}

#[derive(Debug, Default)]
struct Routes(Mutex<HashMap<AgentId, Weak<RunRecorder>>>);

impl Default for RecordRouter {
    fn default() -> Self {
        let routes = Arc::new(Routes::default());
        let queue = QueuedObserver::new("aura-trace-records", Arc::clone(&routes) as _);
        Self {
            routes,
            queue: Arc::new(queue),
        }
    }
}

impl RecordRouter {
    /// New router with no routes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Send `agent`'s entries to `recorder` until it is dropped or
    /// replaced.
    pub fn attach(&self, agent: AgentId, recorder: &Arc<RunRecorder>) {
        let queue = Arc::downgrade(&self.queue);
        recorder.wait_for_records(move || {
            if let Some(queue) = queue.upgrade() {
                queue.flush();
            }
        });
        self.routes
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(agent, Arc::downgrade(recorder));
    }

    /// Stop routing `agent`'s entries, once those already committed have
    /// reached its recorder.
    pub fn detach(&self, agent: AgentId) {
        self.flush();
        self.routes
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&agent);
    }

    /// Wait until every entry committed so far has reached its recorder.
    pub fn flush(&self) {
        self.queue.flush();
    }
}

impl RecordObserver for RecordRouter {
    fn on_entry(&self, entry: &RecordEntry) {
        self.queue.on_entry(entry);
    }
}

impl RecordObserver for Routes {
    fn on_entry(&self, entry: &RecordEntry) {
        let recorder = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&entry.tx.agent_id)
            .and_then(Weak::upgrade);
        if let Some(recorder) = recorder {
            recorder.on_record(&RecordRow::from_entry(entry));
        }
    }
}

#[cfg(test)]
mod tests;
