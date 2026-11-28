use super::*;
use aura_agent::DebugEvent;
use aura_config::HarnessParams;
use aura_core_types::trace::{
    Actor, DecisionSource, HarnessAction, HarnessDecision, ModelAction, ModelExchange, Observation,
    StepAction, TokenUsage, ToolCallAction, ToolIntent,
};
use serde_json::json;

use crate::records::{CallRow, RouteRow};

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

fn gate_usage() -> TokenUsage {
    TokenUsage {
        input: 1_000,
        output: 100,
        ..TokenUsage::default()
    }
}

fn gate_actor() -> Actor {
    Actor::Harness {
        source: DecisionSource::Gate,
    }
}

fn sonnet_route() -> RouteRow {
    RouteRow {
        category: "small_change".into(),
        difficulty: Some("medium".into()),
        model: "claude/claude-sonnet-5-5".into(),
        effort: Some("medium".into()),
        reason: "compare".into(),
        explored: false,
    }
}

/// One turn with every kind of Gate call, a route, a model step and a
/// follow-up, in the order they happen.
struct GateTurn {
    b: StepBuilder,
    prompt: Step,
    classify: Step,
    failed: Step,
    title: Option<Step>,
    route: Step,
    model: Step,
    end: Step,
    nudge: Step,
    judge: Option<Step>,
}

impl GateTurn {
    fn run() -> Self {
        let usage = gate_usage();
        let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
        let prompt = b.user_prompt("fix the parser", 0);
        let mut call = |seq, model, purpose, usage| {
            b.on_record(&gate_row(seq, Some(gate_call(model, purpose, usage)), None))
        };
        let classify = call(1, "claude-haiku-5-5", "classify", Some(usage)).unwrap();
        let failed = call(2, "claude-sonnet-5-5", "criteria", None).unwrap();
        let title = call(3, "claude-haiku-5-5", "title", Some(usage));
        let route = b
            .on_record(&gate_row(4, None, Some(sonnet_route())))
            .unwrap();
        let model = b.on_event(&exchange(0, &[]), 5).unwrap();
        let end = b.turn_end(&AgentLoopResult::default(), 6);
        let nudge = b.follow_up("Run the tests before you finish.", 7);
        let judge = b.on_record(&gate_row(
            8,
            Some(gate_call("claude-opus-5-5", "judge", Some(usage))),
            None,
        ));
        Self {
            b,
            prompt,
            classify,
            failed,
            title,
            route,
            model,
            end,
            nudge,
            judge,
        }
    }
}

#[test]
fn gate_classify_call_is_a_model_step_of_the_turn() {
    let classify = GateTurn::run().classify;
    assert_eq!((classify.turn, classify.actor), (1, gate_actor()));
    assert_eq!(classify.policy.id, "claude-haiku-5-5");
    assert_eq!(
        (classify.usage, classify.duration_ms),
        (Some(gate_usage()), Some(900))
    );
    assert_eq!(classify.links.record_seqs, vec![1]);
    let StepAction::Model(action) = &classify.action else {
        panic!("expected a model step, got {:?}", classify.action);
    };
    assert_eq!(action.purpose.as_deref(), Some("classify"));
}

#[test]
fn failed_gate_call_keeps_its_error() {
    let failed = GateTurn::run().failed;
    let StepAction::Model(action) = &failed.action else {
        panic!("expected a model step, got {:?}", failed.action);
    };
    assert_eq!(
        action.error.as_deref(),
        Some("tool_choice is not supported")
    );
    assert_eq!(failed.usage, None);
}

#[test]
fn calls_gate_did_not_make_are_not_steps() {
    assert!(
        GateTurn::run().title.is_none(),
        "naming the chat is not Gate's"
    );
}

#[test]
fn gate_route_is_a_harness_decision() {
    let route = GateTurn::run().route;
    let StepAction::Harness { name, action, .. } = &route.action else {
        panic!("expected a harness step, got {:?}", route.action);
    };
    assert_eq!((name.as_str(), route.actor), ("route", gate_actor()));
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
}

#[test]
fn model_step_after_a_route_is_caused_by_it() {
    let turn = GateTurn::run();
    assert_eq!(turn.model.links.caused_by, vec![turn.route.seq]);
}

#[test]
fn follow_ups_and_judging_stay_in_the_turn() {
    let turn = GateTurn::run();
    assert_eq!((turn.nudge.turn, turn.nudge.actor), (1, gate_actor()));
    assert!(matches!(turn.nudge.action, StepAction::UserPrompt { .. }));
    assert_eq!(turn.b.turn(), 1);
}

#[test]
fn gate_turn_scorecard_counts_gate_calls_and_follow_ups() {
    let usage = gate_usage();
    let turn = GateTurn::run();
    let mut steps = vec![
        turn.prompt,
        turn.classify,
        turn.failed,
        turn.route,
        turn.model,
        turn.end,
        turn.nudge,
    ];
    steps.extend(turn.judge);
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

fn one_turn() -> (tempfile::TempDir, Arc<RunRecorder>) {
    let home = tempfile::tempdir().unwrap();
    let meta = RunMeta::new("m", crate::RunSource::Test);
    let rec = RunRecorder::start(home.path(), meta, TraceLevel::Decisions)
        .unwrap()
        .unwrap();
    rec.begin_turn("hello");
    rec.on_event(&exchange(0, &[]));
    rec.end_turn(&AgentLoopResult::default());
    (home, rec)
}

#[test]
fn a_finish_left_for_later_is_written_once_with_the_end_it_noted() {
    let (_home, rec) = one_turn();
    let noted = now_ms();
    assert!(rec.finish_later(), "the first note asks for a write");
    assert!(!rec.finish_later(), "a second one shares it");
    assert!(
        rec.bundle().read_summary().is_none(),
        "nothing is written yet"
    );

    std::thread::sleep(std::time::Duration::from_millis(5));
    let written = now_ms();
    rec.finish_due();
    assert_eq!(rec.bundle().read_summary().unwrap().model_calls, 1);
    assert!(rec.dir().join(crate::run_log::LOG_FILE).exists());
    let ended = rec.bundle().read_meta().unwrap().ended_at_ms.unwrap();
    assert!((noted..written).contains(&ended), "stamped when noted");

    std::fs::remove_file(rec.bundle().path(bundle::SUMMARY_FILE)).unwrap();
    rec.finish_due();
    assert!(rec.bundle().read_summary().is_none(), "nothing was due");
    assert!(rec.finish_later(), "a note after the write asks again");
}

#[test]
fn finish_writes_what_was_left_for_later() {
    let (_home, rec) = one_turn();
    assert!(rec.finish_later());
    rec.finish();
    std::fs::remove_file(rec.bundle().path(bundle::SUMMARY_FILE)).unwrap();
    rec.finish_due();
    assert!(
        rec.bundle().read_summary().is_none(),
        "nothing was left due"
    );
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
