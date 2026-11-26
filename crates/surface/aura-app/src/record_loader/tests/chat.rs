use super::*;

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
        Some("claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5")
    );
    assert_eq!(reply.usage.unwrap().tokens, 4 * 110);
}

#[test]
fn reply_footer_shows_the_thinking_level_again_on_reload() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let call = |effort: &str| {
        let payload = json!({
            "model": "claude/claude-sonnet-5-5",
            "input_tokens": 100,
            "output_tokens": 10,
            "thinking_effort": effort,
        });
        text_tx(agent, TransactionType::Reasoning, &payload.to_string())
    };
    let entries = [
        text_tx(agent, TransactionType::UserPrompt, "first"),
        call("high"),
        text_tx(agent, TransactionType::AgentMsg, "answer one"),
        text_tx(agent, TransactionType::UserPrompt, "second"),
        call("low"),
        call("high"),
        text_tx(agent, TransactionType::AgentMsg, "answer two"),
    ];
    for (seq, tx) in (1..).zip(entries) {
        append(&store, seq, tx);
    }

    let history = conversation_history(&store, agent).unwrap();
    let footer = |content: &str| {
        history
            .iter()
            .find(|m| m.content == content)
            .and_then(|m| m.model.clone())
    };
    assert_eq!(
        footer("answer one").as_deref(),
        Some("claude-sonnet-5-5 · high thinking")
    );
    assert_eq!(
        footer("answer two").as_deref(),
        Some("claude-sonnet-5-5 ×2 · low–high thinking")
    );
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
