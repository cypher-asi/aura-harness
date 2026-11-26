use super::support::payload;
use crate::ids::AgentId;
use crate::types::compare::{
    ArmSpec, BenchSource, CompareArm, CompareJudgment, ComparePayload, CompareStatus, RunTurn,
};
use crate::types::gate::OutcomeSignal;

#[test]
fn an_unpriced_judgment_round_trips_and_older_ones_load_as_priced() {
    let mut unpriced = payload();
    unpriced.judgments[0].unpriced_calls = 1;
    let json: serde_json::Value = serde_json::from_slice(&unpriced.to_bytes()).unwrap();
    assert_eq!(json["judgments"][0]["unpriced_calls"], 1);
    assert_eq!(
        ComparePayload::from_bytes(&unpriced.to_bytes()),
        Some(unpriced)
    );

    let mut older = serde_json::to_value(&payload().judgments[0]).unwrap();
    assert!(older.get("unpriced_calls").is_none(), "0 is not written");
    older.as_object_mut().unwrap().remove("unpriced_calls");
    let older: CompareJudgment = serde_json::from_value(older).unwrap();
    assert_eq!(older.unpriced_calls, 0);
}

#[test]
fn compare_payload_round_trips_as_a_compare_system_payload() {
    let payload = payload();
    let bytes = payload.to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["system_kind"], "compare");
    assert_eq!(json["status"]["state"], "applied");
    assert_eq!(json["status"]["detail"], "a-1");
    assert_eq!(json["runs"][0]["arm"], "a");
    assert_eq!(json["arm_a"]["kind"], "gate");
    assert_eq!(json["arm_b"]["model"], "claude/claude-opus-4-7");
    assert_eq!(json["runs"][0]["turns"][0]["signal"], "checks_passed");
    assert_eq!(json["runs"][0]["turns"][0]["category"], "debugging");
    assert_eq!(json["runs"][0]["turns"][0]["escalations"], 1);
    assert_eq!(
        json["runs"][0]["turns"][0]["first_model"],
        "claude/claude-haiku-4-5"
    );
    assert_eq!(json["runs"][0]["turns"][0]["effort"], "max");
    assert_eq!(json["runs"][0]["turns"][0]["first_effort"], "high");
    assert_eq!(json["runs"][0]["turns"][0]["nudges"], 1);
    assert_eq!(json["runs"][0]["turns"][0]["sent_back"], 1);
    assert!(json["runs"][0].get("error").is_none());
    assert!(json["judgments"][0].get("a_vs_reference").is_none());
    assert_eq!(json["runs"][0]["plan_usage"][0]["plan"], "claude");
    assert_eq!(json["classifier"], "claude/claude-haiku-4-5");
    assert_eq!(ComparePayload::from_bytes(&bytes), Some(payload));
}

#[test]
fn records_name_no_classifier_when_older_or_empty() {
    let payload = payload();
    let mut unnamed = serde_json::to_value(&payload).unwrap();
    unnamed.as_object_mut().unwrap().remove("classifier");
    let unnamed = ComparePayload::from_bytes(&serde_json::to_vec(&unnamed).unwrap()).unwrap();
    assert_eq!(unnamed.classifier, "", "older records name no classifier");
    let no_classifier = ComparePayload {
        classifier: String::new(),
        ..payload
    };
    let json: serde_json::Value = serde_json::from_slice(&no_classifier.to_bytes()).unwrap();
    assert!(
        json.get("classifier").is_none(),
        "an empty one is not written"
    );
}

#[test]
fn runs_and_turns_from_older_records_load() {
    let mut without = serde_json::to_value(payload()).unwrap();
    let run = without["runs"][0].as_object_mut().unwrap();
    run.remove("plan_usage");
    let turn = run["turns"][0].as_object_mut().unwrap();
    for field in [
        "category",
        "difficulty",
        "led",
        "escalations",
        "first_model",
        "effort",
        "first_effort",
        "nudges",
        "sent_back",
        "subtask_cost_usd",
    ] {
        turn.remove(field);
    }
    let older = ComparePayload::from_bytes(&serde_json::to_vec(&without).unwrap()).unwrap();
    assert!(
        older.runs[0].plan_usage.is_empty(),
        "older records still load"
    );
    assert_eq!(
        older.runs[0].turns,
        [RunTurn::new(
            "claude/claude-sonnet-4-6",
            0.12,
            42_000,
            OutcomeSignal::ChecksPassed
        )]
    );
    without["runs"][0].as_object_mut().unwrap().remove("turns");
    let oldest = ComparePayload::from_bytes(&serde_json::to_vec(&without).unwrap()).unwrap();
    assert!(oldest.runs[0].turns.is_empty());
}

#[test]
fn records_from_before_free_arms_read_as_gate_against_the_other() {
    let old = serde_json::json!({
        "system_kind": "compare",
        "task": "fix the parser",
        "against": "gate:nodelegate",
        "repeats": 1,
        "requested_by": AgentId::generate(),
        "apply_root": "/work",
        "started_ms": 1,
        "runs": [{
            "name": "gate-1",
            "arm": "gate",
            "agent_id": AgentId::generate(),
            "model": "m",
            "copy": {"root": "/c", "workdir": "/c", "baseline": "t"},
            "wall_ms": 1,
            "cost_usd": 0.1,
            "tokens": [],
            "check_passed": null,
            "files_changed": 0
        }],
        "judgments": [{
            "gate_run": "gate-1",
            "other_run": "other-1",
            "gate_first": true,
            "winner": "other",
            "parsed": true,
            "model": "judge",
            "cost_usd": 0.01,
            "reasoning": ""
        }],
        "status": {"state": "picked", "detail": "gate"}
    });
    let payload = ComparePayload::from_bytes(&serde_json::to_vec(&old).unwrap()).unwrap();
    assert_eq!(payload.arm_a, ArmSpec::Gate { delegate: true });
    assert_eq!(payload.arm_b, ArmSpec::Gate { delegate: false });
    assert_eq!(payload.prompts, ["fix the parser"]);
    assert_eq!(payload.source, BenchSource::Prompt);
    assert_eq!(payload.runs[0].arm, CompareArm::A);
    let judgment = &payload.judgments[0];
    assert_eq!(
        (judgment.a_run.as_str(), judgment.b_run.as_str()),
        ("gate-1", "other-1")
    );
    assert!(judgment.a_first);
    assert_eq!(judgment.winner, Some(CompareArm::B));
    assert_eq!(
        payload.status,
        CompareStatus::Done,
        "a comparison the user once picked an arm of is done"
    );
}

#[test]
fn other_system_payloads_are_not_comparisons() {
    let rewind = serde_json::json!({"system_kind": "rewind", "turn": 1});
    assert_eq!(
        ComparePayload::from_bytes(&serde_json::to_vec(&rewind).unwrap()),
        None
    );
    let mut wrong_kind = serde_json::to_value(payload()).unwrap();
    wrong_kind["system_kind"] = "rewind".into();
    assert_eq!(
        ComparePayload::from_bytes(&serde_json::to_vec(&wrong_kind).unwrap()),
        None
    );
}

#[test]
fn copies_exist_until_applied_or_dismissed() {
    assert!(CompareStatus::Done.has_copies());
    assert!(!CompareStatus::Applied("a-1".to_string()).has_copies());
    assert!(!CompareStatus::Dismissed.has_copies());
    assert_eq!(CompareArm::A.opposite(), CompareArm::B);
}

#[test]
fn runs_are_found_by_name_and_arm() {
    let payload = payload();
    assert_eq!(payload.run("a-1").map(|r| r.wall_ms), Some(42_000));
    assert!(payload.run("b-1").is_none());
    assert_eq!(payload.arm_runs(CompareArm::A).count(), 1);
    assert_eq!(payload.arm_runs(CompareArm::B).count(), 0);
    assert_eq!(
        payload.spec(CompareArm::B).label(),
        "claude/claude-opus-4-7"
    );
}
