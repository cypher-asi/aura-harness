use super::*;

/// Call sites that emit each source. The match is exhaustive, so a
/// new [`DecisionSource`] does not compile until it names a site.
fn emit_sites(source: DecisionSource) -> &'static [&'static str] {
    match source {
        DecisionSource::Steering => &["decisions::steering_nudge(", "decisions::turn_nudge("],
        DecisionSource::Compaction => &["decisions::compaction("],
        DecisionSource::Budget => &["decisions::budget_warning(", "decisions::budget_stop("],
        DecisionSource::ToolPipeline => &[
            "decisions::implement_now_block(",
            "decisions::failed_edit_guard(",
        ],
        DecisionSource::Cache => &["decisions::cache_hit("],
        DecisionSource::Policy | DecisionSource::SelfReview => &["decisions::from_tool_results("],
        DecisionSource::Retry => &["DebugEvent::Retry {"],
        DecisionSource::Build => &["decisions::auto_build("],
        DecisionSource::Thinking => &["decisions::thinking_effort("],
        DecisionSource::Stop => &["decisions::stop("],
        // Gate decides outside the loop; the run recorder makes its
        // steps from the kernel's records.
        DecisionSource::Gate => &[],
    }
}

const LOOP_SOURCES: &[&str] = &[
    include_str!("../state/mod.rs"),
    include_str!("../context.rs"),
    include_str!("../tool_pipeline/mod.rs"),
    include_str!("../tool_pipeline/effects.rs"),
    include_str!("../stream_pump/driver/tool_dispatch.rs"),
    include_str!("../turn/mod.rs"),
    include_str!("../turn/recovery.rs"),
    include_str!("../sampling.rs"),
    include_str!("../run.rs"),
];

#[test]
fn every_decision_source_has_an_emit_site() {
    for source in DecisionSource::ALL {
        for site in emit_sites(source) {
            assert!(
                LOOP_SOURCES.iter().any(|src| src.contains(site)),
                "{} has no emit site `{site}` in the agent loop",
                source.as_str()
            );
        }
    }
}

fn call(id: &str, name: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        input: json!({"path": "src/lib.rs"}),
    }
}

fn result(id: &str, content: &str, is_error: bool, stop_loop: bool) -> ToolCallResult {
    ToolCallResult {
        tool_use_id: id.to_string(),
        content: content.to_string(),
        is_error,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop,
        file_changes: Vec::new(),
        image: None,
    }
}

#[test]
fn tool_results_yield_policy_and_self_review_decisions() {
    let calls = [
        call("a", "run_command"),
        call("b", "task_done"),
        call("c", "read_file"),
    ];
    let results = [
        result(
            "a",
            "Policy denied: tool 'run_command' is not allowed",
            true,
            false,
        ),
        result("b", "done", false, true),
        result("c", "fn main() {}", false, false),
    ];
    let out = from_tool_results(&calls, &results);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].source, DecisionSource::Policy);
    assert_eq!(out[0].action.label(), "block");
    assert_eq!(out[0].links.tool_use_ids, vec!["a".to_string()]);
    assert_eq!(out[1].source, DecisionSource::SelfReview);
    assert_eq!(out[1].action, HarnessAction::Allow);
}

#[test]
fn failed_edit_guard_links_tool_and_file() {
    let d = failed_edit_guard(&call("e", "edit_file"), 4, true);
    assert_eq!(d.action.label(), "stop");
    assert_eq!(d.links.files, vec!["src/lib.rs".to_string()]);
    assert_eq!(d.trigger["identical_failures"], 4);
}

#[test]
fn compaction_records_pressure() {
    let d = compaction("Tier60", 150_000, 60_000, 200_000);
    assert_eq!(
        d.action,
        HarnessAction::Compact {
            tier: "Tier60".into(),
            tokens_before: 150_000,
            tokens_after: 60_000,
        }
    );
    assert!((d.trigger["pressure"].as_f64().unwrap() - 0.75).abs() < 1e-9);
}

#[test]
fn a_run_that_carries_on_records_only_a_change_of_effort() {
    let mut config = super::super::AgentLoopConfig::for_agent("claude-test-model");
    let fresh = LoopState::new_for_tests(&config, Vec::new());
    assert_eq!(fresh.last_thinking_effort, None);
    config.initial_thinking_effort = Some(aura_model::ThinkingEffort::High);
    let carried_on = LoopState::new_for_tests(&config, Vec::new());
    assert_eq!(carried_on.last_thinking_effort.as_deref(), Some("high"));
    let raised = thinking_effort(Some("high"), "max", false, &carried_on);
    assert_eq!(raised.trigger["previous"], "high");
    assert!(
        raised.alternatives.iter().any(|a| a == "effort_max"),
        "{:?}",
        raised.alternatives
    );
}

#[test]
fn flush_stamps_missing_iterations_and_drains() {
    let config = super::super::AgentLoopConfig::for_agent("claude-test-model");
    let mut state = LoopState::new_for_tests(&config, Vec::new());
    state.decide(cache_hit(&call("x", "read_file")));
    state.decide(cache_hit(&call("y", "read_file")).at_iteration(1));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    flush(&mut state, Some(&tx), 5);
    assert!(state.decisions.is_empty());
    let mut iterations = Vec::new();
    while let Ok(AgentLoopEvent::Debug(DebugEvent::Decision { decision, .. })) = rx.try_recv() {
        iterations.push(decision.iteration);
    }
    assert_eq!(iterations, vec![Some(5), Some(1)]);
}
