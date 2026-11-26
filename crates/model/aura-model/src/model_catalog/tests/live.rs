use super::support::{assert_cost, usage};
use crate::model_catalog::output_limits::UNKNOWN_OUTPUT_TOKENS;
use crate::model_catalog::*;

/// An OpenRouter listing: an unknown model, a curated one with a dearer
/// listed price and a zero window, another backend's id, a dated slug and
/// a base model for variant lookup.
fn live_listing(
    unknown: &str,
    curated: &str,
    listed_price: LivePrice,
) -> [(String, LiveModelInfo); 5] {
    [
        (
            unknown.to_string(),
            LiveModelInfo {
                context_window: Some(262_144),
                max_output: Some(16_000),
                price: Some(listed_price),
            },
        ),
        (
            curated.to_string(),
            LiveModelInfo {
                context_window: Some(0),
                max_output: Some(20_000),
                price: Some(LivePrice {
                    input: 99.0,
                    ..listed_price
                }),
            },
        ),
        // Another backend's id: ignored.
        (
            "aura-live-test-model".to_string(),
            LiveModelInfo {
                context_window: Some(1),
                max_output: None,
                price: None,
            },
        ),
        (
            "openrouter/deepseek/deepseek-v4.1-flash-1017".to_string(),
            LiveModelInfo {
                context_window: None,
                max_output: None,
                price: Some(listed_price),
            },
        ),
        (
            "openrouter/acme/variant-test".to_string(),
            LiveModelInfo {
                context_window: None,
                max_output: None,
                price: Some(LivePrice {
                    input: 1.0,
                    output: 1.0,
                    ..listed_price
                }),
            },
        ),
    ]
}

// The live table is process-wide and tests run in parallel, so this is
// the only test that writes it, and only for OpenRouter ids no other
// test reads.
#[test]
fn live_info_fills_in_what_the_curated_tables_do_not_know() {
    let unknown = "openrouter/some/model-xyz";
    let curated = "openrouter/anthropic/claude-opus-4.7:live-test";
    let listed_price = LivePrice {
        input: 2.0,
        output: 8.0,
        cache_write: 2.5,
        cache_read: 0.2,
    };
    let usage = usage(1_000_000, 0, 0, 1_000_000);
    assert_eq!(estimate_cost_usd(unknown, &usage), None);
    assert_eq!(max_output_tokens(unknown), UNKNOWN_OUTPUT_TOKENS);

    replace_live_models(
        ModelBackend::OpenRouter,
        live_listing(unknown, curated, listed_price),
    );
    assert_cost(unknown, &usage, 10.0);
    assert_eq!(max_output_tokens(unknown), 16_000);
    assert_eq!(live_context_window(unknown), Some(262_144));
    // Curated prices win; OpenRouter's listed output limit wins; a zero
    // window is dropped.
    assert_cost(curated, &usage, 30.0);
    assert_eq!(max_output_tokens(curated), 20_000);
    assert_eq!(live_context_window(curated), None);
    assert_eq!(live_model_info("aura-live-test-model"), None);
    // A variant finds its base model's listing; the bare slug an
    // answer names is not looked up as an OpenRouter id.
    assert_cost("openrouter/acme/variant-test:nitro", &usage, 2.0);
    assert_cost("openrouter/deepseek/deepseek-v4.1-flash-1017", &usage, 10.0);
    assert_eq!(
        estimate_cost_usd("deepseek/deepseek-v4.1-flash-1017", &usage),
        None
    );
    assert_eq!(live_model_info("acme/variant-test:nitro"), None);

    // A later listing replaces the earlier one entirely.
    replace_live_models(ModelBackend::OpenRouter, []);
    assert_eq!(live_model_info(unknown), None);
    assert_eq!(estimate_cost_usd(unknown, &usage), None);
    assert_eq!(
        live_model_info("openrouter/deepseek/deepseek-v4.1-flash-1017"),
        None
    );
}
