use super::support::payload;
use crate::ids::AgentId;
use crate::types::compare::{
    BenchSource, CompareArm, ComparePayload, ExampleTaskInfo, RefVerdict, Reference,
    SessionBaseline,
};

#[test]
fn a_replayed_session_keeps_its_baseline_and_reference() {
    let mut payload = payload();
    payload.prompts = vec!["one".to_string(), "two".to_string()];
    payload.source = BenchSource::Session {
        agent_id: AgentId::generate(),
        started_ms: 7,
        title: "one".to_string(),
        baseline: SessionBaseline {
            calls: 4,
            cost_usd: 0.8,
            priced: true,
            unpriced_calls: 1,
            models: vec!["opus".to_string()],
        },
        reference: Some(Reference {
            diff: "+x".to_string(),
            reply: "done".to_string(),
        }),
    };
    payload.judgments[0].a_vs_reference = Some(RefVerdict::AsGood);
    payload.judgments[0].b_vs_reference = Some(RefVerdict::Worse);
    let back = ComparePayload::from_bytes(&payload.to_bytes()).unwrap();
    assert_eq!(back, payload);
    assert_eq!(back.source.reference().unwrap().reply, "done");
    let baseline = back.source.baseline().unwrap();
    assert!((baseline.cost_usd - 0.8).abs() < 1e-9);
    assert_eq!(baseline.unpriced_calls, 1);
    assert_eq!(baseline.known_cost_usd(), None, "partly priced");
    assert_eq!(
        back.judgments[0].vs_reference(CompareArm::B),
        Some(RefVerdict::Worse)
    );
}

#[test]
fn a_baseline_saved_before_unpriced_calls_were_kept_loads() {
    let old = r#"{"calls":3,"cost_usd":0.5,"priced":true,"models":["opus"]}"#;
    let baseline: SessionBaseline = serde_json::from_str(old).unwrap();
    assert_eq!(baseline.unpriced_calls, 0);
    assert_eq!(baseline.known_cost_usd(), Some(0.5));
    let unpriced = SessionBaseline {
        priced: false,
        ..baseline
    };
    assert_eq!(unpriced.known_cost_usd(), None);
}

#[test]
fn example_tasks_take_turns_by_run_number_and_keep_no_prompts() {
    let mut payload = payload();
    payload.prompts = Vec::new();
    let task = |id: &str| ExampleTaskInfo {
        id: id.to_string(),
        title: id.to_string(),
        prompts: vec![format!("do {id}")],
        check: "true".to_string(),
    };
    payload.source = BenchSource::Examples {
        tasks: vec![task("coding/one"), task("coding/two")],
    };
    payload.runs[0].task = Some("coding/one".to_string());
    let back = ComparePayload::from_bytes(&payload.to_bytes()).unwrap();
    assert_eq!(back, payload);
    assert!(back.prompts.is_empty());
    let id = |n| back.source.example_for(n).map(|t| t.id.as_str());
    assert_eq!(
        (id(1), id(2), id(3)),
        (Some("coding/one"), Some("coding/two"), Some("coding/one"))
    );
    assert_eq!(BenchSource::Prompt.example_for(1), None);
}
