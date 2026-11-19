use super::*;
use aura_store_db::{RocksStore, WriteStore};

fn append(store: &RocksStore, seq: u64, tx: Transaction) {
    let agent_id = tx.agent_id;
    let entry = RecordEntry::builder(seq, tx).build();
    store.append_entry_direct(agent_id, seq, &entry).unwrap();
}

fn text_tx(agent_id: AgentId, tx_type: TransactionType, text: &str) -> Transaction {
    let payload = if tx_type == TransactionType::UserPrompt {
        aura_core_types::PromptPayload::new(text).to_bytes()
    } else {
        text.as_bytes().to_vec()
    };
    Transaction::new_chained(agent_id, tx_type, payload, None)
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
fn model_history_resumes_the_conversation_since_the_last_session_start() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let entries = [
        text_tx(agent, TransactionType::UserPrompt, "old question"),
        text_tx(agent, TransactionType::AgentMsg, "old answer"),
        Transaction::session_start(agent),
        text_tx(agent, TransactionType::UserPrompt, "make an app"),
        text_tx(agent, TransactionType::ToolExecution, "not json"),
        text_tx(agent, TransactionType::AgentMsg, "Which stack?"),
        text_tx(agent, TransactionType::UserPrompt, "a visualization"),
        text_tx(agent, TransactionType::AgentMsg, ""),
    ];
    for (seq, tx) in (1..).zip(entries) {
        append(&store, seq, tx);
    }

    let history = model_history(&store, agent).unwrap();
    let rendered: Vec<(aura_model::Role, &str)> = history
        .iter()
        .map(|m| match m.content.as_slice() {
            [aura_model::ContentBlock::Text { text }] => (m.role, text.as_str()),
            other => panic!("expected one text block, got {other:?}"),
        })
        .collect();
    use aura_model::Role::{Assistant, User};
    assert_eq!(
        rendered,
        [
            (User, "make an app"),
            (Assistant, "Which stack?"),
            (User, "a visualization"),
        ]
    );
    assert!(model_history(&store, AgentId::generate())
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
        TransactionType::Outcome,
        TransactionType::ProcessComplete,
        TransactionType::Reasoning,
        TransactionType::SubagentSpawn,
    ] {
        assert!(tx_type_label(t).0.len() <= 8, "{t:?} label too long");
    }
}

#[test]
fn prompt_collapses_to_one_line() {
    let t = Transaction::user_prompt(
        AgentId::generate(),
        "fix the login\n\n  redirect bug\tplease",
    );
    assert_eq!(summarize_record(&t), "fix the login redirect bug please");
}

#[test]
fn long_prompt_is_capped_char_safely() {
    let t = Transaction::user_prompt(AgentId::generate(), "修".repeat(500));
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
fn json_payload_message_is_pretty_printed_in_full() {
    let long = "x".repeat(300);
    let t = json_tx(
        TransactionType::ToolProposal,
        &json!({"tool": "run_command", "args": {"command": long}}),
    );
    let entry = RecordEntry::builder(1, t.clone()).build();

    let summary = build_record_summary(1, &t, &entry, "12:00:00".into());

    let parsed: Value = serde_json::from_str(&summary.message).unwrap();
    assert_eq!(parsed["args"]["command"], json!(long));
    assert!(summary.message.contains("\n  \"tool\": \"run_command\""));
}

#[test]
fn an_outcome_shows_its_signal_verdict_and_work() {
    let outcome = aura_core_types::OutcomePayload {
        prompt_tx: aura_core_types::Hash::from_content(b"prompt"),
        category: aura_core_types::PromptCategory::Debugging,
        model: "claude/claude-sonnet-4-6".to_string(),
        success: Some(false),
        signal: aura_core_types::OutcomeSignal::ChecksFailed,
        cost_usd: 0.0123,
        latency_ms: 12_300,
    };
    let t = Transaction::outcome(AgentId::generate(), &outcome);
    let entry = RecordEntry::builder(1, t.clone()).build();

    assert_eq!(
        summarize_record(&t),
        "checks_failed · debugging · claude/claude-sonnet-4-6 · $0.0123 · 12.3s"
    );
    let columns = record_columns(&t, &entry, "GATE").unwrap();
    assert_eq!(columns.what, "checks_failed");
    assert_eq!(columns.group, "failed");
    assert!(
        conversation_history_of(&[t]).is_empty(),
        "not shown in the chat"
    );
}

/// The chat rebuilt from a record holding `txs`.
fn conversation_history_of(txs: &[Transaction]) -> Vec<MessageData> {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = txs[0].agent_id;
    for (i, t) in txs.iter().enumerate() {
        append(&store, i as u64 + 1, t.clone());
    }
    conversation_history(&store, agent).unwrap()
}

#[test]
fn text_payload_message_is_truncated() {
    let t = text_tx(
        AgentId::generate(),
        TransactionType::AgentMsg,
        &"y".repeat(300),
    );
    let entry = RecordEntry::builder(1, t.clone()).build();

    let summary = build_record_summary(1, &t, &entry, "12:00:00".into());

    assert_eq!(summary.message.chars().count(), MAX_MESSAGE_CHARS);
    assert!(summary.message.ends_with("..."));
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
        duration_ms: None,
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
fn tool_call_history_keeps_the_policy_with_its_tool_block() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let proposal = ToolProposal::new("t1", "read_file", json!({"path": "a.rs"}));
    let proposal_tx = Transaction::tool_proposal(agent, &proposal).unwrap();
    let proposed_at = proposal_tx.ts_ms;
    append(&store, 1, proposal_tx);
    append(&store, 2, policy_decision_tx(agent).0);
    let exec = ToolExecution {
        tool_use_id: "t1".into(),
        tool: "read_file".into(),
        args: json!({"path": "a.rs"}),
        decision: ToolGateVerdict::Approved,
        reason: None,
        result: Some("fn main() {}".into()),
        is_error: false,
        duration_ms: None,
        parent_agent_id: agent,
        originating_user_id: String::new(),
    };
    let exec_tx = Transaction::tool_execution(agent, &exec).unwrap();
    let took = exec_tx.ts_ms - proposed_at;
    append(&store, 3, exec_tx);

    let history = conversation_history(&store, agent).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].activity,
        Some(ActivityData::Tool {
            id: "t1".into(),
            name: "read_file".into(),
            args: r#"{"path":"a.rs"}"#.into(),
            result: Some("fn main() {}".into()),
            is_error: false,
            duration_ms: Some(took),
            policy: Some("allowed by user.on".into()),
        })
    );
}

#[test]
fn policy_without_its_tool_call_is_a_chat_line() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    append(&store, 1, policy_decision_tx(agent).0);

    let history = conversation_history(&store, agent).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "policy: run_command allowed by user.on");
}

#[test]
fn denied_tool_call_history_is_an_error_block() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let proposal = ToolProposal::new("t1", "run_command", json!({"command": "rm -rf /"}));
    append(
        &store,
        1,
        Transaction::tool_proposal(agent, &proposal).unwrap(),
    );
    let exec = ToolExecution {
        tool_use_id: "t1".into(),
        tool: "run_command".into(),
        args: json!({"command": "rm -rf /"}),
        decision: ToolGateVerdict::Denied,
        reason: Some("blocked by policy".into()),
        result: None,
        is_error: false,
        duration_ms: None,
        parent_agent_id: agent,
        originating_user_id: String::new(),
    };
    append(
        &store,
        2,
        Transaction::tool_execution(agent, &exec).unwrap(),
    );

    let history = conversation_history(&store, agent).unwrap();
    let Some(ActivityData::Tool {
        result, is_error, ..
    }) = &history[0].activity
    else {
        panic!("expected a tool block: {history:?}");
    };
    assert!(is_error);
    assert_eq!(result.as_deref(), Some("denied: blocked by policy"));
}

#[test]
fn reasoning_with_thinking_becomes_a_thinking_block() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let with_thinking = json!({
        "model": "m",
        "thinking": "Let me check.",
        "thinking_ms": 1500,
    });
    append(
        &store,
        1,
        text_tx(
            agent,
            TransactionType::Reasoning,
            &with_thinking.to_string(),
        ),
    );
    append(
        &store,
        2,
        text_tx(agent, TransactionType::Reasoning, r#"{"model":"m"}"#),
    );

    let history = conversation_history(&store, agent).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].activity,
        Some(ActivityData::Thinking {
            text: "Let me check.".into(),
            duration_ms: Some(1500),
        })
    );
}

#[test]
fn reply_names_the_models_of_its_turn() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let entries = [
        text_tx(agent, TransactionType::UserPrompt, "first"),
        text_tx(agent, TransactionType::Reasoning, r#"{"model":"qwen3:4b"}"#),
        text_tx(
            agent,
            TransactionType::Reasoning,
            r#"{"model":"claude-opus-4-7"}"#,
        ),
        text_tx(agent, TransactionType::AgentMsg, "answer one"),
        text_tx(agent, TransactionType::UserPrompt, "second"),
        text_tx(agent, TransactionType::Reasoning, r#"{"status":"failed"}"#),
        text_tx(agent, TransactionType::AgentMsg, "answer two"),
    ];
    for (seq, tx) in (1..).zip(entries) {
        append(&store, seq, tx);
    }

    let history = conversation_history(&store, agent).unwrap();
    let models: Vec<(&str, Option<&str>)> = history
        .iter()
        .map(|m| (m.content.as_str(), m.model.as_deref()))
        .collect();
    assert_eq!(
        models,
        [
            ("first", None),
            ("answer one", Some("qwen3:4b · claude-opus-4-7")),
            ("second", None),
            ("answer two", None),
        ]
    );
}

#[test]
fn reply_footer_counts_the_classifier_before_its_prompt_and_the_title_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let call = |model: &str, purpose: Option<&str>| {
        let mut payload = json!({"model": model, "input_tokens": 100, "output_tokens": 10});
        if let Some(purpose) = purpose {
            payload["purpose"] = json!(purpose);
        }
        text_tx(agent, TransactionType::Reasoning, &payload.to_string())
    };
    let routed = aura_core_types::PromptPayload {
        route: aura_core_types::Route::turn(
            "claude/claude-sonnet-5-5",
            aura_core_types::RouteReason::Default,
        ),
        ..aura_core_types::PromptPayload::new("test")
    };
    let entries = [
        call("claude/claude-haiku-5-5", Some("classify")),
        Transaction::user_prompt_with(agent, &routed),
        call("claude/claude-sonnet-5-5", None),
        call("claude/claude-sonnet-5-5", None),
        text_tx(agent, TransactionType::AgentMsg, "done"),
        call("claude/claude-haiku-5-5", Some("title")),
    ];
    for (seq, tx) in (1..).zip(entries) {
        append(&store, seq, tx);
    }

    let history = conversation_history(&store, agent).unwrap();
    let reply = history.iter().find(|m| m.content == "done").unwrap();
    assert_eq!(
        reply.model.as_deref(),
        Some("claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5 · title: claude-haiku-5-5")
    );
    assert_eq!(reply.usage.unwrap().tokens, 4 * 110);
}

#[test]
fn reply_restores_the_tokens_and_cost_of_its_call() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let opus = json!({
        "model": "claude-opus-4-7",
        "input_tokens": 1_000,
        "output_tokens": 1_000,
        "cache_creation_input_tokens": 2_000,
        "cache_read_input_tokens": 1_000_000,
    });
    let local = json!({"model": "qwen3:4b", "input_tokens": 300, "output_tokens": 600});
    let entries = [
        text_tx(agent, TransactionType::UserPrompt, "first"),
        text_tx(agent, TransactionType::Reasoning, &opus.to_string()),
        text_tx(agent, TransactionType::AgentMsg, "answer one"),
        text_tx(agent, TransactionType::UserPrompt, "second"),
        text_tx(agent, TransactionType::Reasoning, &local.to_string()),
        text_tx(agent, TransactionType::AgentMsg, "answer two"),
    ];
    for (seq, tx) in (1..).zip(entries) {
        append(&store, seq, tx);
    }

    let history = conversation_history(&store, agent).unwrap();
    let usage = |content: &str| {
        history
            .iter()
            .find(|m| m.content == content)
            .and_then(|m| m.usage)
            .unwrap()
    };
    let opus = usage("answer one");
    assert_eq!(opus.tokens, 1_004_000);
    assert!((opus.cost_usd.unwrap() - 0.5425).abs() < 1e-9, "{opus:?}");
    let local = usage("answer two");
    assert_eq!(local.tokens, 900);
    assert_eq!(local.cost_usd, None);
}

#[test]
fn policy_line_is_hidden_only_for_allowed_user_commands() {
    let (t, _) = policy_decision_tx(AgentId::generate());
    let mut decision: PolicyDecision = serde_json::from_slice(&t.payload).unwrap();
    assert_eq!(
        policy_chat_line(&decision).as_deref(),
        Some("policy: run_command allowed by user.on")
    );

    decision.tool_use_id = "user_abc".into();
    assert_eq!(policy_chat_line(&decision), None);

    decision.verdict = ToolGateVerdict::Denied;
    assert_eq!(
        policy_chat_line(&decision).as_deref(),
        Some("policy: run_command denied by user.on")
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

/// The Record panel's (what, group, detail) columns for `t`.
fn columns(t: &Transaction) -> (String, String, String) {
    let entry = RecordEntry::builder(1, t.clone()).build();
    let summary = build_record_summary(1, t, &entry, "12:00:00".into());
    (summary.what, summary.group, summary.detail)
}

fn cols(what: &str, group: &str, detail: &str) -> (String, String, String) {
    (what.into(), group.into(), detail.into())
}

#[test]
fn tool_records_split_into_tool_group_and_detail() {
    let call = proposal_tx(&ToolProposal::new(
        "t1",
        "read_file",
        json!({"path": "src/main.rs"}),
    ));
    assert_eq!(columns(&call), cols("read_file", "file", "src/main.rs"));

    let result = execution(ToolGateVerdict::Approved, Some("hello"), false);
    assert_eq!(columns(&result), cols("read_file", "file", "a.rs · ok 5B"));

    let (policy, _) = policy_decision_tx(AgentId::generate());
    assert_eq!(
        columns(&policy),
        cols("run_command", "shell", "allow · user.on")
    );

    let unknown = proposal_tx(&ToolProposal::new("t2", "frobnicate", json!({})));
    assert_eq!(columns(&unknown), cols("frobnicate", "other", ""));
}

#[test]
fn tool_call_row_says_why_the_tool_did_not_run() {
    let cached = proposal_tx(
        &ToolProposal::new("t1", "read_file", json!({"path": "src/main.rs"})).with_source("cache"),
    );
    assert_eq!(
        columns(&cached),
        cols("read_file", "file", "src/main.rs · cached result")
    );

    let engine =
        proposal_tx(&ToolProposal::new("t2", "task_done", json!({})).with_source("task_executor"));
    assert_eq!(columns(&engine).2, "handled by task runner",);
}

#[test]
fn llm_records_show_stop_reason_and_model_or_error_class() {
    let ok = json_tx(
        TransactionType::Reasoning,
        &json!({"model": "claude-opus-4-6", "stop_reason": "ToolUse",
                "input_tokens": 1234, "output_tokens": 340,
                "tool_uses": ["read_file", "grep"]}),
    );
    assert_eq!(
        columns(&ok),
        cols("ToolUse", "opus", "1.2k/340 tok · tools: read_file, grep")
    );

    let failed = json_tx(
        TransactionType::Reasoning,
        &json!({"status": "failed", "error": "HTTP 429 Too Many Requests"}),
    );
    assert_eq!(
        columns(&failed),
        cols("error", "rate", "HTTP 429 Too Many Requests")
    );
    assert_eq!(
        llm_error_class("stream dropped before completion"),
        "stream"
    );
    assert_eq!(llm_error_class("Overloaded"), "busy");
    assert_eq!(llm_error_class("connection reset"), "other");
}

#[test]
fn conversation_records_show_sender() {
    let prompt = Transaction::user_prompt(AgentId::generate(), "hi there");
    assert_eq!(columns(&prompt), cols("USER", "text", "hi there"));
}

#[test]
fn unparseable_records_have_no_columns() {
    let t = tx(TransactionType::Reasoning, "not json");
    assert_eq!(columns(&t), cols("", "", ""));
}

#[test]
fn group_labels_fit_group_column() {
    let groups = [
        "read_file",
        "run_command",
        "search_code",
        "git_commit",
        "web_fetch",
        "create_spec",
        "create_task",
        "spawn_agent",
        "frobnicate",
    ]
    .map(tool_group);
    let errors = ["429", "529", "timed out", "401", "stream", "?"].map(llm_error_class);
    for label in groups.iter().chain(&errors) {
        assert!(label.len() <= 7, "{label} too long");
    }
    assert_eq!(model_family("claude-sonnet-4-5-20250929"), "sonnet");
    assert_eq!(model_family("openrouter/mistral-large"), "mistral");
}

#[test]
fn plan_usage_is_read_from_model_calls_only() {
    let agent = AgentId::generate();
    let call = text_tx(
        agent,
        TransactionType::Reasoning,
        r#"{"model":"claude-sonnet-4-6","input_tokens":3,"output_tokens":1,
            "plan_usage":[{"plan":"claude","window":"5h","used":0.25,"resets_at":1764554400}]}"#,
    );
    let readings = plan_usage(&call);
    assert_eq!(readings.len(), 1);
    assert_eq!(readings[0].plan, aura_core_types::Plan::Claude);
    assert_eq!(readings[0].window, "5h");
    assert_eq!(readings[0].resets_at, Some(1_764_554_400));

    let without = text_tx(agent, TransactionType::Reasoning, r#"{"model":"m"}"#);
    assert!(plan_usage(&without).is_empty());
    let unreadable = text_tx(
        agent,
        TransactionType::Reasoning,
        r#"{"model":"m","plan_usage":[{"plan":"mystery"}]}"#,
    );
    assert!(plan_usage(&unreadable).is_empty());
    let prompt = text_tx(agent, TransactionType::UserPrompt, "plan_usage");
    assert!(plan_usage(&prompt).is_empty());
}

/// The app was killed while the agent on screen, a spawn, was still queued:
/// on the next start it is archived and the app opens on its parent rather
/// than on the empty chat.
#[test]
fn an_unstarted_spawn_left_active_is_archived_and_not_opened() {
    let data = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let store = RocksStore::open(db.path(), false).unwrap();
    let mut registry = AgentRegistry::load_or_init(data.path()).unwrap();
    let root = registry.active().clone();
    append(
        &store,
        1,
        text_tx(root.agent_id(), TransactionType::UserPrompt, "go"),
    );
    let stuck = registry
        .create_instance("rust-engineer", Some(root.zns_id.clone()))
        .unwrap();
    let ran = registry
        .create_instance("explorer", Some(root.zns_id.clone()))
        .unwrap();
    append(
        &store,
        1,
        text_tx(ran.agent_id(), TransactionType::UserPrompt, "look"),
    );
    registry.set_active(stuck.agent_id()).unwrap();

    let archived = archive_unstarted_spawns(&mut registry, &store);

    assert_eq!(archived, [stuck.agent_id()]);
    assert_eq!(registry.active().agent_id(), root.agent_id());
    assert!(!registry.find(ran.agent_id()).unwrap().archived);
    // Archived and titled for good: the next start finds nothing to do.
    let reloaded = AgentRegistry::load_or_init(data.path()).unwrap();
    let stuck = reloaded.find(stuck.agent_id()).unwrap();
    assert!(stuck.archived);
    assert_eq!(
        stuck.title.as_deref(),
        Some("Did not start before the app closed")
    );
    let mut reloaded = reloaded;
    assert!(archive_unstarted_spawns(&mut reloaded, &store).is_empty());
}
