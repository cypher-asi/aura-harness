use super::*;

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
fn a_model_call_keeps_the_route_it_asked_for() {
    let agent = AgentId::generate();
    let routed = text_tx(
        agent,
        TransactionType::Reasoning,
        r#"{"model":"openai/gpt-6.1-sol","route":"openrouter/openai/gpt-6.1-sol",
            "input_tokens":1000000,"output_tokens":0}"#,
    );
    let call = model_call(&routed).unwrap();
    assert_eq!(
        call.model, "openai/gpt-6.1-sol",
        "shown as the answer named it"
    );
    assert_eq!(call.route.as_deref(), Some("openrouter/openai/gpt-6.1-sol"));
    assert_eq!(call.usage.input_tokens, 1_000_000);
    assert_eq!(call.list_price(), Some(2.0));
    assert_eq!(call.purpose, None, "the agent's own work");
    let criteria = text_tx(
        agent,
        TransactionType::Reasoning,
        r#"{"model":"openai/gpt-6.1-terra","purpose":"criteria","input_tokens":1}"#,
    );
    assert_eq!(
        model_call(&criteria).unwrap().purpose,
        Some(aura_model::ModelCallPurpose::Criteria)
    );

    // Older records keep no route; an empty or unknown one is none.
    for payload in [
        r#"{"model":"claude-haiku-4-5","input_tokens":1}"#,
        r#"{"model":"claude-haiku-4-5","route":"","input_tokens":1}"#,
        r#"{"model":"claude-haiku-4-5","route":"unknown","input_tokens":1}"#,
    ] {
        let call = model_call(&text_tx(agent, TransactionType::Reasoning, payload)).unwrap();
        assert_eq!(call.route, None, "{payload}");
    }
    // No answering model, or not a model call: nothing.
    let unanswered = r#"{"model":"unknown","route":"claude/claude-opus-4-7"}"#;
    assert!(model_call(&text_tx(agent, TransactionType::Reasoning, unanswered)).is_none());
    let prompt = text_tx(agent, TransactionType::UserPrompt, r#"{"model":"m"}"#);
    assert!(model_call(&prompt).is_none());
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
    // Named after its subtask before the app closed; the archive's note
    // must still show.
    registry
        .set_subtitle(stuck.agent_id(), "Fix parser escapes")
        .unwrap();

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
    let code = stuck.code.as_deref().unwrap();
    assert_eq!(
        stuck.display_name(),
        format!("{code}: Did not start before the app closed"),
        "the note wins over the subtitle and keeps the code"
    );
    let mut reloaded = reloaded;
    assert!(archive_unstarted_spawns(&mut reloaded, &store).is_empty());
}
