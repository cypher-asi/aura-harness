use super::convert::{resolve_output_config, resolve_thinking};
use crate::{ModelRequest, ThinkingEffort};

#[test]
fn native_effort_ladder_matches_each_model_capability() {
    let tiers = [
        ThinkingEffort::Low,
        ThinkingEffort::Medium,
        ThinkingEffort::High,
        ThinkingEffort::XHigh,
        ThinkingEffort::Max,
    ];
    for model in [
        "claude-opus-4-7",
        "aura-claude-opus-4-8",
        "claude-opus-5",
        "claude-sonnet-4-6",
        "claude-opus-4-6",
    ] {
        for (tier, expected) in tiers
            .into_iter()
            .zip(["low", "medium", "high", "xhigh", "max"])
        {
            let request = ModelRequest::builder(model, "system")
                .max_tokens(16384)
                .thinking_effort(Some(tier))
                .try_build()
                .unwrap();
            let expected = if model.ends_with("4-6") && expected == "xhigh" {
                "high"
            } else {
                expected
            };
            assert_eq!(
                resolve_output_config(&request, model).unwrap().effort,
                expected,
                "{model}: {tier:?}"
            );
        }
    }
}

#[test]
fn enabled_thinking_never_exceeds_explicit_response_allowance() {
    for cap in [800, 1024, 1025, 2048, 8192, 16384, 64000] {
        for tier in [
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
            ThinkingEffort::XHigh,
            ThinkingEffort::Max,
        ] {
            let request = ModelRequest::builder("claude-3-7-sonnet", "system")
                .max_tokens(cap)
                .thinking_effort(Some(tier))
                .try_build()
                .unwrap();
            let thinking = resolve_thinking(&request, "claude-3-7-sonnet");
            if cap <= 1024 {
                assert!(thinking.is_none());
            } else {
                let budget = thinking.unwrap().budget_tokens.unwrap();
                assert!(budget >= 1024 && budget < cap);
            }
        }
    }
}
