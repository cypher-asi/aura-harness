//! Join between a run's steps and the kernel record log.
//!
//! The kernel's record observer hands every committed entry to
//! [`RecordRouter`], which appends a compact [`RecordRow`] to the owning
//! run's `record.jsonl`. [`join`] then fills each step's
//! `links.record_seqs`: tool steps match on `tool_use_id`, user prompts
//! match in order.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use aura_agent_kernel::RecordObserver;
use aura_core_types::trace::{Step, StepAction};
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
}

impl RecordRow {
    /// Reduce `entry`.
    #[must_use]
    pub fn from_entry(entry: &RecordEntry) -> Self {
        let payload: Value = serde_json::from_slice(&entry.tx.payload).unwrap_or(Value::Null);
        let field = |name: &str| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let policy = entry.tx.tx_type == TransactionType::PolicyDecision;
        let rule = payload.get("decided_by").and_then(|by| {
            let part = |key: &str| by.get(key).and_then(Value::as_str).unwrap_or("");
            (!part("rule").is_empty()).then(|| format!("{}.{}", part("source"), part("rule")))
        });
        Self {
            seq: entry.seq,
            tx_type: serde_json::to_value(entry.tx.tx_type)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            at_ms: entry.tx.ts_ms,
            tool_use_id: field("tool_use_id"),
            tool: field("tool"),
            verdict: field("verdict"),
            approver: payload
                .pointer("/approval/approver/kind")
                .and_then(Value::as_str)
                .map(str::to_string),
            rule: rule.filter(|_| policy),
            reason: field("reason").filter(|_| policy),
        }
    }

    fn is_user_prompt(&self) -> bool {
        serde_json::to_value(TransactionType::UserPrompt)
            .ok()
            .is_some_and(|v| v.as_str() == Some(self.tx_type.as_str()))
    }
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
#[derive(Debug, Default)]
pub struct RecordRouter {
    routes: Mutex<HashMap<AgentId, Weak<RunRecorder>>>,
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
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(agent, Arc::downgrade(recorder));
    }

    /// Stop routing `agent`'s entries.
    pub fn detach(&self, agent: AgentId) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&agent);
    }
}

impl RecordObserver for RecordRouter {
    fn on_entry(&self, entry: &RecordEntry) {
        let recorder = self
            .routes
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
mod tests {
    use super::*;
    use crate::rewards::tests::{model, result};
    use aura_core_types::Transaction;
    use bytes::Bytes;
    use serde_json::json;

    fn row(seq: u64, tx_type: &str, tool_use_id: Option<&str>) -> RecordRow {
        RecordRow {
            seq,
            tx_type: tx_type.into(),
            at_ms: 0,
            tool_use_id: tool_use_id.map(str::to_string),
            tool: None,
            verdict: None,
            approver: None,
            rule: None,
            reason: None,
        }
    }

    #[test]
    fn rows_reduce_tool_payloads() {
        let agent = AgentId::generate();
        let payload = json!({"tool_use_id": "t1", "tool": "read_file", "args": {}});
        let tx = Transaction::new_chained(
            agent,
            TransactionType::ToolProposal,
            Bytes::from(payload.to_string()),
            None,
        );
        let r = RecordRow::from_entry(&RecordEntry::builder(7, tx).build());
        assert_eq!(r.seq, 7);
        assert_eq!(r.tx_type, "tool_proposal");
        assert_eq!(r.tool_use_id.as_deref(), Some("t1"));
        assert_eq!(r.tool.as_deref(), Some("read_file"));
        assert_eq!(r.approver, None);

        let payload = json!({
            "tool_use_id": "t1",
            "tool": "edit_file",
            "verdict": "approved",
            "reason": "asked the user",
            "decided_by": {"source": "mode", "rule": "ask_on_write"},
            "approval": {"approver": {"kind": "user"}, "remember": "once"},
        });
        let tx = Transaction::new_chained(
            agent,
            TransactionType::PolicyDecision,
            Bytes::from(payload.to_string()),
            None,
        );
        let r = RecordRow::from_entry(&RecordEntry::builder(8, tx).build());
        assert_eq!(r.verdict.as_deref(), Some("approved"));
        assert_eq!(r.approver.as_deref(), Some("user"));
        assert_eq!(r.rule.as_deref(), Some("mode.ask_on_write"));
        assert_eq!(r.reason.as_deref(), Some("asked the user"));
    }

    #[test]
    fn join_links_tools_and_prompts() {
        let mut prompt = model(1, &[]);
        prompt.action = StepAction::UserPrompt { text: "hi".into() };
        let mut steps = vec![
            prompt,
            model(2, &[("a", "read_file", json!({"path": "x"}))]),
            result(3, 2, "a", "read_file", "x", "read", false),
        ];
        let rows = [
            row(1, "user_prompt", None),
            row(2, "tool_proposal", Some("a")),
            row(3, "tool_execution", Some("a")),
            row(4, "tool_proposal", Some("other")),
        ];
        assert_eq!(join(&mut steps, &rows), 3);
        assert_eq!(steps[0].links.record_seqs, vec![1]);
        assert_eq!(steps[1].links.record_seqs, vec![2, 3]);
        assert_eq!(steps[2].links.record_seqs, vec![2, 3]);
    }
}
