use super::*;

// ------------------------------------------------------------------
// phase_reset_signal + LoopState::begin_iteration tests
// ------------------------------------------------------------------

/// The reset signal handshake: drive the exploration counter up,
/// flip the shared `Arc<AtomicBool>`, run one `begin_iteration`
/// tick, then assert the exploration counter is cleared. The
/// `ReadGuardState` / `BlockingContext` resets that used to be
/// exercised here were removed along with the dead detector
/// modules (cook-loop-fix follow-up, 2026-05) — the reset is now
/// purely an exploration-counter clear plus
/// `exploration_compaction_done` rearm.
#[test]
fn phase_reset_clears_exploration_budget() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let signal = Arc::new(AtomicBool::new(false));
    let config = AgentLoopConfig {
        phase_reset_signal: Some(Arc::clone(&signal)),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let mut state = super::super::LoopState::new(&config, vec![]);

    state.exploration_state.count = 40;
    state.exploration_compaction_done = true;

    signal.store(true, Ordering::Release);

    state.begin_iteration(&config, 5);

    assert_eq!(state.exploration_state.count, 0);
    assert!(!state.exploration_compaction_done);
    assert!(
        !signal.load(Ordering::Acquire),
        "signal must be consumed by begin_iteration"
    );
}

/// Phase 2: the `phase_reset_signal` flip on iteration > 0 must
/// latch `submit_plan_called` so the effort policy can drop to
/// `Low` once a plan has actually been accepted. The iteration-0
/// flip is the task-start pre-seed (see
/// `agent_runner::execute_task_tracked`) and must NOT latch the
/// signal — confirms the heuristic that separates "task start" from
/// "real submit_plan accept".
#[test]
fn submit_plan_signal_latches_only_on_iteration_after_zero() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let signal = Arc::new(AtomicBool::new(false));
    let config = AgentLoopConfig {
        phase_reset_signal: Some(Arc::clone(&signal)),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let mut state = super::super::LoopState::new(&config, vec![]);

    // Task-start pre-seed: iteration 0 observes the flip but the
    // latch stays off (this is the runner zeroing exploration, not
    // a real submit_plan accept).
    signal.store(true, Ordering::Release);
    state.begin_iteration(&config, 0);
    assert!(
        !state.submit_plan_called,
        "iteration-0 reset is the task-start pre-seed; must not latch"
    );

    // Real submit_plan accept (executor flips the signal mid-run):
    // iteration > 0 observes it and the latch turns on.
    signal.store(true, Ordering::Release);
    state.begin_iteration(&config, 4);
    assert!(
        state.submit_plan_called,
        "iteration > 0 reset is a real submit_plan accept; must latch"
    );

    // Latch is cumulative — subsequent iterations keep it set even
    // when the signal is not flipped again.
    state.begin_iteration(&config, 5);
    assert!(state.submit_plan_called, "latch is cumulative");
}

/// Companion: when the signal is wired but not flipped, the reset
/// branch must not fire — the exploration counter keeps ticking.
#[test]
fn begin_iteration_does_not_reset_when_signal_unset() {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let signal = Arc::new(AtomicBool::new(false));
    let config = AgentLoopConfig {
        phase_reset_signal: Some(Arc::clone(&signal)),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let mut state = super::super::LoopState::new(&config, vec![]);
    state.exploration_state.count = 10;

    state.begin_iteration(&config, 5);

    assert_eq!(state.exploration_state.count, 10);
}

#[test]
fn begin_iteration_injects_implement_now_after_exploration_threshold() {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use crate::types::{ToolCallInfo, ToolCallResult};

    let signal = Arc::new(AtomicBool::new(false));
    let config = AgentLoopConfig {
        phase_reset_signal: Some(signal),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let mut state = super::super::LoopState::new(&config, vec![Message::user("start")]);

    // Phase 5: drive the registry's `ImplementNowSteering` source
    // via `observe_tool` (the call site `tool_pipeline::track_tool_effects`
    // uses on every real run). Hand-setting
    // `state.exploration_state.count` is no longer sufficient
    // because the steering source tracks its own internal counter.
    let threshold = aura_config::IMPLEMENT_NOW_DEFAULT_THRESHOLD;
    for i in 0..threshold {
        let tool = ToolCallInfo {
            id: format!("toolu_explore_{i}"),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": format!("src/file_{i}.rs")}),
        };
        let result = ToolCallResult {
            tool_use_id: tool.id.clone(),
            content: "stub".to_string(),
            is_error: false,
            kind: aura_core_types::ToolResultKind::Ok,
            stop_loop: false,
            file_changes: Vec::new(),
            image: None,
        };
        state.steering.observe_tool(&tool, &result);
    }

    state.begin_iteration(&config, 3);

    assert!(state.implement_now_injected);
    assert!(
        state
            .messages
            .iter()
            .any(|m| m.text_content().contains("kind=\"implement_now\"")),
        "next model request must include the implement_now steering"
    );
}

/// Companion: without a wired signal (the chat path), begin_iteration
/// must not touch exploration counters at all.
#[test]
fn begin_iteration_no_op_when_no_signal_configured() {
    let config = AgentLoopConfig {
        phase_reset_signal: None,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let mut state = super::super::LoopState::new(&config, vec![]);
    state.exploration_state.count = 40;

    state.begin_iteration(&config, 5);

    assert_eq!(state.exploration_state.count, 40);
}

// ------------------------------------------------------------------
// Phase 3 — parallel tool calls per assistant turn
// ------------------------------------------------------------------
//
// Phase 7 removed `three_read_file_calls_execute_in_one_iteration`:
// the test depended on the buffered transport's batched
// `execute(tool_calls)` so `MockExecutor`'s positional-zip returned
// distinct ok-1 / ok-2 / ok-3 results. With the pump as the sole
// transport, each `tool_use` block spawns its own
// `execute(&[single_call])` and the positional zip collapses to
// ok-1 / ok-1 / ok-1. The pump-side regression contract (three
// parallel tool_use blocks land three tool_result blocks in the
// same iteration) lives in `agent_loop::parity_tests::*`.

// ------------------------------------------------------------------
// Phase 2 — `compute_thinking_effort` dev-loop policy
// ------------------------------------------------------------------

/// Temporary (2026-05): dev-loop turns (signalled by
/// `disable_thinking_iteration_0: true`, which `configure_loop_config`
/// sets exclusively for dev-loop tasks) are pinned to `Medium`
/// regardless of iteration / write / plan state while we evaluate
/// whether a single effort level converges faster than the
/// codex-style `Off → Medium → Low` taper.
#[test]
fn effort_medium_when_disable_thinking_iteration_0() {
    use aura_model::ThinkingEffort;

    let config = AgentLoopConfig {
        disable_thinking_iteration_0: true,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let mut state = super::super::LoopState::new(&config, vec![]);

    assert_eq!(
        state.compute_thinking_effort(&config, 0),
        ThinkingEffort::Medium,
        "iteration 0 dev-loop turn pinned to Medium"
    );

    state.had_any_file_write = true;
    assert_eq!(
        state.compute_thinking_effort(&config, 3),
        ThinkingEffort::Medium,
        "post-write dev-loop turn still pinned to Medium"
    );

    state.submit_plan_called = true;
    assert_eq!(
        state.compute_thinking_effort(&config, 5),
        ThinkingEffort::Medium,
        "post-submit_plan dev-loop turn still pinned to Medium"
    );
}

/// Default analysis turn (iteration 0 without
/// `disable_thinking_iteration_0`) gets `Medium` so the agent has
/// budget to plan before its first tool call. Mirrors codex's
/// default `reasoning.effort = medium`.
#[test]
fn effort_medium_on_iteration_0_no_disable() {
    use aura_model::ThinkingEffort;

    let config = AgentLoopConfig {
        disable_thinking_iteration_0: false,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let state = super::super::LoopState::new(&config, vec![]);
    assert_eq!(
        state.compute_thinking_effort(&config, 0),
        ThinkingEffort::Medium
    );
}

/// Once the first write_file/edit_file/delete_file has landed, drop
/// to `Low`. Caps the thinking spiral that amplifies follow-up
/// iterations after forward motion has already happened.
#[test]
fn effort_low_after_first_write() {
    use aura_model::ThinkingEffort;

    let config = AgentLoopConfig::for_agent("gpt-test-model");
    let mut state = super::super::LoopState::new(&config, vec![]);
    state.had_any_file_write = true;
    assert_eq!(
        state.compute_thinking_effort(&config, 3),
        ThinkingEffort::Low
    );
}

/// Once `submit_plan` has been accepted (signal observed on
/// iteration > 0), drop to `Low`. Codex's analogous behaviour is to
/// keep effort low during the implementation phase.
#[test]
fn effort_low_after_submit_plan() {
    use aura_model::ThinkingEffort;

    let config = AgentLoopConfig::for_agent("gpt-test-model");
    let mut state = super::super::LoopState::new(&config, vec![]);
    state.submit_plan_called = true;
    assert_eq!(
        state.compute_thinking_effort(&config, 2),
        ThinkingEffort::Low
    );
}

/// Claude Sonnet 5.5 and Opus 5 reason high on every iteration,
/// whatever the run has done and also in a dev-loop run. A choice of the
/// user's still wins.
#[test]
fn effort_high_on_sonnet_5_5_and_opus_5() {
    use aura_model::ThinkingEffort;

    for model in ["claude/claude-sonnet-5-5", "claude/claude-opus-5-5"] {
        for dev_loop in [false, true] {
            let config = AgentLoopConfig {
                disable_thinking_iteration_0: dev_loop,
                ..AgentLoopConfig::for_agent(model)
            };
            let mut state = super::super::LoopState::new(&config, vec![]);
            assert_eq!(
                state.compute_thinking_effort(&config, 0),
                ThinkingEffort::High,
                "{model}"
            );
            state.had_any_file_write = true;
            state.submit_plan_called = true;
            assert_eq!(
                state.compute_thinking_effort(&config, 3),
                ThinkingEffort::High,
                "{model}"
            );
        }
        let chosen = AgentLoopConfig {
            user_thinking_effort: Some(ThinkingEffort::Low),
            ..AgentLoopConfig::for_agent(model)
        };
        let state = super::super::LoopState::new(&chosen, vec![]);
        let request = state.build_request(&chosen, &[], 0).unwrap();
        assert_eq!(
            request.thinking_effort,
            Some(ThinkingEffort::Low),
            "{model}"
        );
        let default = AgentLoopConfig::for_agent(model);
        let state = super::super::LoopState::new(&default, vec![]);
        let request = state.build_request(&default, &[], 2).unwrap();
        assert_eq!(
            request.thinking_effort,
            Some(ThinkingEffort::High),
            "{model}"
        );
    }
}

/// The effort a router chose holds on every iteration in place of the
/// heuristic, and a choice of the user's wins over it.
#[test]
fn a_routed_effort_replaces_the_heuristic_and_the_users_wins() {
    use aura_model::ThinkingEffort;

    let routed = AgentLoopConfig {
        routed_thinking_effort: Some(ThinkingEffort::Max),
        ..AgentLoopConfig::for_agent("claude/claude-opus-5-5")
    };
    let mut state = super::super::LoopState::new(&routed, vec![]);
    state.had_any_file_write = true;
    for iteration in [0, 3] {
        let request = state.build_request(&routed, &[], iteration).unwrap();
        assert_eq!(request.thinking_effort, Some(ThinkingEffort::Max));
    }
    let pinned = AgentLoopConfig {
        user_thinking_effort: Some(ThinkingEffort::Low),
        ..routed
    };
    let request = state.build_request(&pinned, &[], 3).unwrap();
    assert_eq!(request.thinking_effort, Some(ThinkingEffort::Low));
}

/// A change of effort makes Claude write the conversation to its prompt
/// cache again, so a Claude model keeps `Medium` after a write or a plan.
#[test]
fn effort_stays_medium_on_claude_models() {
    use aura_model::ThinkingEffort;

    for model in [
        "claude/claude-sonnet-5",
        "aura-claude-opus-4-7",
        "claude-haiku-4-5",
    ] {
        let config = AgentLoopConfig::for_agent(model);
        let mut state = super::super::LoopState::new(&config, vec![]);
        state.had_any_file_write = true;
        state.submit_plan_called = true;
        assert_eq!(
            state.compute_thinking_effort(&config, 3),
            ThinkingEffort::Medium,
            "{model}"
        );
    }
}

/// Non-iteration-0 iterations without writes, plans, or continuation
/// pressure default to `Medium`. Confirms the policy doesn't
/// silently fall to `Low` without one of the explicit triggers.
#[test]
fn effort_medium_default_after_iteration_zero() {
    use aura_model::ThinkingEffort;

    let config = AgentLoopConfig::for_agent("claude-test-model");
    let state = super::super::LoopState::new(&config, vec![]);
    assert_eq!(
        state.compute_thinking_effort(&config, 3),
        ThinkingEffort::Medium
    );
}

/// Sanity: with the read-only / force-tool steering removed by the
/// cook-loop-fix strip (2026-05), `build_request` must always
/// produce `ToolChoice::Auto` regardless of any internal state.
#[test]
fn build_request_always_emits_tool_choice_auto() {
    use aura_model::{Message, ToolChoice};

    let config = AgentLoopConfig::for_agent("claude-test-model");
    let state = super::super::LoopState::new(&config, vec![Message::user("hi")]);
    let request = state
        .build_request(&config, &[], 5)
        .expect("build_request must succeed");
    assert!(
        matches!(request.tool_choice, ToolChoice::Auto),
        "tool_choice must always be Auto after the cook-loop-fix strip; got {:?}",
        request.tool_choice,
    );
}
