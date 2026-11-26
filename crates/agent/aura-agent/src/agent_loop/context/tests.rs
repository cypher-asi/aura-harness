use super::{
    compact_if_needed, compaction_pressure_tokens, effective_compaction_request_kind,
    heuristic_context_tokens, reserved_output_tokens, CompactionOutcome,
};
use crate::agent_loop::AgentLoopConfig;
use crate::agent_loop::LoopState;
use aura_context_compaction::{pick_stricter_tier, CompactionConfig};
use aura_model::{Message, ModelRequestKind, ToolDefinition};
use std::sync::{Mutex, OnceLock};

/// Serialize tests that swap `aura_config` to flip the compaction
/// kill switch. Sibling tests that call `compact_if_needed`
/// read the same config, so any toggle has
/// to be coordinated to keep parallel-test runs deterministic.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn install_compaction_disabled(disabled: bool) -> aura_config::ConfigGuard {
    let mut cfg = aura_config::current();
    cfg.agent.compaction.disabled = disabled;
    aura_config::install_for_test(cfg)
}

fn dummy_tool(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition::new(
        name,
        description,
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        }),
    )
}

#[test]
fn compaction_keeps_headroom_for_the_next_reply() {
    let config = AgentLoopConfig::for_agent("claude-haiku-4-5");
    assert_eq!(reserved_output_tokens(&config, 200_000), 16_384);
    let local = AgentLoopConfig::for_agent("local/qwen3:4b");
    assert_eq!(
        reserved_output_tokens(&local, 32_000),
        u64::from(aura_model::model_catalog::DEFAULT_LOCAL_MAX_OUTPUT_TOKENS)
    );
}

#[test]
fn reserve_is_capped_by_context_window() {
    let config = AgentLoopConfig::for_agent("claude-haiku-4-5");
    assert_eq!(reserved_output_tokens(&config, 8_000), 8_000);
}

#[test]
fn pressure_tokens_include_output_reserve() {
    let config = AgentLoopConfig::for_agent("claude-haiku-4-5");
    assert_eq!(compaction_pressure_tokens(&config, 60_000, 200_000), 76_384);
}

#[test]
fn a_reply_may_take_what_is_left_of_the_window() {
    let config = AgentLoopConfig {
        max_context_tokens: Some(200_000),
        ..AgentLoopConfig::for_agent("claude-haiku-4-5")
    };
    let request = |conversation_tokens: usize| {
        let messages = vec![Message::user("R".repeat(conversation_tokens * 4))];
        LoopState::new(&config, messages)
            .build_request(&config, &[], 0)
            .unwrap()
            .max_tokens
            .get()
    };
    assert_eq!(request(1_000), 64_000);
    assert_eq!(request(160_000), 40_000);
    assert_eq!(request(190_000), 64_000, "an overflow compacts instead");
}

/// `compact_if_needed` is the single place that recomputes the
/// per-bucket breakdown each turn. Verify every bucket lights up
/// from the obvious sources and that `mcp_tokens` stays at 0
/// (reserved for future MCP support).
#[test]
fn compact_if_needed_populates_context_breakdown() {
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let config = AgentLoopConfig {
        // Long enough that chars/CHARS_PER_TOKEN rounds to >= 1
        // even after `recompute_breakdown` subtracts `skills_chars`.
        system_prompt: "S".repeat(200),
        // 80 chars / 4 chars-per-token = 20 tokens.
        skills_chars: 80,
        // 60 chars / 4 = 15 tokens.
        subagents_chars: 60,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let mut state = LoopState::new(
        &config,
        vec![Message::user("hello"), Message::assistant("M".repeat(200))],
    );
    let tools = vec![
        dummy_tool("read_file", "Read a file from disk."),
        dummy_tool("write_file", "Write a file to disk."),
    ];

    compact_if_needed(&config, &mut state, &tools, 0);

    let breakdown = &state.result.context_breakdown;
    // system_prompt is reported net of `skills_chars` to avoid
    // double-counting injected skill text. (200 - 80) / 4 = 30.
    assert_eq!(breakdown.system_prompt_tokens, 30);
    assert!(
        breakdown.tools_tokens > 0,
        "tools bucket should be > 0 with two tool defs"
    );
    assert_eq!(breakdown.skills_tokens, 20);
    assert_eq!(breakdown.subagents_tokens, 15);
    assert_eq!(breakdown.mcp_tokens, 0, "mcp bucket is reserved (no MCP)");
    assert!(
        breakdown.conversation_tokens > 0,
        "conversation bucket should reflect the live transcript"
    );

    // The parallel `context_contents` must be populated from the
    // same turn: full system prompt present and one segment per
    // tool, so the contents viewer stays in sync with the bucket
    // token counts above.
    let contents = &state.result.context_contents;
    assert_eq!(
        contents.system_prompt.as_deref(),
        Some(config.system_prompt.as_str()),
        "system prompt text should be carried verbatim"
    );
    assert_eq!(
        contents.tools.len(),
        2,
        "one tool segment per effective tool def"
    );
    assert!(
        contents.tools.iter().all(|seg| !seg.text.is_empty()),
        "every tool segment should carry rendered text"
    );
    assert!(contents.mcp.is_empty(), "mcp bucket is reserved (empty)");
}

/// Empty inputs everywhere should yield a near-zero breakdown.
/// `validate_and_repair` always inserts a sentinel user message
/// when the conversation is empty, so `conversation_tokens` is
/// allowed to be small but non-zero — the static buckets must
/// stay at zero so the frontend's "all-zero ⇒ unavailable"
/// sentinel still triggers correctly.
#[test]
fn compact_if_needed_static_buckets_zero_when_nothing_configured() {
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let mut state = LoopState::new(&config, vec![]);

    compact_if_needed(&config, &mut state, &[], 0);

    let b = &state.result.context_breakdown;
    assert_eq!(b.system_prompt_tokens, 0);
    assert_eq!(b.tools_tokens, 0);
    assert_eq!(b.skills_tokens, 0);
    assert_eq!(b.subagents_tokens, 0);
    assert_eq!(b.mcp_tokens, 0);
}

/// Setting `aura_config::agent().compaction.disabled = true`
/// (sourced from `AURA_AGENT_DISABLE_COMPACTION`) must
/// short-circuit `compact_if_needed` so older `read_file` results
/// never get truncated, even on a config that would normally hit
/// the aggressive tier. This is the operator escape hatch used to
/// test whether read-spiral behavior is being driven by mid-run
/// truncation.
#[test]
fn kill_switch_disables_compact_if_needed() {
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = install_compaction_disabled(true);

    let huge = "X".repeat(200_000);
    let mut messages = vec![
        Message::user("start"),
        Message::assistant(huge.clone()),
        Message::user("more"),
        Message::assistant(huge.clone()),
    ];
    let config = AgentLoopConfig {
        max_context_tokens: Some(8_000),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let before_chars: usize = messages.iter().map(|m| m.text_content().len()).sum();
    let mut state = LoopState::new(&config, std::mem::take(&mut messages));

    let outcome = compact_if_needed(&config, &mut state, &[], 0);

    let after_chars: usize = state.messages.iter().map(|m| m.text_content().len()).sum();

    assert!(
        matches!(outcome, super::CompactionOutcome::None),
        "kill switch must report None outcome"
    );
    assert_eq!(
        after_chars, before_chars,
        "kill switch must leave message bytes untouched"
    );
}

#[test]
fn compaction_trigger_leaves_transcript_untouched_below_threshold() {
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let config = AgentLoopConfig {
        max_context_tokens: Some(200_000),
        compaction_trigger: Some(0.70),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(Message::user(format!("step {i}")));
        messages.push(Message::assistant("R".repeat(20_000)));
    }
    messages.push(Message::user("next"));
    let before = serde_json::to_string(&messages).unwrap();
    let mut state = LoopState::new(&config, messages);

    let outcome = compact_if_needed(&config, &mut state, &[], 3);

    assert!(matches!(outcome, CompactionOutcome::None), "{outcome:?}");
    assert_eq!(
        serde_json::to_string(&*state.messages).unwrap(),
        before,
        "below the trigger the cached prefix must stay byte-identical"
    );
}

#[test]
fn pick_stricter_tier_prefers_smaller_tool_result_cap() {
    let light = CompactionConfig::light();
    let aggressive = CompactionConfig::aggressive();

    assert_eq!(
        pick_stricter_tier(Some(light), Some(aggressive))
            .unwrap()
            .tool_result_max_chars,
        aggressive.tool_result_max_chars
    );
    assert_eq!(
        pick_stricter_tier(Some(aggressive), Some(light))
            .unwrap()
            .tool_result_max_chars,
        aggressive.tool_result_max_chars
    );
    assert_eq!(
        pick_stricter_tier(Some(light), None)
            .unwrap()
            .tool_result_max_chars,
        light.tool_result_max_chars
    );
    assert_eq!(
        pick_stricter_tier(None, Some(aggressive))
            .unwrap()
            .tool_result_max_chars,
        aggressive.tool_result_max_chars
    );
    assert!(pick_stricter_tier(None, None).is_none());
}

// -----------------------------------------------------------------
// effective_compaction_request_kind: pins the bootstrap →
// continuation projection so `compact_if_needed` / `accumulate_response`
// see the same kind that `build_request` will actually ship on the
// wire. Without this projection the dev-loop bootstrap's 24 KiB body
// cap leaks into every continuation iteration and pins compaction
// cap-pressure at 1.0 for the rest of the task (which fires
// `NeedsSummary` on every iteration and doubles the outbound
// Anthropic call rate).
// -----------------------------------------------------------------

#[test]
fn effective_kind_keeps_bootstrap_on_iter_zero() {
    let config = AgentLoopConfig {
        request_kind: ModelRequestKind::DevLoopBootstrap,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    assert_eq!(
        effective_compaction_request_kind(&config, 0),
        ModelRequestKind::DevLoopBootstrap
    );
}

#[test]
fn effective_kind_projects_bootstrap_to_continuation_after_iter_zero() {
    let config = AgentLoopConfig {
        request_kind: ModelRequestKind::DevLoopBootstrap,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    for iter in 1usize..=8 {
        assert_eq!(
            effective_compaction_request_kind(&config, iter),
            ModelRequestKind::DevLoopContinuation,
            "iteration {iter} should project bootstrap → continuation"
        );
    }
}

#[test]
fn effective_kind_passes_other_kinds_through_unchanged() {
    for kind in [
        ModelRequestKind::Chat,
        ModelRequestKind::DevLoopContinuation,
        ModelRequestKind::Auxiliary,
        ModelRequestKind::ProjectToolSpecGen,
        ModelRequestKind::ProjectToolTaskExtract,
    ] {
        let config = AgentLoopConfig {
            request_kind: kind,
            ..AgentLoopConfig::for_agent("claude-test-model")
        };
        for iter in 0usize..=3 {
            assert_eq!(
                effective_compaction_request_kind(&config, iter),
                kind,
                "non-dev-loop kinds must pass through unchanged ({kind:?} @ iter {iter})"
            );
        }
    }
}

/// Regression for the doubled-Anthropic-call-rate bug: in a
/// dev-loop run with the bootstrap kind seeded into the config,
/// passing `iteration > 0` to `compact_if_needed` must NOT escalate
/// to `NeedsSummary` on a transcript that only crosses the 24 KiB
/// bootstrap body cap (and is otherwise well below
/// `summary_at = 0.85` on the model's real context window).
///
/// Before the fix, the stale `config.request_kind = DevLoopBootstrap`
/// flowed into `CompactionPolicy::request_kind` for every iteration;
/// the resulting `cap_pressure = raw_message_bytes / 24576` clamped
/// to 1.0 the moment the transcript crossed 24 KiB and fired
/// `NeedsSummary` on every subsequent call.
#[test]
fn compact_if_needed_does_not_escalate_when_continuation_outgrows_bootstrap_cap() {
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());

    // Real opus-class context window so context-pressure stays well
    // below the 0.85 summary threshold; the only signal that could
    // possibly trigger NeedsSummary here is the bootstrap body-cap
    // leak. ~40 KiB of transcript clears the 24 KiB bootstrap cap
    // with margin but is < 0.01% of the 1 M-token context window.
    let config = AgentLoopConfig {
        request_kind: ModelRequestKind::DevLoopBootstrap,
        max_context_tokens: Some(1_000_000),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let big_payload = "X".repeat(40_000);
    let mut state = LoopState::new(
        &config,
        vec![
            Message::user("seed"),
            Message::assistant(big_payload),
            Message::user("next"),
        ],
    );
    state.last_context_tokens_estimate = Some(heuristic_context_tokens(&state.messages));

    // iteration > 0 ⇒ effective kind = DevLoopContinuation (no cap).
    let outcome = compact_if_needed(&config, &mut state, &[], 5);

    assert!(
            matches!(outcome, CompactionOutcome::None | CompactionOutcome::Applied(_)),
            "continuation iteration on a real-sized context window must not fire NeedsSummary; got {outcome:?}"
        );
}

// (The "bootstrap iteration 0 must escalate" mirror test was
// dropped because the assertion was too coupled to internal
// compaction-tier byte budgets: a single 40 KiB assistant blob is
// compressible enough that the local `Aggressive` tier already
// brings the transcript under `target_total_chars`, so
// `NeedsSummary` legitimately doesn't fire even though the cap
// pressure was correctly observed. The bootstrap-side contract is
// pinned at the helper level by
// `effective_kind_keeps_bootstrap_on_iter_zero` above, which is
// the only branch this fix actually changes.)
