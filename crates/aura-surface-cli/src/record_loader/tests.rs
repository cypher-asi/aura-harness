use super::*;
use aura_store_db::{RocksStore, WriteStore};

fn append(store: &RocksStore, seq: u64, tx: Transaction) {
    let agent_id = tx.agent_id;
    let entry = RecordEntry::builder(seq, tx).build();
    store.append_entry_direct(agent_id, seq, &entry).unwrap();
}

fn text_tx(agent_id: AgentId, tx_type: TransactionType, text: &str) -> Transaction {
    Transaction::new_chained(agent_id, tx_type, text.as_bytes().to_vec(), None)
}

#[test]
fn conversation_history_rebuilds_chat_per_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let alice = AgentId::generate();
    let bob = AgentId::generate();

    append(&store, 1, Transaction::session_start(alice));
    append(
        &store,
        2,
        text_tx(alice, TransactionType::UserPrompt, "hi alice"),
    );
    append(
        &store,
        3,
        text_tx(alice, TransactionType::ToolExecution, "not json"),
    );
    append(
        &store,
        4,
        text_tx(alice, TransactionType::AgentMsg, "hello from alice"),
    );
    append(&store, 5, text_tx(alice, TransactionType::AgentMsg, ""));
    append(
        &store,
        1,
        text_tx(bob, TransactionType::UserPrompt, "hi bob"),
    );

    let history = conversation_history(&store, alice).unwrap();
    let rendered: Vec<(MessageRole, &str)> = history
        .iter()
        .map(|m| (m.role, m.content.as_str()))
        .collect();
    assert_eq!(
        rendered,
        [
            (MessageRole::System, "── new session ──"),
            (MessageRole::User, "hi alice"),
            (MessageRole::Assistant, "hello from alice"),
        ]
    );

    let bob_history = conversation_history(&store, bob).unwrap();
    assert_eq!(bob_history.len(), 1);
    assert_eq!(bob_history[0].content, "hi bob");

    assert!(conversation_history(&store, AgentId::generate())
        .unwrap()
        .is_empty());
}

#[test]
fn recent_record_summaries_are_scoped_to_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let alice = AgentId::generate();
    let bob = AgentId::generate();

    append(&store, 1, text_tx(alice, TransactionType::UserPrompt, "a1"));
    append(&store, 2, text_tx(alice, TransactionType::AgentMsg, "a2"));
    append(&store, 1, text_tx(bob, TransactionType::UserPrompt, "b1"));

    let summaries = recent_record_summaries(&store, alice).unwrap();
    let seqs: Vec<u64> = summaries.iter().map(|s| s.seq).collect();
    assert_eq!(seqs, [1, 2]);
    assert!(summaries.iter().all(|s| s.agent_id == alice.to_hex()));
}

use serde_json::json;
fn tx(tx_type: TransactionType, payload: impl Into<Vec<u8>>) -> Transaction {
    Transaction::new_chained(AgentId::generate(), tx_type, payload.into(), None)
}

fn json_tx(tx_type: TransactionType, payload: &Value) -> Transaction {
    tx(tx_type, serde_json::to_vec(payload).unwrap())
}

#[test]
fn labels_fit_kind_column() {
    for t in [
        TransactionType::UserPrompt,
        TransactionType::AgentMsg,
        TransactionType::Trigger,
        TransactionType::ActionResult,
        TransactionType::System,
        TransactionType::SessionStart,
        TransactionType::ToolProposal,
        TransactionType::ToolExecution,
        TransactionType::PolicyDecision,
        TransactionType::ProcessComplete,
        TransactionType::Reasoning,
        TransactionType::SubagentSpawn,
    ] {
        assert!(tx_type_label(t).0.len() <= 8, "{t:?} label too long");
    }
}

#[test]
fn prompt_collapses_to_one_line() {
    let t = tx(
        TransactionType::UserPrompt,
        "fix the login\n\n  redirect bug\tplease",
    );
    assert_eq!(summarize_record(&t), "fix the login redirect bug please");
}

#[test]
fn long_prompt_is_capped_char_safely() {
    let t = tx(TransactionType::UserPrompt, "修".repeat(500));
    let out = summarize_record(&t);
    assert_eq!(out.chars().count(), MAX_INFO_CHARS);
    assert!(out.ends_with("..."));
}

#[test]
fn tool_proposal_shows_primary_arg() {
    let t = json_tx(
        TransactionType::ToolProposal,
        &json!({"tool_use_id": "t1", "tool": "read_file", "args": {"path": "src/main.rs"}}),
    );
    assert_eq!(summarize_record(&t), "read_file src/main.rs");
}

#[test]
fn tool_proposal_run_command() {
    let t = json_tx(
        TransactionType::ToolProposal,
        &json!({"tool_use_id": "t1", "tool": "run_command",
                "args": {"program": "cargo", "args": ["test", "-p", "x"]}}),
    );
    assert_eq!(summarize_record(&t), "run_command cargo test -p x");
}

fn proposal_tx(proposal: &ToolProposal) -> Transaction {
    Transaction::tool_proposal(AgentId::generate(), proposal).unwrap()
}

#[test]
fn user_command_is_a_tool_call_from_user() {
    let proposal = crate::event_loop::run_command::user_command_proposal("ls | head -n 2", None);
    let t = proposal_tx(&proposal);
    let entry = RecordEntry::builder(7, t.clone()).build();

    let summary = build_record_summary(7, &t, &entry, "12:00:00".into());

    assert_eq!(summary.tx_kind, "ToolCall");
    assert_eq!(summary.sender, "USER");
    assert_eq!(summary.info, "run_command ls | head -n 2");
}

#[test]
fn model_tool_call_keeps_llm_sender() {
    let t = proposal_tx(&ToolProposal::new(
        "t1",
        "read_file",
        json!({"path": "a.rs"}),
    ));
    let entry = RecordEntry::builder(1, t.clone()).build();

    let summary = build_record_summary(1, &t, &entry, "12:00:00".into());

    assert_eq!(summary.tx_kind, "ToolCall");
    assert_eq!(summary.sender, "LLM");
}

#[test]
fn user_commands_appear_in_conversation_history_as_shell_commands() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let proposal = crate::event_loop::run_command::user_command_proposal("ls -la", Some(80));
    append(
        &store,
        1,
        Transaction::tool_proposal(agent, &proposal).unwrap(),
    );

    let history = conversation_history(&store, agent).unwrap();

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, MessageRole::User);
    assert_eq!(history[0].command.as_deref(), Some("ls -la"));
}

fn execution(decision: ToolGateVerdict, result: Option<&str>, is_error: bool) -> Transaction {
    let exec = ToolExecution {
        tool_use_id: "t1".into(),
        tool: "read_file".into(),
        args: json!({"path": "a.rs"}),
        decision,
        reason: Some("policy says no".into()),
        result: result.map(str::to_string),
        is_error,
        parent_agent_id: AgentId::generate(),
        originating_user_id: "u".into(),
    };
    tx(
        TransactionType::ToolExecution,
        serde_json::to_vec(&exec).unwrap(),
    )
}

#[test]
fn tool_execution_outcomes() {
    assert_eq!(
        summarize_record(&execution(ToolGateVerdict::Approved, Some("hello"), false)),
        "read_file a.rs · ok 5B"
    );
    assert_eq!(
        summarize_record(&execution(
            ToolGateVerdict::Approved,
            Some("\nNo such file\nmore"),
            true
        )),
        "read_file a.rs · err: No such file"
    );
    assert_eq!(
        summarize_record(&execution(ToolGateVerdict::Denied, None, false)),
        "read_file a.rs · denied: policy says no"
    );
}

#[test]
fn tool_execution_reads_output_from_effects() {
    use aura_core_types::{ActionId, Effect, EffectKind, ToolResult};

    let t = execution(ToolGateVerdict::Approved, None, false);
    let result = ToolResult::success("read_file", "hello");
    let effect = Effect::new(
        ActionId::generate(),
        EffectKind::Agreement,
        EffectStatus::Committed,
        serde_json::to_vec(&result).unwrap(),
    );
    let entry = RecordEntry::builder(3, t.clone())
        .effects(vec![effect])
        .build();

    let summary = build_record_summary(3, &t, &entry, "12:00:00".into());
    assert_eq!(summary.tx_kind, "ToolRes");
    assert_eq!(summary.info, "read_file a.rs · ok 5B");
    assert!(summary.policy.is_none());
}

fn policy_decision_tx(agent_id: AgentId) -> (Transaction, aura_core_types::Hash) {
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

#[test]
fn policy_decision_names_the_rule_and_carries_detail() {
    let (t, proposal_hash) = policy_decision_tx(AgentId::generate());
    let entry = RecordEntry::builder(2, t.clone()).build();

    let summary = build_record_summary(2, &t, &entry, "12:00:00".into());

    assert_eq!(summary.tx_kind, "Policy");
    assert_eq!(summary.sender, "KERNEL");
    assert_eq!(summary.info, "run_command · allow · user.on");
    let policy = summary.policy.expect("policy detail");
    assert_eq!(policy.verdict, "approved");
    assert_eq!(policy.decided_by, "user.on");
    assert_eq!(policy.approval, "user u-1 (session)");
    assert_eq!(
        policy.trace,
        ["allow mode.agent", "ask   user_default.auto_review"]
    );
    assert_eq!(policy.proposal_tx, proposal_hash.to_hex());
    assert_eq!(policy.policy_hash, "abc123");
}

#[test]
fn tool_call_history_shows_command_then_policy_line() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let proposal = ToolProposal::new("t1", "read_file", json!({"path": "a.rs"}));
    append(
        &store,
        1,
        Transaction::tool_proposal(agent, &proposal).unwrap(),
    );
    append(&store, 2, policy_decision_tx(agent).0);
    let exec = ToolExecution {
        tool_use_id: "t1".into(),
        tool: "read_file".into(),
        args: json!({"path": "a.rs"}),
        decision: ToolGateVerdict::Approved,
        reason: None,
        result: None,
        is_error: false,
        parent_agent_id: agent,
        originating_user_id: String::new(),
    };
    append(
        &store,
        3,
        Transaction::tool_execution(agent, &exec).unwrap(),
    );

    let history = conversation_history(&store, agent).unwrap();
    let rendered: Vec<&str> = history.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        rendered,
        ["$ read_file", "policy: run_command allowed by user.on"]
    );
}

#[test]
fn reasoning_completed_and_failed() {
    let ok = json_tx(
        TransactionType::Reasoning,
        &json!({"model": "claude-opus-4-6", "stop_reason": "ToolUse",
                "input_tokens": 1234, "output_tokens": 340,
                "tool_uses": ["read_file", "grep"]}),
    );
    assert_eq!(
        summarize_record(&ok),
        "claude-opus-4-6 · 1.2k/340 tok · ToolUse · tools: read_file, grep"
    );
    let failed = json_tx(
        TransactionType::Reasoning,
        &json!({"status": "failed", "error": "rate limited"}),
    );
    assert_eq!(summarize_record(&failed), "failed: rate limited");
}

#[test]
fn system_kind_and_detail() {
    let t = json_tx(
        TransactionType::System,
        &json!({"system_kind": "domain_mutation", "phase": "request", "method": "create_spec"}),
    );
    assert_eq!(
        summarize_record(&t),
        "domain_mutation · create_spec request"
    );
}

#[test]
fn session_start() {
    let t = tx(TransactionType::SessionStart, "session_start");
    assert_eq!(summarize_record(&t), "new session");
}

#[test]
fn unparseable_json_falls_back_to_text() {
    let t = tx(TransactionType::Reasoning, "not json");
    assert_eq!(summarize_record(&t), "not json");
    let binary = tx(TransactionType::ToolProposal, vec![0xff, 0xfe]);
    assert_eq!(summarize_record(&binary), "");
}
