//! The builder linking tools, decisions, approvals and cache hits.

use super::*;

fn implement_now_nudge() -> AgentLoopEvent {
    AgentLoopEvent::Debug(DebugEvent::Decision {
        timestamp: now(),
        decision: Box::new(
            HarnessDecision::new(
                DecisionSource::Steering,
                "implement_now",
                HarnessAction::Nudge {
                    kind: "implement_now".into(),
                },
            )
            .at_iteration(0),
        ),
        task_id: None,
    })
}

fn read_file_result(tool_use_id: &str, content: &str) -> AgentLoopEvent {
    AgentLoopEvent::ToolResult {
        tool_use_id: tool_use_id.into(),
        tool_name: "read_file".into(),
        content: content.into(),
        is_error: false,
        image: None,
    }
}

#[test]
fn builder_links_tools_to_model_and_decisions_to_next_model() {
    let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
    let prompt = b.user_prompt("fix it", 1);
    assert_eq!(prompt.turn, 1);
    let nudge = b.on_event(&implement_now_nudge(), 2).unwrap();
    let model = b
        .on_event(
            &exchange(0, &[("t1", "read_file", json!({"path": "a.rs"}))]),
            3,
        )
        .unwrap();
    assert_eq!(model.links.caused_by, vec![nudge.seq]);
    assert_eq!(model.usage.unwrap().cache_read, 50);
    b.on_event(
        &AgentLoopEvent::ToolStart {
            id: "t1".into(),
            name: "read_file".into(),
        },
        4,
    );
    let result = b
        .on_event(&read_file_result("t1", "fn main() {}"), 9)
        .unwrap();
    assert_eq!(result.parent, Some(model.seq));
    assert_eq!(result.duration_ms, Some(5));
    assert_eq!(result.links.files, vec!["a.rs".to_string()]);
    let end = b.turn_end(&AgentLoopResult::default(), 10);
    assert!(matches!(end.action, StepAction::TurnEnd { .. }));
    assert_eq!(end.seq, 5);
}

fn approval_row(tx_type: &str, at_ms: u64, id: &str) -> RecordRow {
    RecordRow {
        seq: 0,
        tx_type: tx_type.into(),
        at_ms,
        tool_use_id: Some(id.into()),
        tool: Some("edit_file".into()),
        verdict: None,
        approver: None,
        rule: None,
        reason: None,
        call: None,
        route: None,
    }
}

/// A builder whose model just proposed three edits, and that model step.
fn three_proposed_edits() -> (StepBuilder, Step) {
    let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
    b.user_prompt("x", 0);
    let model = b
        .on_event(
            &exchange(
                0,
                &[
                    ("quick", "edit_file", json!({"path": "a.ts"})),
                    ("slow", "edit_file", json!({"path": "a.ts"})),
                    ("no", "edit_file", json!({"path": "a.ts"})),
                ],
            ),
            1,
        )
        .unwrap();
    (b, model)
}

#[test]
fn approval_waits_become_policy_steps() {
    let row = approval_row;
    let (mut b, model) = three_proposed_edits();
    for id in ["quick", "slow", "no"] {
        assert!(b.on_record(&row("tool_proposal", 10, id)).is_none());
    }
    assert!(
        b.on_record(&row("policy_decision", 13, "quick")).is_none(),
        "a decision nobody was asked about is no step"
    );

    let slow = b
        .on_record(&RecordRow {
            verdict: Some("approved".into()),
            approver: Some("user".into()),
            ..row("policy_decision", 244_678, "slow")
        })
        .unwrap();
    assert_eq!(slow.parent, Some(model.seq));
    let StepAction::Harness { name, action, .. } = &slow.action else {
        panic!("expected a harness step, got {:?}", slow.action);
    };
    assert_eq!(name, "approval_wait");
    assert!(
        matches!(action, HarnessAction::Warn { message } if message.contains("4m 04s")),
        "{action:?}"
    );

    let denied = b
        .on_record(&RecordRow {
            verdict: Some("denied".into()),
            ..row("policy_decision", 600_010, "no")
        })
        .unwrap();
    assert!(matches!(
        denied.action,
        StepAction::Harness {
            action: HarnessAction::Block { .. },
            ..
        }
    ));
}

#[test]
fn cache_hits_mark_tool_results() {
    let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
    b.user_prompt("x", 0);
    b.on_event(
        &exchange(0, &[("t1", "read_file", json!({"path": "a"}))]),
        1,
    );
    let hit = b
        .on_event(
            &AgentLoopEvent::Debug(DebugEvent::Decision {
                timestamp: now(),
                decision: Box::new(
                    HarnessDecision::new(
                        DecisionSource::Cache,
                        "tool_result_cache",
                        HarnessAction::CacheHit {
                            tool: "read_file".into(),
                            tool_use_id: "t1".into(),
                        },
                    )
                    .with_tool_use("t1"),
                ),
                task_id: None,
            }),
            2,
        )
        .unwrap();
    assert_eq!(hit.parent, Some(2));
    let result = b
        .on_event(
            &AgentLoopEvent::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "read_file".into(),
                content: "x".into(),
                is_error: false,
                image: None,
            },
            3,
        )
        .unwrap();
    let StepAction::ToolResult { source, .. } = result.action else {
        panic!("expected tool result");
    };
    assert_eq!(source, "cache");
}
