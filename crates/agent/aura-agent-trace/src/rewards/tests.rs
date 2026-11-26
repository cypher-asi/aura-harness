//! Tests for reward computation.

use super::*;
use aura_core_types::trace::{
    FileTouch, HarnessAction, ModelAction, Observation, PolicyRef, StepLinks, TokenUsage,
    ToolCallAction,
};
use serde_json::json;

pub(crate) fn base(seq: u64, actor: Actor, action: StepAction) -> Step {
    Step {
        run_id: "r".into(),
        seq,
        parent: None,
        turn: 1,
        iteration: None,
        at_ms: seq * 1000,
        actor,
        observation: Observation::None,
        action,
        policy: PolicyRef::default(),
        links: StepLinks::default(),
        usage: None,
        duration_ms: None,
    }
}

pub(crate) fn model(seq: u64, calls: &[(&str, &str, serde_json::Value)]) -> Step {
    let mut s = base(
        seq,
        Actor::Model,
        StepAction::Model(ModelAction {
            tool_calls: calls
                .iter()
                .map(|(id, n, i)| ToolCallAction {
                    tool_use_id: (*id).into(),
                    name: (*n).into(),
                    input: i.clone(),
                    intent: ToolIntent::classify(n, i),
                })
                .collect(),
            ..ModelAction::default()
        }),
    );
    s.usage = Some(TokenUsage {
        input: 1000,
        output: 100,
        cache_write: 0,
        cache_read: 0,
    });
    s
}

pub(crate) fn result(
    seq: u64,
    parent: u64,
    id: &str,
    tool: &str,
    path: &str,
    kind: &str,
    err: bool,
) -> Step {
    let mut s = base(
        seq,
        Actor::Environment,
        StepAction::ToolResult {
            tool_use_id: id.into(),
            tool: tool.into(),
            is_error: err,
            source: "executor".into(),
            content_chars: 1,
            excerpt: String::new(),
            content: None,
            files: vec![FileTouch {
                path: path.into(),
                kind: kind.into(),
                added: 1,
                removed: 0,
            }],
        },
    );
    s.parent = Some(parent);
    s
}

pub(crate) fn nudge(seq: u64, name: &str) -> Step {
    base(
        seq,
        Actor::Harness {
            source: DecisionSource::Steering,
        },
        StepAction::Harness {
            name: name.into(),
            action: HarnessAction::Nudge { kind: name.into() },
            alternatives: vec![],
        },
    )
}

#[test]
fn redundant_read_and_edit_rewards() {
    let steps = vec![
        model(1, &[("a", "read_file", json!({"path": "x"}))]),
        result(2, 1, "a", "read_file", "x", "read", false),
        model(3, &[("b", "read_file", json!({"path": "x"}))]),
        result(4, 3, "b", "read_file", "x", "read", false),
        model(5, &[("c", "edit_file", json!({"path": "x"}))]),
        result(6, 5, "c", "edit_file", "x", "modify", false),
        model(7, &[("d", "read_file", json!({"path": "x"}))]),
        result(8, 7, "d", "read_file", "x", "read", false),
    ];
    let names: Vec<String> = process_rewards(&steps)
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, vec!["redundant_read", "edit_applied"]);
}

#[test]
fn implement_now_effect_is_positive_when_an_edit_follows() {
    let steps = vec![
        nudge(1, "implement_now"),
        model(2, &[("a", "read_file", json!({"path": "x"}))]),
        model(3, &[("b", "edit_file", json!({"path": "x"}))]),
    ];
    let effects = decision_effects(&steps);
    assert_eq!(effects.len(), 1);
    assert!((effects[0].value - 1.0).abs() < f64::EPSILON);
    let ignored = decision_effects(&[nudge(1, "implement_now"), model(2, &[])]);
    assert!((ignored[0].value + 1.0).abs() < f64::EPSILON);
}

#[test]
fn cost_is_priced_per_call_and_tokens_counted_apart() {
    let mut opus = model(1, &[]);
    opus.policy.id = "claude-opus-5-5".into();
    let mut local = model(2, &[]);
    local.policy.id = "local/qwen3:4b".into();
    let usd = step_cost_usd("claude-opus-5-5", opus.usage.unwrap()).unwrap();
    let rewards = cost_rewards(&[opus, local]);
    let values = |kind: RewardKind, name: &str| -> Vec<f64> {
        rewards
            .iter()
            .filter(|r| r.kind == kind && r.name == name)
            .map(|r| r.value)
            .collect()
    };
    assert_eq!(values(RewardKind::Tokens, "tokens"), vec![-0.011, -0.011]);
    assert_eq!(
        values(RewardKind::Cost, "usd"),
        vec![-usd],
        "a model with no known price has no usd reward"
    );
    assert_eq!(values(RewardKind::Cost, "wall_minutes").len(), 1);
}

#[test]
fn returns_discount_backwards() {
    let steps = vec![model(1, &[]), model(2, &[])];
    let rewards = vec![reward(
        RewardTarget::Run,
        RewardKind::Outcome,
        "pass",
        1.0,
        "t",
        0,
    )];
    let r = returns(&steps, &rewards, 0.5);
    assert!((r[1].2 - 1.0).abs() < 1e-9);
    assert!((r[0].2 - 0.5).abs() < 1e-9);
}

#[test]
fn outcome_credit_goes_to_editing_model_steps() {
    let mut end = base(
        9,
        Actor::Environment,
        StepAction::TurnEnd {
            stop_reason: "end_turn".into(),
            iterations: 1,
            input_tokens: 0,
            output_tokens: 0,
            files_changed: 1,
            error: None,
        },
    );
    end.links.files = vec!["x".into()];
    let steps = vec![
        model(1, &[("c", "edit_file", json!({"path": "x"}))]),
        result(2, 1, "c", "edit_file", "x", "modify", false),
        end,
    ];
    let credit = attribute_outcome(&steps, 1.0, 0);
    assert_eq!(credit.len(), 1);
    assert_eq!(credit[0].target, RewardTarget::Step { seq: 1 });
}
