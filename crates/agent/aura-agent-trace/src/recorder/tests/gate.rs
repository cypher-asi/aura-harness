//! Gate's calls and routes as steps of the turn.

use super::*;

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
