use super::*;
use serde_json::{json, Value};

#[test]
fn harness_decision_roundtrips() {
    let decision = HarnessDecision::new(
        DecisionSource::Steering,
        "implement_now",
        HarnessAction::Nudge {
            kind: "implement_now".into(),
        },
    )
    .at_iteration(4)
    .with_features(json!({"exploration_count": 11}))
    .with_alternatives(&["allow"]);
    let text = serde_json::to_string(&decision).unwrap();
    let back: HarnessDecision = serde_json::from_str(&text).unwrap();
    assert_eq!(decision, back);
    assert!(text.contains("\"type\":\"nudge\""));
}

#[test]
fn step_roundtrips_with_nested_tags() {
    let step = Step {
        run_id: "r1".into(),
        seq: 3,
        parent: Some(2),
        turn: 1,
        iteration: Some(0),
        at_ms: 10,
        actor: Actor::Harness {
            source: DecisionSource::Cache,
        },
        observation: Observation::Harness {
            features: json!({"hits": 1}),
            trigger: Value::Null,
        },
        action: StepAction::Harness {
            name: "tool_result_cache".into(),
            action: HarnessAction::CacheHit {
                tool: "read_file".into(),
                tool_use_id: "t1".into(),
            },
            alternatives: vec![],
        },
        policy: PolicyRef {
            kind: PolicyKind::Harness,
            id: "tool_result_cache".into(),
            version: "1".into(),
            params: Value::Null,
            propensity: Some(0.5),
        },
        links: StepLinks::default(),
        usage: None,
        duration_ms: None,
    };
    let text = serde_json::to_string(&step).unwrap();
    let back: Step = serde_json::from_str(&text).unwrap();
    assert_eq!(step, back);
}

#[test]
fn reward_roundtrips() {
    let reward = Reward {
        target: RewardTarget::Step { seq: 7 },
        kind: RewardKind::Process,
        name: "edit_applied".into(),
        value: 0.2,
        source: "process".into(),
        at_ms: 1,
        evidence: Value::Null,
    };
    let back: Reward = serde_json::from_str(&serde_json::to_string(&reward).unwrap()).unwrap();
    assert_eq!(reward, back);
}

#[test]
fn classifies_tool_intents() {
    assert_eq!(
        ToolIntent::classify("read_file", &json!({})),
        ToolIntent::Explore
    );
    assert_eq!(
        ToolIntent::classify("edit_file", &json!({})),
        ToolIntent::Edit
    );
    assert_eq!(
        ToolIntent::classify("run_command", &json!({"command": "cargo test -p x"})),
        ToolIntent::Verify
    );
    assert_eq!(
        ToolIntent::classify("run_command", &json!({"command": "ls -la"})),
        ToolIntent::Other
    );
    assert_eq!(
        ToolIntent::classify("task", &json!({})),
        ToolIntent::Delegate
    );
    assert_eq!(
        ToolIntent::classify("task_done", &json!({})),
        ToolIntent::Complete
    );
}

#[test]
fn blob_id_is_stable_hex() {
    let id = blob_id(b"hello");
    assert_eq!(id.len(), 64);
    assert_eq!(id, blob_id(b"hello"));
    assert_ne!(id, blob_id(b"hello!"));
}
