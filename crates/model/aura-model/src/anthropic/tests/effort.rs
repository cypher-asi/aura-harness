//! The `ThinkingEffort` knob and `output_config.effort`.

use super::support::*;
use crate::anthropic::convert::{resolve_output_config, resolve_thinking};
use crate::{ModelRequest, ModelRequestKind, ThinkingEffort};

#[test]
fn thinking_effort_off_emits_no_config() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(16_384)
        .thinking_effort(Some(ThinkingEffort::Off))
        .try_build()
        .unwrap();
    assert!(resolve_thinking(&request, TEST_DEFAULT_MODEL).is_none());
    assert!(resolve_output_config(&request, TEST_DEFAULT_MODEL).is_none());
}

#[test]
fn thinking_effort_off_disables_haiku_5_thinking() {
    for model in ["claude-haiku-5-5", "aura-claude-haiku-5-5"] {
        let request = ModelRequest::builder(model, "system")
            .max_tokens(32)
            .thinking_effort(Some(ThinkingEffort::Off))
            .try_build()
            .unwrap();
        let thinking = resolve_thinking(&request, model).expect("{model}");
        assert_eq!(
            serde_json::to_value(&thinking).unwrap(),
            serde_json::json!({"type": "disabled"}),
            "{model}"
        );
        assert!(resolve_output_config(&request, model).is_none(), "{model}");
    }
    for model in ["claude-haiku-4-5", "claude-sonnet-5-5", "claude-opus-5-5"] {
        let request = ModelRequest::builder(model, "system")
            .max_tokens(32)
            .thinking_effort(Some(ThinkingEffort::Off))
            .try_build()
            .unwrap();
        assert!(resolve_thinking(&request, model).is_none(), "{model}");
    }
}

#[test]
fn haiku_5_thinks_adaptively_at_the_effort_asked_for() {
    for (effort, expected) in [
        (ThinkingEffort::Minimal, "low"),
        (ThinkingEffort::Low, "low"),
        (ThinkingEffort::Medium, "medium"),
        (ThinkingEffort::High, "high"),
        (ThinkingEffort::XHigh, "high"),
        (ThinkingEffort::Max, "high"),
    ] {
        for model in ["claude-haiku-5-5", "aura-claude-haiku-5-5"] {
            let request = ModelRequest::builder(model, "system")
                .max_tokens(16_384)
                .thinking_effort(Some(effort))
                .try_build()
                .unwrap();
            let thinking = resolve_thinking(&request, model).expect("Haiku 5 thinks");
            assert_eq!(
                serde_json::to_value(&thinking).unwrap(),
                serde_json::json!({"type": "adaptive", "display": "summarized"}),
                "{model}"
            );
            assert_eq!(
                resolve_output_config(&request, model).unwrap().effort,
                expected,
                "{model}: {effort:?}"
            );
        }
    }
}

/// Regression for `thinking.adaptive.budget_tokens: Extra inputs are not
/// permitted`. Anthropic's `adaptive` thinking mode (Claude 4 family)
/// rejects `budget_tokens` outright — the model picks its own budget.
/// The Low/Medium/High effort knob must therefore translate to
/// `{"type":"adaptive"}` with no `budget_tokens` for adaptive models.
#[test]
fn thinking_effort_low_adaptive_omits_budget_tokens() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(16_384)
        .thinking_effort(Some(ThinkingEffort::Low))
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL).expect("Low emits a config");
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
    // Low must NOT inherit the forced effort=high override — that's
    // exactly the spiral amplifier we want to cap.
    assert_eq!(
        resolve_output_config(&request, TEST_DEFAULT_MODEL)
            .unwrap()
            .effort,
        "low"
    );
}

#[test]
fn thinking_effort_medium_adaptive_omits_budget_tokens() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(16_384)
        .thinking_effort(Some(ThinkingEffort::Medium))
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL).expect("Medium emits a config");
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
    assert_eq!(
        resolve_output_config(&request, TEST_DEFAULT_MODEL)
            .unwrap()
            .effort,
        "medium"
    );
}

#[test]
fn thinking_effort_high_adaptive_omits_budget_tokens_but_keeps_output_effort() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8_192)
        .thinking_effort(Some(ThinkingEffort::High))
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL).expect("High emits a config");
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);

    // High retains the existing forced effort=high override for adaptive.
    let out = resolve_output_config(&request, TEST_DEFAULT_MODEL).expect("High keeps effort=high");
    assert_eq!(out.effort, "high");
}

#[test]
fn opus_5_preserves_native_output_effort_ladder() {
    for (thinking_effort, expected) in [
        (ThinkingEffort::Low, "low"),
        (ThinkingEffort::Medium, "medium"),
        (ThinkingEffort::High, "high"),
        (ThinkingEffort::XHigh, "xhigh"),
        (ThinkingEffort::Max, "max"),
    ] {
        let request = ModelRequest::builder("aura-claude-opus-5", "system")
            .max_tokens(64_000)
            .thinking_effort(Some(thinking_effort))
            .try_build()
            .unwrap();
        let output = resolve_output_config(&request, "aura-claude-opus-5")
            .expect("Opus 5 effort should reach output_config");
        assert_eq!(output.effort, expected);
    }
}

#[test]
fn fable_and_mythos_5_1_preserve_native_output_effort_ladder() {
    for model in ["aura-claude-fable-5-1", "aura-claude-mythos-5-1"] {
        for (thinking_effort, expected) in [
            (ThinkingEffort::Low, "low"),
            (ThinkingEffort::Medium, "medium"),
            (ThinkingEffort::High, "high"),
            (ThinkingEffort::XHigh, "xhigh"),
            (ThinkingEffort::Max, "max"),
        ] {
            let request = ModelRequest::builder(model, "system")
                .max_tokens(128_000)
                .thinking_effort(Some(thinking_effort))
                .try_build()
                .unwrap();
            let output = resolve_output_config(&request, model)
                .expect("Claude 5.1 effort should reach output_config");
            assert_eq!(output.effort, expected, "{model}");
        }
    }
}

/// Companion to the adaptive tests: the `enabled` thinking mode
/// (Claude 3.7 sonnet) DOES accept `budget_tokens`, and the effort knob
/// keeps the calibrated 1024 / 4096 / clamped budgets there.
#[test]
fn thinking_effort_low_enabled_sets_1024_budget() {
    let request = ModelRequest::builder("claude-3-7-sonnet", "system")
        .max_tokens(16_384)
        .thinking_effort(Some(ThinkingEffort::Low))
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "claude-3-7-sonnet").expect("Low emits a config");
    assert_eq!(thinking.thinking_type, "enabled");
    assert_eq!(thinking.budget_tokens, Some(1024));
}

#[test]
fn thinking_effort_medium_enabled_sets_4096_budget() {
    let request = ModelRequest::builder("claude-3-7-sonnet", "system")
        .max_tokens(16_384)
        .thinking_effort(Some(ThinkingEffort::Medium))
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "claude-3-7-sonnet").expect("Medium emits a config");
    assert_eq!(thinking.thinking_type, "enabled");
    assert_eq!(thinking.budget_tokens, Some(4096));
}

#[test]
fn thinking_effort_high_enabled_clamps_budget_to_8192_16000() {
    // Thinking must leave room for output and be strictly below max_tokens.
    let request = ModelRequest::builder("claude-3-7-sonnet", "system")
        .max_tokens(8_192)
        .thinking_effort(Some(ThinkingEffort::High))
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "claude-3-7-sonnet").expect("High emits a config");
    assert_eq!(thinking.thinking_type, "enabled");
    assert_eq!(thinking.budget_tokens, Some(4_096));

    // Large ceiling: clamp pulls the budget down to 16000.
    let request_big = ModelRequest::builder("claude-3-7-sonnet", "system")
        .max_tokens(64_000)
        .thinking_effort(Some(ThinkingEffort::High))
        .try_build()
        .unwrap();
    let thinking_big =
        resolve_thinking(&request_big, "claude-3-7-sonnet").expect("High emits a config");
    assert_eq!(thinking_big.budget_tokens, Some(16_000));
}

#[test]
fn thinking_effort_none_preserves_legacy_max_tokens_gate() {
    // No effort set + max_tokens above the legacy 2048 threshold:
    // legacy auto-enable fires unchanged.
    let auto = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8_192)
        .try_build()
        .unwrap();
    let thinking_auto = resolve_thinking(&auto, TEST_DEFAULT_MODEL).expect("legacy auto-enable");
    assert_eq!(thinking_auto.thinking_type, "adaptive");
    assert_eq!(thinking_auto.budget_tokens, None);
    let out_auto = resolve_output_config(&auto, TEST_DEFAULT_MODEL).expect("legacy effort=high");
    assert_eq!(out_auto.effort, "high");

    // No effort set + max_tokens at/below 2048: legacy gate keeps it off.
    let off = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(2_048)
        .try_build()
        .unwrap();
    assert!(resolve_thinking(&off, TEST_DEFAULT_MODEL).is_none());
}

#[test]
fn thinking_effort_returns_none_for_models_without_thinking_support() {
    // Models that don't support thinking (e.g. Haiku) must stay off
    // regardless of the effort knob — capability detection wins.
    let request = ModelRequest::builder("aura-claude-haiku-4-5", "system")
        .max_tokens(16_384)
        .thinking_effort(Some(ThinkingEffort::High))
        .try_build()
        .unwrap();
    assert!(resolve_thinking(&request, "aura-claude-haiku-4-5").is_none());
}

#[test]
fn thinking_effort_low_on_older_model_emits_enabled_type() {
    // Sanity: Claude 3.7 sonnet still maps to the "enabled" thinking
    // type, but with the Low budget instead of the auto-derived one.
    let request = ModelRequest::builder("claude-3-7-sonnet", "system")
        .max_tokens(16_384)
        .thinking_effort(Some(ThinkingEffort::Low))
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "claude-3-7-sonnet").expect("Low emits a config");
    assert_eq!(thinking.thinking_type, "enabled");
    assert_eq!(thinking.budget_tokens, Some(1024));
}

#[test]
fn test_resolve_output_config_only_for_claude_4_thinking_models() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let output = resolve_output_config(&request, TEST_DEFAULT_MODEL).unwrap();
    assert_eq!(output.effort, "high");

    let sonnet_37_output = resolve_output_config(&request, "claude-3-7-sonnet");
    assert!(sonnet_37_output.is_none());
}

/// Regression for Anthropic's May 2026 removal of `thinking.type.enabled`
/// on the Claude 4 family. Dev-loop runs used to escalate the adaptive
/// thinking mode for opus-4 / sonnet-4 to coax visible thinking blocks,
/// which now 400s with
/// `"thinking.type.enabled" is not supported for this model.
///  Use "thinking.type.adaptive" and "output_config.effort" to control
///  thinking behavior.` Every dev-loop ModelRequestKind on an adaptive
/// model must therefore stay on `adaptive` (no `budget_tokens`) and lean
/// on `output_config.effort: "high"` instead.
#[test]
fn dev_loop_kinds_on_adaptive_model_stay_adaptive_with_effort_high() {
    for kind in [
        ModelRequestKind::DevLoopBootstrap,
        ModelRequestKind::DevLoopContinuation,
        ModelRequestKind::ProjectToolTaskExtract,
        ModelRequestKind::ProjectToolSpecGen,
    ] {
        let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
            .max_tokens(8192)
            .request_kind(kind)
            .try_build()
            .unwrap();
        let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL)
            .unwrap_or_else(|| panic!("{kind:?} should still emit a thinking config"));
        assert_eq!(
            thinking.thinking_type, "adaptive",
            "{kind:?} must NOT escalate to thinking.type=enabled on opus-4/sonnet-4; Anthropic rejects that combination"
        );
        assert_eq!(
            thinking.budget_tokens, None,
            "{kind:?}: adaptive mode rejects budget_tokens",
        );
        let output = resolve_output_config(&request, TEST_DEFAULT_MODEL)
            .unwrap_or_else(|| panic!("{kind:?} should pair adaptive with output_config.effort"));
        assert_eq!(
            output.effort, "high",
            "{kind:?} must keep effort=high so adaptive still produces visible thinking",
        );
    }
}
