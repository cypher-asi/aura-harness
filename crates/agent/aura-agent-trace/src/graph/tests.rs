
use super::*;
use crate::bundle::RunSource;
use crate::rewards::tests::{base, model, nudge, result};
use crate::RunTrace;
use serde_json::json;

fn trace() -> RunTrace {
    let prompt = base(
        1,
        Actor::User,
        StepAction::UserPrompt {
            text: "fix the test".into(),
        },
    );
    let mut m2 = model(3, &[("a", "edit_file", json!({"path": "x"}))]);
    m2.links.caused_by = vec![2];
    let steps = vec![
        prompt,
        nudge(2, "implement_now"),
        m2,
        result(4, 3, "a", "edit_file", "x", "modify", false),
        model(5, &[("b", "read_file", json!({"path": "x"}))]),
        result(6, 5, "b", "read_file", "x", "read", false),
    ];
    RunTrace::from_parts(RunMeta::new("m", RunSource::Test), steps, Vec::new())
}

#[test]
fn builds_containment_and_cross_edges() {
    let g = trace().graph();
    let turn = g.node("t1").unwrap();
    assert_eq!(turn.children, vec!["s2", "s3", "s5"]);
    assert_eq!(g.node("s3").unwrap().children, vec!["tc:a"]);
    assert_eq!(g.node("tc:a").unwrap().status, NodeStatus::Ok);
    let links: Vec<(&str, &str, EdgeKind)> = g
        .links("tc:a")
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str(), e.kind))
        .collect();
    assert!(links.contains(&("tc:a", "tc:b", EdgeKind::Touched)));
    assert!(g
        .links("s2")
        .iter()
        .any(|e| e.to == "s3" && e.kind == EdgeKind::Triggered));
    assert!(g.links("s3").iter().any(|e| e.kind == EdgeKind::Next));
    assert!(g.node("s2").unwrap().detail.contains("effect +"));
}

#[test]
fn incremental_apply_matches_full_build() {
    let t = trace();
    let mut live = RunGraph::new(&t.meta);
    for s in &t.steps {
        live.apply(s);
    }
    let full = t.graph();
    let shape = |g: &RunGraph| -> Vec<(String, Option<String>)> {
        g.nodes
            .iter()
            .filter(|n| n.kind != NodeKind::Reward)
            .map(|n| (n.id.clone(), n.parent.clone()))
            .collect()
    };
    assert_eq!(shape(&live), shape(&full));
    assert_eq!(live.nodes.len(), 7);
}

#[test]
fn changed_since_returns_new_nodes_and_their_ancestors() {
    let t = trace();
    let mut g = RunGraph::new(&t.meta);
    for s in &t.steps[..3] {
        g.apply(s);
    }
    let before = g.nodes.len();
    g.apply(&t.steps[3]);
    let ids: Vec<&str> = g
        .changed_since(before, t.steps[3].seq)
        .iter()
        .map(|n| n.id.as_str())
        .collect();
    assert_eq!(ids, vec!["run", "t1", "s3", "tc:a"]);
}

fn turn_end(seq: u64, stop_reason: &str, error: Option<&str>) -> Step {
    base(
        seq,
        Actor::Environment,
        StepAction::TurnEnd {
            stop_reason: stop_reason.into(),
            iterations: 1,
            input_tokens: 0,
            output_tokens: 0,
            files_changed: 0,
            error: error.map(str::to_string),
        },
    )
}

fn policy(seq: u64, id: &str, verdict: &str) -> RecordRow {
    RecordRow {
        seq,
        tx_type: "policy_decision".into(),
        at_ms: seq * 1000,
        tool_use_id: Some(id.into()),
        tool: Some("edit_file".into()),
        verdict: Some(verdict.into()),
        approver: Some("user".into()),
        rule: Some("mode.ask".into()),
        reason: None,
        call: None,
        route: None,
    }
}

#[test]
fn model_calls_are_timed_priced_and_flag_max_tokens() {
    let mut m = model(2, &[]);
    m.policy.id = "claude-opus-4-5".into();
    m.usage = Some(aura_core_types::trace::TokenUsage {
        input: 200,
        output: 100,
        cache_write: 0,
        cache_read: 800,
    });
    if let StepAction::Model(action) = &mut m.action {
        action.stop_reason = "max_tokens".into();
    }
    let mut g = RunGraph::new(&RunMeta::new("m", RunSource::Test));
    g.apply(&m);
    g.apply(&turn_end(3, "end_turn", None));
    let call = g.node("s2").unwrap();
    assert_eq!(call.status, NodeStatus::Warn);
    assert!(call.detail.starts_with("1.0k/100"), "{}", call.detail);
    assert!(call.detail.ends_with("max tokens"));
    assert_eq!(call.meta["at_ms"], json!(2000));
    assert_eq!(call.meta["prompt_tokens"], json!(1000));
    let cost = call.meta["cost_usd"].as_f64().unwrap();
    assert!((cost - 0.0039).abs() < 1e-9);
    let turn = g.node("t1").unwrap();
    assert_eq!(turn.meta["cost_usd"], json!(cost));
    assert_eq!(turn.meta["duration_ms"], json!(1000));
    assert_eq!(g.node("run").unwrap().meta["cost_usd"], json!(cost));
}

#[test]
fn a_turn_that_ended_on_an_error_shows_the_failed_call() {
    let mut g = RunGraph::new(&RunMeta::new("m", RunSource::Test));
    g.apply(&model(1, &[]));
    g.apply(&turn_end(2, "llm_error", Some("overloaded\nretry later")));
    let failed = g.node("s2:failed").unwrap();
    assert_eq!(failed.label, "I2 failed");
    assert_eq!(failed.detail, "overloaded");
    assert_eq!(failed.status, NodeStatus::Error);
    assert!(g.links("s1").iter().any(|e| e.to == "s2:failed"));
    assert_eq!(
        g.node("t1").unwrap().meta["error"],
        json!("overloaded\nretry later")
    );
}

#[test]
fn policy_decisions_land_on_their_tool_call_in_either_order() {
    let mut g = RunGraph::new(&RunMeta::new("m", RunSource::Test));
    assert_eq!(g.apply_record(&policy(1, "early", "denied")), None);
    g.apply(&model(2, &[("early", "edit_file", json!({"path": "x"}))]));
    let early = g.node("tc:early").unwrap();
    assert_eq!(early.status, NodeStatus::Error);
    assert_eq!(early.meta["policy"]["verdict"], json!("denied"));

    g.apply(&model(3, &[("late", "edit_file", json!({"path": "y"}))]));
    let changed = g.apply_record(&policy(4, "late", "pending_approval"));
    assert_eq!(changed.as_deref(), Some("tc:late"));
    assert_eq!(g.node("tc:late").unwrap().detail, "awaiting approval");
    g.apply_record(&policy(5, "late", "approved"));
    let late = g.node("tc:late").unwrap();
    assert_eq!(late.detail, "running");
    assert_eq!(late.meta["policy"]["approver"], json!("user"));
    assert_eq!(late.meta["policy"]["rule"], json!("mode.ask"));
    assert_eq!(late.meta["policy"]["record_seq"], json!(5));
}

#[test]
fn child_runs_link_to_the_call_that_spawned_them() {
    let parent = RunMeta::new("m", RunSource::Test);
    let mut g = RunGraph::new(&parent);
    g.apply(&model(
        1,
        &[
            ("a", "task", json!({"prompt": "count files\nin src"})),
            ("b", "task", json!({"prompt": "list tests"})),
        ],
    ));
    assert_eq!(g.node("tc:a").unwrap().kind, NodeKind::Subagent);
    let child = |id: &str, title: &str, at: u64| {
        let mut meta = RunMeta::new("m", RunSource::Test);
        meta.run_id = id.into();
        meta.title = title.into();
        meta.started_at_ms = at;
        meta.spawned_by = Some(parent.run_id.clone());
        meta
    };
    let changed = g.link_child_runs(&[
        child("c2", "list tests", 1500),
        child("c1", "count files", 1200),
        child("c3", "list tests, retried", 1800),
    ]);
    assert_eq!(changed, vec!["run", "tc:a", "tc:b"]);
    assert_eq!(g.node("tc:a").unwrap().meta["child_run"], json!("c1"));
    assert_eq!(g.node("tc:b").unwrap().meta["child_run"], json!("c2"));
    assert_eq!(
        g.node("tc:b").unwrap().meta["child_runs"],
        json!(["c2", "c3"])
    );
    assert_eq!(
        g.node("run").unwrap().meta["child_runs"],
        json!(["c1", "c2", "c3"])
    );
    assert!(g
        .link_child_runs(&[child("c1", "count files", 1200)])
        .is_empty());
}

#[test]
fn the_run_node_shows_unset_fields_as_null() {
    let g = RunGraph::new(&RunMeta::new("m", RunSource::Test));
    let run = &g.node("run").unwrap().meta;
    assert_eq!(run["ended_at_ms"], Value::Null);
    assert_eq!(run["spawned_by"], Value::Null);
    assert!(run["started_at_ms"].as_u64().is_some());
}
