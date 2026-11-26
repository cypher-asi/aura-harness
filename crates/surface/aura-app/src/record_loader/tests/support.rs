//! Shared builders for the record loader tests.

use super::*;

pub(super) fn append(store: &RocksStore, seq: u64, tx: Transaction) {
    let agent_id = tx.agent_id;
    let entry = RecordEntry::builder(seq, tx).build();
    store.append_entry_direct(agent_id, seq, &entry).unwrap();
}

pub(super) fn text_tx(agent_id: AgentId, tx_type: TransactionType, text: &str) -> Transaction {
    let payload = if tx_type == TransactionType::UserPrompt {
        aura_core_types::PromptPayload::new(text).to_bytes()
    } else {
        text.as_bytes().to_vec()
    };
    Transaction::new_chained(agent_id, tx_type, payload, None)
}

pub(super) fn tx(tx_type: TransactionType, payload: impl Into<Vec<u8>>) -> Transaction {
    Transaction::new_chained(AgentId::generate(), tx_type, payload.into(), None)
}

pub(super) fn json_tx(tx_type: TransactionType, payload: &Value) -> Transaction {
    tx(tx_type, serde_json::to_vec(payload).unwrap())
}

pub(super) fn proposal_tx(proposal: &ToolProposal) -> Transaction {
    Transaction::tool_proposal(AgentId::generate(), proposal).unwrap()
}

/// The chat rebuilt from a record holding `txs`.
pub(super) fn conversation_history_of(txs: &[Transaction]) -> Vec<MessageData> {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = txs[0].agent_id;
    for (i, t) in txs.iter().enumerate() {
        append(&store, i as u64 + 1, t.clone());
    }
    conversation_history(&store, agent).unwrap()
}

pub(super) fn execution(
    decision: ToolGateVerdict,
    result: Option<&str>,
    is_error: bool,
) -> Transaction {
    let exec = ToolExecution {
        tool_use_id: "t1".into(),
        tool: "read_file".into(),
        args: json!({"path": "a.rs"}),
        decision,
        reason: Some("policy says no".into()),
        result: result.map(str::to_string),
        is_error,
        duration_ms: None,
        parent_agent_id: AgentId::generate(),
        originating_user_id: "u".into(),
    };
    tx(
        TransactionType::ToolExecution,
        serde_json::to_vec(&exec).unwrap(),
    )
}

pub(super) fn policy_decision_tx(agent_id: AgentId) -> (Transaction, aura_core_types::Hash) {
    use aura_core_types::{
        PolicyApproval, PolicyCheck, PolicyCheckOutcome, PolicyRule, PolicySource,
    };

    let proposal = proposal_tx(&ToolProposal::new("t1", "run_command", json!({})));
    let decision = PolicyDecision {
        tool_use_id: "t1".into(),
        tool: "run_command".into(),
        verdict: ToolGateVerdict::Approved,
        reason: None,
        decided_by: PolicyRule::new(PolicySource::User, "on"),
        trace: vec![
            PolicyCheck {
                rule: PolicyRule::new(PolicySource::Mode, "agent"),
                outcome: PolicyCheckOutcome::Allow,
            },
            PolicyCheck {
                rule: PolicyRule::new(PolicySource::UserDefault, "auto_review"),
                outcome: PolicyCheckOutcome::Ask,
            },
        ],
        approval: Some(PolicyApproval {
            approver: PolicyApprover::User {
                user_id: Some("u-1".into()),
            },
            remember: Some("session".into()),
            request_id: None,
        }),
        mode: "agent".into(),
        policy_hash: "abc123".into(),
        action_id: None,
    };
    (
        Transaction::policy_decision(agent_id, &decision, proposal.hash).unwrap(),
        proposal.hash,
    )
}

/// The Record panel's (what, group, detail) columns for `t`.
pub(super) fn columns(t: &Transaction) -> (String, String, String) {
    let entry = RecordEntry::builder(1, t.clone()).build();
    let summary = build_record_summary(1, t, &entry, "12:00:00".into());
    (summary.what, summary.group, summary.detail)
}

pub(super) fn cols(what: &str, group: &str, detail: &str) -> (String, String, String) {
    (what.into(), group.into(), detail.into())
}

/// Assert `rows` list the agents of `tree` as the registry and `store`
/// describe them, with `active` the one marked active.
pub(super) fn check_rows(
    registry: &crate::agent_registry::AgentRegistry,
    store: &RocksStore,
    rows: &[(AgentId, AgentSummary)],
    tree: Vec<(&crate::agent_registry::RegisteredAgent, usize)>,
    active: AgentId,
) {
    assert_eq!(rows.len(), tree.len());
    for ((id, row), (agent, depth)) in rows.iter().zip(tree) {
        assert_eq!(*id, agent.agent_id());
        assert_eq!(row.id, id.to_hex());
        assert_eq!(row.depth, depth);
        assert_eq!(row.bench_run, registry.under_compare(*id), "{}", agent.name);
        assert_eq!(
            row.project,
            registry
                .project_of(*id)
                .map(crate::agent_registry::Project::name)
        );
        assert_eq!(row.is_active, *id == active);
        assert_eq!(row.record_count, store.get_head_seq(*id).unwrap());
    }
}
