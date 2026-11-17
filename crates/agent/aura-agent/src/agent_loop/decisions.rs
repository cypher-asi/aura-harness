//! Harness decision capture for run traces.
//!
//! Every heuristic that changes what the model sees or what a tool
//! call does records a [`HarnessDecision`] on [`LoopState`]. The queue
//! is drained to the event channel as [`DebugEvent::Decision`] frames
//! at points that hold the sender. Decisions carry the features the
//! heuristic looked at so offline analysis can relate the decision to
//! what happened next.

use aura_context_prompts::steering::SteeringKind;
use aura_core_types::trace::{DecisionSource, HarnessAction, HarnessDecision};
use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc::Sender;

use crate::events::{AgentLoopEvent, DebugEvent};
use crate::types::{ToolCallInfo, ToolCallResult};

use super::{streaming, LoopState};

/// Emit queued decisions, stamping `iteration` on any that lack one.
pub(super) fn flush(
    state: &mut LoopState,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    iteration: usize,
) {
    if state.decisions.is_empty() {
        return;
    }
    let stamp = u32::try_from(iteration).ok();
    for mut decision in state.decisions.drain(..) {
        if decision.iteration.is_none() {
            decision.iteration = stamp;
        }
        streaming::emit(
            event_tx,
            AgentLoopEvent::Debug(DebugEvent::Decision {
                timestamp: Utc::now(),
                decision: Box::new(decision),
                task_id: None,
            }),
        );
    }
}

/// Shared loop state most heuristics condition on.
fn loop_features(state: &LoopState) -> serde_json::Value {
    json!({
        "exploration_count": state.exploration_state.count,
        "had_any_write": state.had_any_write,
        "had_any_file_write": state.had_any_file_write,
        "messages": state.messages.len(),
        "context_tokens": state.result.estimated_context_tokens,
    })
}

/// A steering message injected at the top of an iteration.
pub(super) fn steering_nudge(
    kind: &SteeringKind,
    state: &LoopState,
    iteration: usize,
) -> HarnessDecision {
    let trigger = match kind {
        SteeringKind::TaskDoneNoWrites => json!({}),
        SteeringKind::StubDetected { reports } => json!({
            "stubs": reports.len(),
            "paths": reports.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
        }),
        SteeringKind::RepeatedRead { content_hash } => json!({
            "content_hash": content_hash,
            "threshold": aura_config::agent().steering.repeated_read_threshold,
        }),
        SteeringKind::TaskAlreadySatisfiedHint { test_command } => {
            json!({ "test_command": test_command })
        }
        SteeringKind::ImplementNow {
            exploration_count,
            sample_paths,
        } => json!({
            "exploration_count": exploration_count,
            "threshold": aura_config::agent().steering.implement_now_threshold,
            "sample_paths": sample_paths,
        }),
    };
    let label = kind.label();
    let mut decision = HarnessDecision::new(
        DecisionSource::Steering,
        label,
        HarnessAction::Nudge {
            kind: label.to_string(),
        },
    )
    .at_iteration(iteration)
    .with_features(loop_features(state))
    .with_trigger(trigger)
    .with_alternatives(&["allow"]);
    if let SteeringKind::StubDetected { reports } = kind {
        for report in reports {
            decision = decision.with_file(report.path.clone());
        }
    }
    decision
}

/// An exploration call refused by the `implement_now` hard block.
pub(super) fn implement_now_block(call: &ToolCallInfo, state: &LoopState) -> HarnessDecision {
    HarnessDecision::new(
        DecisionSource::ToolPipeline,
        "implement_now_block",
        HarnessAction::Block {
            tool: call.name.clone(),
            tool_use_id: Some(call.id.clone()),
            reason: "exploration after implement_now without any file write".to_string(),
        },
    )
    .with_features(loop_features(state))
    .with_alternatives(&["allow"])
    .with_tool_use(call.id.clone())
}

/// A tool call served from the per-run result cache.
pub(super) fn cache_hit(call: &ToolCallInfo) -> HarnessDecision {
    let mut decision = HarnessDecision::new(
        DecisionSource::Cache,
        "tool_result_cache",
        HarnessAction::CacheHit {
            tool: call.name.clone(),
            tool_use_id: call.id.clone(),
        },
    )
    .with_alternatives(&["execute"])
    .with_tool_use(call.id.clone());
    if let Some(path) = call.input.get("path").and_then(|v| v.as_str()) {
        decision = decision.with_file(path);
    }
    decision
}

/// The failed-edit guard warned about or stopped a repeated edit.
pub(super) fn failed_edit_guard(call: &ToolCallInfo, count: u32, stop: bool) -> HarnessDecision {
    let steering = aura_config::agent().steering;
    let action = if stop {
        HarnessAction::Stop {
            reason: "tool_no_progress".to_string(),
        }
    } else {
        HarnessAction::Warn {
            message: "identical edit failed repeatedly".to_string(),
        }
    };
    let mut decision =
        HarnessDecision::new(DecisionSource::ToolPipeline, "failed_edit_guard", action)
            .with_trigger(json!({
                "identical_failures": count,
                "warn_after": steering.failed_edit_warn_after,
                "stop_after": steering.failed_edit_stop_after,
            }))
            .with_alternatives(if stop { &["warn", "allow"] } else { &["allow"] })
            .with_tool_use(call.id.clone());
    if let Some(path) = call.input.get("path").and_then(|v| v.as_str()) {
        decision = decision.with_file(path);
    }
    decision
}

/// Context compaction rewrote message history before a model call.
pub(super) fn compaction(
    tier: impl Into<String>,
    tokens_before: u64,
    tokens_after: u64,
    max_context_tokens: u64,
) -> HarnessDecision {
    let tier = tier.into();
    HarnessDecision::new(
        DecisionSource::Compaction,
        "compact_if_needed",
        HarnessAction::Compact {
            tier: tier.clone(),
            tokens_before,
            tokens_after,
        },
    )
    .with_trigger(json!({
        "tier": tier,
        "max_context_tokens": max_context_tokens,
        "pressure": ratio(tokens_before, max_context_tokens),
    }))
    .with_alternatives(&["none"])
}

/// An iteration-budget warning appended to the context.
pub(super) fn budget_warning(
    state: &LoopState,
    utilization: f64,
    message: &str,
) -> HarnessDecision {
    let thresholds = aura_config::agent().thinking;
    HarnessDecision::new(
        DecisionSource::Budget,
        "iteration_budget_warning",
        HarnessAction::Warn {
            message: message.to_string(),
        },
    )
    .with_features(loop_features(state))
    .with_trigger(json!({
        "utilization": utilization,
        "warn_30": thresholds.budget_warning_30,
        "warn_40_no_write": thresholds.budget_warning_40_no_write,
        "warn_60": thresholds.budget_warning_60,
    }))
    .with_alternatives(&["allow"])
}

/// The loop stopped because the iteration or credit budget ran out.
pub(super) fn budget_stop(
    state: &LoopState,
    iterations_done: usize,
    max_iterations: usize,
    credit_budget: Option<u64>,
) -> HarnessDecision {
    HarnessDecision::new(
        DecisionSource::Budget,
        "budget_stop",
        HarnessAction::Stop {
            reason: "budget_exhausted".to_string(),
        },
    )
    .with_features(loop_features(state))
    .with_trigger(json!({
        "iterations_done": iterations_done,
        "max_iterations": max_iterations,
        "total_tokens": state.result.total_input_tokens + state.result.total_output_tokens,
        "credit_budget": credit_budget,
    }))
    .with_alternatives(&["continue"])
}

/// The automatic post-write build check ran.
pub(super) fn auto_build(success: bool, error_count: usize, files: &[String]) -> HarnessDecision {
    let mut decision = HarnessDecision::new(
        DecisionSource::Build,
        "auto_build",
        HarnessAction::Build {
            success,
            error_count,
        },
    )
    .with_trigger(json!({
        "cooldown": aura_config::agent().thinking.auto_build_cooldown,
    }))
    .with_alternatives(&["skip"]);
    for file in files {
        decision = decision.with_file(file.clone());
    }
    decision
}

/// Decisions visible only in a batch's tool results: kernel policy
/// denials and `task_done` completion-gate verdicts.
pub(super) fn from_tool_results(
    calls: &[ToolCallInfo],
    results: &[ToolCallResult],
) -> Vec<HarnessDecision> {
    let mut out = Vec::new();
    for result in results {
        let Some(call) = calls.iter().find(|c| c.id == result.tool_use_id) else {
            continue;
        };
        if result.is_error && crate::task_executor::is_policy_denial(&result.content) {
            out.push(
                HarnessDecision::new(
                    DecisionSource::Policy,
                    "kernel_policy",
                    HarnessAction::Block {
                        tool: call.name.clone(),
                        tool_use_id: Some(call.id.clone()),
                        reason: first_line(&result.content),
                    },
                )
                .with_alternatives(&["allow"])
                .with_tool_use(call.id.clone()),
            );
        } else if call.name == "task_done" {
            let action = if result.is_error {
                HarnessAction::Block {
                    tool: call.name.clone(),
                    tool_use_id: Some(call.id.clone()),
                    reason: first_line(&result.content),
                }
            } else {
                HarnessAction::Allow
            };
            out.push(
                HarnessDecision::new(DecisionSource::SelfReview, "task_done_gate", action)
                    .with_trigger(json!({ "stop_loop": result.stop_loop }))
                    .with_alternatives(if result.is_error {
                        &["allow"]
                    } else {
                        &["block"]
                    })
                    .with_tool_use(call.id.clone()),
            );
        }
    }
    out
}

/// The reasoning effort for the next request changed.
pub(super) fn thinking_effort(
    previous: Option<&str>,
    next: &str,
    user_pinned: bool,
    state: &LoopState,
) -> HarnessDecision {
    HarnessDecision::new(
        DecisionSource::Thinking,
        "thinking_effort",
        HarnessAction::Nudge {
            kind: format!("effort_{next}"),
        },
    )
    .with_features(loop_features(state))
    .with_trigger(json!({
        "previous": previous,
        "next": next,
        "user_pinned": user_pinned,
        "submit_plan_called": state.submit_plan_called,
    }))
    .with_alternatives(&["effort_low", "effort_medium", "effort_high"])
}

fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

/// A turn-end recovery nudge re-prompted the model instead of ending
/// the turn.
pub(super) fn turn_nudge(name: &str, state: &LoopState) -> HarnessDecision {
    HarnessDecision::new(
        DecisionSource::Steering,
        name,
        HarnessAction::Nudge {
            kind: name.to_string(),
        },
    )
    .with_features(loop_features(state))
    .with_alternatives(&["end_turn"])
}

/// The turn ended for a harness-determined reason.
pub(super) fn stop(name: &str, reason: &str, state: &LoopState) -> HarnessDecision {
    HarnessDecision::new(
        DecisionSource::Stop,
        name,
        HarnessAction::Stop {
            reason: reason.to_string(),
        },
    )
    .with_features(loop_features(state))
}

#[allow(clippy::cast_precision_loss)]
fn ratio(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

#[cfg(test)]
mod tests {
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
            DecisionSource::Policy | DecisionSource::SelfReview => {
                &["decisions::from_tool_results("]
            }
            DecisionSource::Retry => &["DebugEvent::Retry {"],
            DecisionSource::Build => &["decisions::auto_build("],
            DecisionSource::Thinking => &["decisions::thinking_effort("],
            DecisionSource::Stop => &["decisions::stop("],
        }
    }

    const LOOP_SOURCES: &[&str] = &[
        include_str!("state.rs"),
        include_str!("context.rs"),
        include_str!("tool_pipeline.rs"),
        include_str!("stream_pump/driver.rs"),
        include_str!("turn.rs"),
        include_str!("sampling.rs"),
        include_str!("run.rs"),
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
}
