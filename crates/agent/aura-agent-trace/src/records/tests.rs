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
        call: None,
        route: None,
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

fn reduce(seq: u64, tx_type: TransactionType, payload: &Value) -> RecordRow {
    let tx = Transaction::new_chained(
        AgentId::generate(),
        tx_type,
        Bytes::from(payload.to_string()),
        None,
    );
    RecordRow::from_entry(&RecordEntry::builder(seq, tx).build())
}

fn route_payload() -> Value {
    json!({
        "level": "turn",
        "model": "claude/claude-sonnet-5-5",
        "reason": "default",
        "explored": false,
        "propensity": 1.0,
        "effort": "medium",
    })
}

#[test]
fn rows_reduce_gate_classify_call() {
    let classify = reduce(
        1,
        TransactionType::Reasoning,
        &json!({
            "model": "claude-haiku-5-5",
            "stop_reason": "EndTurn",
            "input_tokens": 900,
            "output_tokens": 40,
            "cache_read_input_tokens": 100,
            "duration_ms": 700,
            "purpose": "classify",
        }),
    );
    assert_eq!(
        classify.call,
        Some(CallRow {
            model: "claude-haiku-5-5".into(),
            purpose: "classify".into(),
            stop_reason: "end_turn".into(),
            usage: Some(TokenUsage {
                input: 900,
                output: 40,
                cache_write: 0,
                cache_read: 100,
            }),
            duration_ms: Some(700),
            error: None,
        })
    );
}

#[test]
fn rows_reduce_failed_gate_call_and_skip_agents_own_calls() {
    let failed = reduce(
        2,
        TransactionType::Reasoning,
        &json!({
            "model": "claude-sonnet-5-5",
            "stop_reason": "Error",
            "stage": "complete",
            "error": "tool_choice is not supported",
            "purpose": "criteria",
        }),
    )
    .call
    .unwrap();
    assert_eq!(
        (
            failed.stop_reason.as_str(),
            failed.usage,
            failed.error.as_deref()
        ),
        ("error", None, Some("tool_choice is not supported"))
    );
    let agents_own = reduce(
        3,
        TransactionType::Reasoning,
        &json!({"model": "m", "stop_reason": "ToolUse", "input_tokens": 5, "output_tokens": 5}),
    );
    assert_eq!(agents_own.call, None);
}

#[test]
fn rows_reduce_routes() {
    let route = route_payload();
    let routed = reduce(
        4,
        TransactionType::UserPrompt,
        &json!({
            "text": "fix the parser",
            "category": "small_change",
            "classifier": "claude-haiku-5-5",
            "route": route,
            "difficulty": "medium",
        }),
    );
    assert_eq!(
        routed.route,
        Some(RouteRow {
            category: "small_change".into(),
            difficulty: Some("medium".into()),
            model: "claude/claude-sonnet-5-5".into(),
            effort: Some("medium".into()),
            reason: "default".into(),
            explored: false,
        })
    );
    let pinned = reduce(
        5,
        TransactionType::UserPrompt,
        &json!({"text": "hi", "classifier": "", "route": route}),
    );
    assert_eq!(pinned.route, None);
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
