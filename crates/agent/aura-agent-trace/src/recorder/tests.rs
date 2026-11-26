use super::*;
use aura_core_types::trace::{ModelAction, TokenUsage, ToolCallAction, ToolIntent};

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

pub(crate) fn exchange(iteration: u32, calls: &[(&str, &str, Value)]) -> AgentLoopEvent {
    AgentLoopEvent::Debug(DebugEvent::ModelExchange {
        timestamp: now(),
        exchange: Box::new(ModelExchange {
            iteration,
            model: "m".into(),
            observation: None,
            action: ModelAction {
                tool_calls: calls
                    .iter()
                    .map(|(id, name, input)| ToolCallAction {
                        tool_use_id: (*id).into(),
                        name: (*name).into(),
                        input: input.clone(),
                        intent: ToolIntent::classify(name, input),
                    })
                    .collect(),
                stop_reason: "tool_use".into(),
                ..ModelAction::default()
            },
            usage: TokenUsage {
                input: 100,
                output: 10,
                cache_write: 0,
                cache_read: 50,
            },
            latency_ms: 7,
            provider_request_id: None,
        }),
        task_id: None,
    })
}

fn gate_row(seq: u64, call: Option<CallRow>, route: Option<RouteRow>) -> RecordRow {
    RecordRow {
        seq,
        tx_type: if call.is_some() {
            "reasoning"
        } else {
            "user_prompt"
        }
        .into(),
        at_ms: seq,
        tool_use_id: None,
        tool: None,
        verdict: None,
        approver: None,
        rule: None,
        reason: None,
        call,
        route,
    }
}

fn gate_call(model: &str, purpose: &str, usage: Option<TokenUsage>) -> CallRow {
    CallRow {
        model: model.into(),
        purpose: purpose.into(),
        stop_reason: if usage.is_some() { "end_turn" } else { "error" }.into(),
        usage,
        duration_ms: usage.map(|_| 900),
        error: usage
            .is_none()
            .then(|| "tool_choice is not supported".into()),
    }
}

#[test]
fn gate_calls_routes_and_follow_ups_are_steps_of_the_turn() {
    let usage = TokenUsage {
        input: 1_000,
        output: 100,
        ..TokenUsage::default()
    };
    let gate = Actor::Harness {
        source: DecisionSource::Gate,
    };
    let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
    let mut steps = vec![b.user_prompt("fix the parser", 0)];

    let classify = b
        .on_record(&gate_row(
            1,
            Some(gate_call("claude-haiku-5-5", "classify", Some(usage))),
            None,
        ))
        .unwrap();
    assert_eq!((classify.turn, classify.actor), (1, gate));
    assert_eq!(classify.policy.id, "claude-haiku-5-5");
    assert_eq!(
        (classify.usage, classify.duration_ms),
        (Some(usage), Some(900))
    );
    assert_eq!(classify.links.record_seqs, vec![1]);
    let StepAction::Model(action) = &classify.action else {
        panic!("expected a model step, got {:?}", classify.action);
    };
    assert_eq!(action.purpose.as_deref(), Some("classify"));
    steps.push(classify);

    let failed = b
        .on_record(&gate_row(
            2,
            Some(gate_call("claude-sonnet-5-5", "criteria", None)),
            None,
        ))
        .unwrap();
    let StepAction::Model(action) = &failed.action else {
        panic!("expected a model step, got {:?}", failed.action);
    };
    assert_eq!(
        action.error.as_deref(),
        Some("tool_choice is not supported")
    );
    assert_eq!(failed.usage, None);
    steps.push(failed);
    assert!(
        b.on_record(&gate_row(
            3,
            Some(gate_call("claude-haiku-5-5", "title", Some(usage))),
            None,
        ))
        .is_none(),
        "naming the chat is not Gate's"
    );

    let route = b
        .on_record(&gate_row(
            4,
            None,
            Some(RouteRow {
                category: "small_change".into(),
                difficulty: Some("medium".into()),
                model: "claude/claude-sonnet-5-5".into(),
                effort: Some("medium".into()),
                reason: "compare".into(),
                explored: false,
            }),
        ))
        .unwrap();
    let StepAction::Harness { name, action, .. } = &route.action else {
        panic!("expected a harness step, got {:?}", route.action);
    };
    assert_eq!((name.as_str(), route.actor), ("route", gate));
    assert_eq!(
        action,
        &HarnessAction::Route {
            model: "claude/claude-sonnet-5-5".into(),
            effort: Some("medium".into()),
        }
    );
    let Observation::Harness { features, trigger } = &route.observation else {
        panic!("expected what Gate saw, got {:?}", route.observation);
    };
    assert_eq!(
        (&features["difficulty"], &trigger["reason"]),
        (&json!("medium"), &json!("compare"))
    );
    let route_seq = route.seq;
    steps.push(route);

    let model = b.on_event(&exchange(0, &[]), 5).unwrap();
    assert_eq!(model.links.caused_by, vec![route_seq]);
    steps.push(model);
    steps.push(b.turn_end(&AgentLoopResult::default(), 6));
    let nudge = b.follow_up("Run the tests before you finish.", 7);
    assert_eq!((nudge.turn, nudge.actor), (1, gate));
    assert!(matches!(nudge.action, StepAction::UserPrompt { .. }));
    steps.push(nudge);
    steps.extend(b.on_record(&gate_row(
        8,
        Some(gate_call("claude-opus-5-5", "judge", Some(usage))),
        None,
    )));
    assert_eq!(b.turn(), 1);

    let card = RunScorecard::compute(&RunMeta::new("m", crate::RunSource::Test), &steps, &[]);
    assert_eq!(card.turns, 1, "a follow-up is part of the turn");
    assert_eq!(card.model_calls, 4);
    let priced = crate::scorecard::step_cost_usd("claude-haiku-5-5", usage).unwrap()
        + crate::scorecard::step_cost_usd("claude-opus-5-5", usage).unwrap();
    assert!((card.cost_usd - priced).abs() < 1e-12, "{}", card.cost_usd);
    assert_eq!(
        card.unpriced_calls, 1,
        "the agent's own call is to model `m`"
    );
    assert_eq!(card.decisions.get("gate/route"), Some(&1));
}

#[test]
fn builder_links_tools_to_model_and_decisions_to_next_model() {
    let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
    let prompt = b.user_prompt("fix it", 1);
    assert_eq!(prompt.turn, 1);
    let nudge = b
        .on_event(
            &AgentLoopEvent::Debug(DebugEvent::Decision {
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
            }),
            2,
        )
        .unwrap();
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
        .on_event(
            &AgentLoopEvent::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "read_file".into(),
                content: "fn main() {}".into(),
                is_error: false,
                image: None,
            },
            9,
        )
        .unwrap();
    assert_eq!(result.parent, Some(model.seq));
    assert_eq!(result.duration_ms, Some(5));
    assert_eq!(result.links.files, vec!["a.rs".to_string()]);
    let end = b.turn_end(&AgentLoopResult::default(), 10);
    assert!(matches!(end.action, StepAction::TurnEnd { .. }));
    assert_eq!(end.seq, 5);
}

#[test]
fn approval_waits_become_policy_steps() {
    let row = |tx_type: &str, at_ms: u64, id: &str| RecordRow {
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
    };
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

#[test]
fn recorder_writes_bundle_and_summary() {
    let home = tempfile::tempdir().unwrap();
    let meta = RunMeta::new("m", crate::RunSource::Test);
    let rec = RunRecorder::start(home.path(), meta, TraceLevel::Full)
        .unwrap()
        .unwrap();
    assert!(rec.blob_store().is_some());
    let seen = Arc::new(AtomicU64::new(0));
    let seen2 = seen.clone();
    rec.subscribe(move |_| {
        seen2.fetch_add(1, Ordering::Relaxed);
    });
    rec.begin_turn("hello\nworld");
    rec.on_event(&exchange(0, &[]));
    rec.end_turn(&AgentLoopResult::default());
    let card = rec.finish().unwrap();
    assert_eq!(card.model_calls, 1);
    assert_eq!(seen.load(Ordering::Relaxed), 3);
    let meta = rec.bundle().read_meta().unwrap();
    assert_eq!(meta.title, "hello");
    assert!(meta.ended_at_ms.is_some());
    assert_eq!(rec.bundle().read_steps().unwrap().len(), 3);
}

#[test]
fn off_level_records_nothing() {
    let home = tempfile::tempdir().unwrap();
    let meta = RunMeta::new("m", crate::RunSource::Test);
    assert!(RunRecorder::start(home.path(), meta, TraceLevel::Off)
        .unwrap()
        .is_none());
    assert!(bundle::list_run_dirs(home.path()).is_empty());
}
