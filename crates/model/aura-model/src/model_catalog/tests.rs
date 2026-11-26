
use super::*;

#[test]
fn backend_follows_prefix() {
    assert_eq!(backend_for("local/qwen3:4b"), ModelBackend::Local);
    assert_eq!(backend_for("aura-claude-sonnet-4-6"), ModelBackend::Router);
    assert_eq!(backend_for("qwen3:4b"), ModelBackend::Router);
    assert_eq!(backend_for("claude/claude-opus-4-7"), ModelBackend::Claude);
    assert_eq!(backend_for("chatgpt/gpt-5.4"), ModelBackend::Chatgpt);
    assert_eq!(
        ModelBackend::Router.required_provider(),
        Some(ProviderKind::Zos)
    );
    assert_eq!(ModelBackend::Local.required_provider(), None);
}

#[test]
fn live_plan_models_replace_the_built_in_list() {
    let live = [ModelOption {
        id: "claude/claude-opus-5-5".to_string(),
        label: "Claude Opus 5.5".to_string(),
        backend: ModelBackend::Claude,
        installed: true,
        tier: ModelTier::Frontier,
    }];
    let options = catalog_with_plans("local/qwen3:4b", &[], &live);
    let claude: Vec<&str> = options
        .iter()
        .filter(|o| o.backend == ModelBackend::Claude)
        .map(|o| o.id.as_str())
        .collect();
    assert_eq!(claude, ["claude/claude-opus-5-5"]);
    assert!(options.iter().any(|o| o.backend == ModelBackend::Router));
    assert!(options.iter().any(|o| o.backend == ModelBackend::Chatgpt));
}

#[test]
fn tiers_are_inferred_from_the_model_name() {
    assert_eq!(infer_tier("local/qwen3:4b"), ModelTier::Local);
    assert_eq!(infer_tier("claude/claude-haiku-5-5"), ModelTier::Fast);
    assert_eq!(infer_tier("chatgpt/gpt-5.4-mini"), ModelTier::Fast);
    assert_eq!(infer_tier("claude/claude-sonnet-5-5"), ModelTier::Standard);
    assert_eq!(infer_tier("claude/claude-opus-5-5"), ModelTier::Frontier);
    assert_eq!(infer_tier("chatgpt/gpt-5.6"), ModelTier::Frontier);
    assert_eq!(infer_tier("aura-gpt-5-4"), ModelTier::Frontier);
    assert_eq!(infer_tier("some-new-model"), ModelTier::Standard);
    assert!(ModelTier::Local < ModelTier::Fast && ModelTier::Standard < ModelTier::Frontier);
}

#[test]
fn curated_tiers_match_inference() {
    for option in catalog("local/qwen3:4b", &[]) {
        assert_eq!(option.tier, infer_tier(&option.id), "{}", option.id);
    }
}

#[test]
fn model_key_is_the_same_on_every_route() {
    assert_eq!(model_key("aura-claude-opus-4-7"), "claude-opus-4-7");
    assert_eq!(model_key("claude/claude-opus-4-7"), "claude-opus-4-7");
    assert_eq!(model_key("chatgpt/gpt-5.4"), model_key("aura-gpt-5-4"));
}

#[test]
fn replies_may_be_as_long_as_each_api_allows() {
    for (model, limit) in [
        ("aura-claude-opus-5-5", 128_000),
        ("claude/claude-sonnet-4-6", 128_000),
        ("aura-claude-haiku-4-5", 64_000),
        ("claude-sonnet-4-20250514", 64_000),
        ("claude-opus-4-1", 32_000),
        ("chatgpt/gpt-5.4", 128_000),
        ("aura-gpt-4o", 16_384),
        ("aura-gemini-3-pro", 65_536),
        ("aura-kimi-k2", CONTEXT_BOUND_OUTPUT_TOKENS),
        ("some-new-model", UNKNOWN_OUTPUT_TOKENS),
        ("local/qwen3:4b", DEFAULT_LOCAL_MAX_OUTPUT_TOKENS),
    ] {
        assert_eq!(max_output_tokens(model), limit, "{model}");
    }
}

#[test]
fn upstream_name_strips_route_prefix() {
    assert_eq!(
        upstream_model_name("claude/claude-opus-4-7"),
        "claude-opus-4-7"
    );
    assert_eq!(upstream_model_name("chatgpt/gpt-5.4"), "gpt-5.4");
    assert_eq!(upstream_model_name("local/qwen3:4b"), "qwen3:4b");
    assert_eq!(upstream_model_name("aura-gpt-5-4"), "aura-gpt-5-4");
    assert_eq!(upstream_model_name("claude/"), "claude/");
}

#[test]
fn catalog_offers_both_routes_for_claude() {
    let options = catalog("local/qwen3:4b", &[]);
    assert!(options
        .iter()
        .any(|o| o.id == "aura-claude-opus-4-7" && o.backend == ModelBackend::Router));
    assert!(options
        .iter()
        .any(|o| o.id == "claude/claude-opus-4-7" && o.backend == ModelBackend::Claude));
    assert!(options.iter().any(|o| o.backend == ModelBackend::Chatgpt));
}

fn usage(input: u64, cache_write: u64, cache_read: u64, output: u64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: Some(cache_write),
        cache_read_input_tokens: Some(cache_read),
    }
}

fn assert_cost(model: &str, usage: &Usage, expected: f64) {
    let cost = estimate_cost_usd(model, usage).unwrap_or_else(|| panic!("no price: {model}"));
    assert!(
        (cost - expected).abs() < 1e-9,
        "{model}: {cost} != {expected}"
    );
}

#[test]
fn cost_is_priced_the_same_on_every_route() {
    let usage = usage(1_000, 2_000, 1_000_000, 1_000);
    for model in [
        "claude-opus-4-7",
        "claude/claude-opus-4-7",
        "aura-claude-opus-4-7",
    ] {
        assert_cost(model, &usage, 0.5425);
        assert_eq!(call_tokens(model, &usage), 1_004_000);
    }
}

#[test]
fn openai_cached_tokens_are_part_of_the_input() {
    let usage = usage(10_000, 0, 8_000, 1_000);
    for model in ["gpt-5.4", "chatgpt/gpt-5.4", "aura-gpt-5-4"] {
        assert_cost(model, &usage, 0.022);
        assert_eq!(call_tokens(model, &usage), 11_000);
    }
}

#[test]
fn dated_snapshots_share_their_model_price() {
    let usage = usage(1_000_000, 0, 0, 0);
    assert_cost("claude-haiku-4-5-20251001", &usage, 1.0);
    assert_cost("claude-opus-4-20250514", &usage, 15.0);
    assert_cost("gpt-5.4-2026-03-05", &usage, 2.5);
    assert_cost("claude-opus-5-5", &usage, 4.0);
}

#[test]
fn sonnet_5_5_and_opus_5_reason_high() {
    for model in [
        "claude/claude-sonnet-5-5",
        "claude-sonnet-5-5-20261001",
        "aura-claude-opus-5-5",
        "claude/claude-opus-5",
    ] {
        assert!(reasons_high(model), "{model}");
    }
    for model in [
        "claude/claude-sonnet-5",
        "claude/claude-haiku-5-5",
        "claude/claude-opus-4-8",
        "claude/claude-fable-5-2",
        "chatgpt/gpt-5.6-sol",
        "ollama/qwen3",
    ] {
        assert!(!reasons_high(model), "{model}");
    }
}

#[test]
fn max_effort_is_its_own_step_only_on_claude_models_whose_ladder_reaches_it() {
    for model in [
        "claude/claude-opus-5-5",
        "aura-claude-opus-4-8",
        "claude-sonnet-4-6",
        "claude/claude-sonnet-5-5",
        "claude/claude-fable-5-2",
    ] {
        assert!(has_max_effort(model), "{model}");
    }
    for model in [
        "claude/claude-haiku-4-5",
        "claude/claude-opus-4-1",
        "chatgpt/gpt-5.4",
        "local/qwen3:4b",
    ] {
        assert!(!has_max_effort(model), "{model}");
    }
}

#[test]
fn sonnet_and_opus_5_5_take_no_forced_tool_choice() {
    for model in [
        "claude/claude-sonnet-5-5",
        "claude-sonnet-5-5-20261001",
        "aura-claude-sonnet-5-5",
        "claude/claude-opus-5-5",
    ] {
        assert!(!takes_forced_tool_choice(model), "{model}");
    }
    for model in [
        "claude/claude-haiku-5-5",
        "claude/claude-sonnet-5",
        "claude/claude-sonnet-4-6",
        "aura-claude-opus-4-8",
        "chatgpt/gpt-5.4",
        "local/qwen3:4b",
    ] {
        assert!(takes_forced_tool_choice(model), "{model}");
    }
}

#[test]
fn a_later_version_that_costs_no_more_supersedes_the_earlier() {
    // Same family, later version, no token dearer: on any route.
    assert!(supersedes("claude-sonnet-5-5", "claude-sonnet-5"));
    assert!(supersedes(
        "claude/claude-sonnet-5-5",
        "claude/claude-sonnet-4-6"
    ));
    assert!(supersedes("claude-opus-5-5", "claude-opus-4-8"));
    assert!(supersedes("claude-opus-4-8", "claude-opus-4-7"));
    assert!(supersedes("claude-haiku-5-5", "claude-3-5-haiku"));
    assert!(supersedes("gpt-6-sol", "gpt-5.6-sol"));
    // Not the other way, nor a model itself or its dated snapshot.
    assert!(!supersedes("claude-sonnet-5", "claude-sonnet-5-5"));
    assert!(!supersedes("claude-sonnet-5-5", "claude-sonnet-5-5"));
    assert!(!supersedes("claude-haiku-4-5-20251001", "claude-haiku-4-5"));
    // A later version that costs more is a different trade.
    assert!(!supersedes("gpt-5.5", "gpt-5.4"));
    assert!(!supersedes("claude-haiku-4-5", "claude-3-5-haiku"));
    // Another family, whatever it costs.
    assert!(!supersedes("claude-haiku-5-5", "claude-sonnet-5"));
    assert!(!supersedes("gpt-5.4-mini", "gpt-5.2"));
    // Nothing is known of an unpriced or local model.
    assert!(!supersedes("claude-fable-5-2", "claude-fable-5-1"));
    assert!(!supersedes("local/qwen3:8b", "local/qwen3:4b"));
}

#[test]
fn local_and_unknown_models_have_no_cost() {
    let usage = usage(1_000, 0, 0, 1_000);
    assert_eq!(estimate_cost_usd("local/qwen3:4b", &usage), None);
    assert_eq!(estimate_cost_usd("qwen3:4b", &usage), None);
    assert_eq!(estimate_cost_usd("gpt-5-3", &usage), None);
    assert_eq!(estimate_cost_usd("claude-opus-9", &usage), None);
    assert_eq!(call_tokens("qwen3:4b", &usage), 2_000);
}

#[test]
fn local_model_name_strips_prefix() {
    assert_eq!(local_model_name("local/qwen3:4b"), Some("qwen3:4b"));
    assert_eq!(local_model_name("local/"), None);
    assert_eq!(local_model_name("claude-opus-4-7"), None);
}

#[test]
fn catalog_lists_default_local_first_and_dedupes() {
    let installed = vec!["qwen3:4b".to_string(), "llama3.2:3b".to_string()];
    let options = catalog("local/qwen3:4b", &installed);
    assert_eq!(options[0].id, "local/qwen3:4b");
    assert!(options[0].installed);
    assert_eq!(options[1].id, "local/llama3.2:3b");
    assert_eq!(
        options
            .iter()
            .filter(|o| o.backend == ModelBackend::Local)
            .count(),
        2
    );
    assert!(options
        .iter()
        .filter(|o| o.backend == ModelBackend::Router)
        .all(ModelOption::requires_login));
}

#[test]
fn catalog_marks_missing_default_as_not_installed() {
    let options = catalog("local/qwen3:4b", &[]);
    assert!(!options[0].installed);
}

#[test]
fn version_gate() {
    assert!(version_supports_messages_api("0.14.0"));
    assert!(version_supports_messages_api("0.15.2"));
    assert!(version_supports_messages_api("1.0.0"));
    assert!(!version_supports_messages_api("0.13.5"));
    assert!(version_supports_messages_api("dev"));
}

#[test]
fn installed_matches_latest_tag() {
    let installed = vec!["llama3.2:latest".to_string()];
    assert!(is_installed(&installed, "llama3.2"));
    assert!(!is_installed(&installed, "qwen3:4b"));
}

#[tokio::test]
async fn probe_unreachable_server() {
    let status = probe_local_server("http://127.0.0.1:9").await;
    assert_eq!(status, LocalServerStatus::Unreachable);
}

#[test]
fn openrouter_ids_route_to_openrouter_with_their_vendor_slug() {
    for (model, upstream, key, label) in [
        (
            "openrouter/anthropic/claude-sonnet-4.6",
            "anthropic/claude-sonnet-4.6",
            "claude-sonnet-4-6",
            "anthropic/claude-sonnet-4.6 (openrouter)",
        ),
        (
            "openrouter/openai/gpt-5.4",
            "openai/gpt-5.4",
            "gpt-5-4",
            "openai/gpt-5.4 (openrouter)",
        ),
        (
            "openrouter/anthropic/claude-haiku-4.5:nitro",
            "anthropic/claude-haiku-4.5:nitro",
            "claude-haiku-4-5",
            "anthropic/claude-haiku-4.5:nitro (openrouter)",
        ),
        (
            "openrouter/meta-llama/llama-4:free",
            "meta-llama/llama-4:free",
            "llama-4",
            "meta-llama/llama-4:free (openrouter)",
        ),
    ] {
        assert_eq!(backend_for(model), ModelBackend::OpenRouter, "{model}");
        assert_eq!(upstream_model_name(model), upstream, "{model}");
        assert_eq!(model_key(model), key, "{model}");
        assert_eq!(route_label(model), label, "{model}");
    }
    assert_eq!(upstream_model_name("openrouter/"), "openrouter/");
    assert_eq!(
        ModelBackend::OpenRouter.required_provider(),
        Some(ProviderKind::OpenRouter)
    );
    assert_eq!(
        model_key("openrouter/openai/gpt-5.4"),
        model_key("chatgpt/gpt-5.4")
    );
}

#[test]
fn source_label_names_who_pays() {
    for (backend, source) in [
        (ModelBackend::Local, "local"),
        (ModelBackend::Router, "aura-router"),
        (ModelBackend::Claude, "Claude plan"),
        (ModelBackend::Chatgpt, "ChatGPT plan"),
        (ModelBackend::OpenRouter, "OpenRouter"),
    ] {
        assert_eq!(backend.source_label(), source);
    }
}

#[test]
fn openrouter_models_share_curated_tiers_limits_and_prices() {
    assert_eq!(
        infer_tier("openrouter/anthropic/claude-haiku-4.5"),
        ModelTier::Fast
    );
    assert_eq!(
        infer_tier("openrouter/openai/gpt-5.4:nitro"),
        ModelTier::Frontier
    );
    assert_eq!(
        max_output_tokens("openrouter/anthropic/claude-haiku-4.5"),
        64_000
    );
    let usage = usage(1_000, 2_000, 1_000_000, 1_000);
    let direct = estimate_cost_usd("claude/claude-sonnet-4-6", &usage);
    assert!(direct.is_some());
    assert_eq!(
        estimate_cost_usd("openrouter/anthropic/claude-sonnet-4.6", &usage),
        direct
    );
    assert!(options_of(ModelBackend::OpenRouter)
        .contains(&"openrouter/anthropic/claude-sonnet-4.6".to_string()));
}

#[test]
fn openrouter_usage_is_anthropic_shaped_even_for_openai_models() {
    let usage = usage(10_000, 0, 8_000, 1_000);
    assert_eq!(call_tokens("openrouter/openai/gpt-5.4", &usage), 19_000);
    assert_eq!(call_tokens("chatgpt/gpt-5.4", &usage), 11_000);
}

#[test]
fn a_bare_vendor_slug_is_not_taken_for_an_openrouter_id() {
    // Local servers name models the way OpenRouter does
    // (`openai/gpt-oss-20b` on LM Studio), so a name with no route
    // prefix is not priced, keyed or shaped as OpenRouter's.
    let slug = "openai/gpt-6.1-sol";
    assert_eq!(backend_for(slug), ModelBackend::Router);
    assert_eq!(model_key(slug), "openai/gpt-6-1-sol");
    assert_ne!(model_key(slug), model_key("openrouter/openai/gpt-6.1-sol"));
    let usage = usage(10_000, 0, 8_000, 1_000);
    assert_eq!(estimate_cost_usd(slug, &usage), None);
    assert!(estimate_cost_usd("openrouter/openai/gpt-6.1-sol", &usage).is_some());

    // Router aliases and bare GPT names keep their OpenAI shape.
    assert_eq!(model_key("aura-gpt-5-4"), "gpt-5-4");
    assert_eq!(call_tokens("aura-gpt-5-4", &usage), 11_000);
    assert_eq!(call_tokens("gpt-6.1-sol", &usage), 11_000);
}

fn options_of(backend: ModelBackend) -> Vec<String> {
    catalog("local/qwen3:4b", &[])
        .into_iter()
        .filter(|o| o.backend == backend)
        .map(|o| o.id)
        .collect()
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
        ],
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
