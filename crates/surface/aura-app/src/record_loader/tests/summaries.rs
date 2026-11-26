use super::*;

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
        subtask_cost_usd: 0.0,
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
