use super::*;
use aura_model::model_catalog::estimate_cost_usd;

fn usage(input: u64, output: u64, write: Option<u64>, read: Option<u64>) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: write,
        cache_read_input_tokens: read,
    }
}

/// A call to `model` whose record keeps no route, as older ones don't.
fn call(model: &str, usage: Usage) -> ModelCall {
    ModelCall {
        model: model.to_string(),
        route: None,
        usage,
    }
}

/// A call that asked for `route` and was answered by `model`.
fn routed(model: &str, route: &str, usage: Usage) -> ModelCall {
    ModelCall {
        route: Some(route.to_string()),
        ..call(model, usage)
    }
}

#[test]
fn tally_adds_up_tokens_and_cost_per_model() {
    let rows = tally([
        call("claude-haiku-4-5", usage(1_000, 10, None, Some(0))),
        call("claude-opus-4-7", usage(10_000, 500, None, Some(2_000))),
        call("claude-opus-4-7", usage(20_000, 500, None, Some(0))),
        call("local/qwen3:4b", usage(5_000, 100, None, Some(0))),
    ]);
    let models: Vec<&str> = rows.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(
        models,
        ["claude-opus-4-7", "claude-haiku-4-5", "local/qwen3:4b"],
        "most expensive first"
    );
    assert_eq!(rows[0].calls, 2);
    assert_eq!(
        rows[0].input_tokens, 32_000,
        "cache reads are prompt tokens"
    );
    assert_eq!(rows[0].output_tokens, 1_000);
    assert_eq!(rows[0].cache_read_tokens, 2_000);
    let expected = estimate_cost_usd("claude-opus-4-7", &usage(10_000, 500, None, Some(2_000)))
        .unwrap()
        + estimate_cost_usd("claude-opus-4-7", &usage(20_000, 500, None, Some(0))).unwrap();
    assert!((rows[0].cost_usd - expected).abs() < 1e-12);
    assert!(
        rows[2].cost_usd.abs() < f64::EPSILON,
        "local models are free"
    );

    let merged = merge_tokens(rows.iter().chain(rows.iter()));
    assert_eq!(merged[0].calls, 4);
    assert!((merged[0].cost_usd - 2.0 * expected).abs() < 1e-12);
}

#[test]
fn tally_counts_the_whole_prompt_however_the_provider_reports_cache() {
    let anthropic = usage(10, 390, Some(9_391), Some(17_888));
    let openai = usage(1_000, 50, None, Some(800));
    let rows = tally([
        call("claude-sonnet-5-5", anthropic),
        call("gpt-5.4", openai),
    ]);
    let row = |model: &str| rows.iter().find(|r| r.model == model).unwrap();
    let sonnet = row("claude-sonnet-5-5");
    assert_eq!(sonnet.input_tokens, 10 + 9_391 + 17_888);
    assert_eq!(
        (sonnet.cache_read_tokens, sonnet.cache_write_tokens),
        (17_888, 9_391)
    );
    let gpt = row("gpt-5.4");
    assert_eq!(gpt.input_tokens, 1_000, "OpenAI counts cache reads inside");
    assert_eq!(gpt.cache_read_tokens, 800);
}

#[test]
fn a_call_the_answering_model_cannot_price_is_priced_by_its_route() {
    let tokens = || usage(10_000, 1_000, None, Some(8_000));
    let route = "openrouter/openai/gpt-6.1-sol";
    let by_route = estimate_cost_usd(route, &tokens()).unwrap();
    let rows = tally([
        routed("echoed-mystery-name", route, tokens()),
        // No route to fall back on: still unpriced.
        call("unrouted-mystery-name", tokens()),
    ]);
    let row = |model: &str| rows.iter().find(|r| r.model == model).unwrap();
    let echoed = row("echoed-mystery-name");
    assert!(
        (echoed.cost_usd - by_route).abs() < 1e-12,
        "{} != {by_route}",
        echoed.cost_usd
    );
    assert_eq!(echoed.calls, 1, "counted under the name that answered");
    assert_eq!(echoed.unpriced_calls, 0);
    let unrouted = row("unrouted-mystery-name");
    assert!(unrouted.cost_usd.abs() < f64::EPSILON);
    assert_eq!(unrouted.unpriced_calls, 1, "an unknown price is not free");
}

#[test]
fn tally_counts_calls_with_no_known_price_apart_from_free_ones() {
    let tokens = || usage(10_000, 1_000, None, Some(0));
    let rows = tally([
        call("claude-opus-4-7", tokens()),
        call("mystery-model", tokens()),
        call("mystery-model", tokens()),
        call("local/qwen3:4b", tokens()),
    ]);
    let row = |model: &str| rows.iter().find(|r| r.model == model).unwrap();
    let unpriced = |model: &str| row(model).unpriced_calls;
    assert_eq!(unpriced("claude-opus-4-7"), 0);
    assert_eq!(unpriced("mystery-model"), 2);
    assert_eq!(
        unpriced("local/qwen3:4b"),
        0,
        "local models are priced at 0"
    );
    assert!(row("mystery-model").is_incomplete());
    assert!(!row("local/qwen3:4b").is_incomplete());

    let merged = merge_tokens(rows.iter().chain(rows.iter()));
    let mystery = merged.iter().find(|r| r.model == "mystery-model").unwrap();
    assert_eq!((mystery.calls, mystery.unpriced_calls), (4, 4));
}

#[test]
fn the_answering_model_s_price_wins_over_the_route_s() {
    let tokens = || usage(1_000_000, 0, None, None);
    let haiku = routed("claude-haiku-4-5", "claude/claude-opus-4-7", tokens());
    assert_eq!(
        haiku.list_price(),
        estimate_cost_usd("claude-haiku-4-5", &tokens())
    );
    // OpenRouter's bare slug for the answering model is priced itself.
    let slug = routed(
        "openai/gpt-6.1-sol",
        "openrouter/openai/gpt-6.1-sol",
        tokens(),
    );
    assert_eq!(slug.list_price(), Some(2.0));
    assert_eq!(slug.estimate_cost_usd(), Some(2.0));
    // A local route is free to `list_price`, unpriced to the estimate.
    let local = routed("qwen3:4b", "local/qwen3:4b", tokens());
    assert_eq!(local.list_price(), Some(0.0));
    assert_eq!(local.estimate_cost_usd(), None);
}

// The live table is process-wide and tests run in parallel, so this is
// the only test of this crate that writes it.
#[test]
fn the_route_decides_whether_a_slug_is_openrouter_s() {
    use aura_model::model_catalog::{replace_live_models, LiveModelInfo, LivePrice};
    let listed = |id: &str| {
        (
            id.to_string(),
            LiveModelInfo {
                context_window: None,
                max_output: None,
                price: Some(LivePrice {
                    input: 1.0,
                    output: 1.0,
                    cache_write: 1.0,
                    cache_read: 1.0,
                }),
            },
        )
    };
    replace_live_models(
        ModelBackend::OpenRouter,
        [
            listed("openrouter/openai/gpt-oss-20b"),
            listed("openrouter/qwen/qwen3-coder-30b"),
        ],
    );
    let tokens = || usage(1_000_000, 0, None, None);

    // LM Studio names its models like OpenRouter slugs: a local call is
    // still free, however its answer reads.
    let local = routed("openai/gpt-oss-20b", "local/openai/gpt-oss-20b", tokens());
    assert_eq!(local.list_price(), Some(0.0));
    assert_eq!(local.estimate_cost_usd(), None);
    let lm_studio = routed("qwen/qwen3-coder-30b", "local/qwen3-coder", tokens());
    assert_eq!(lm_studio.list_price(), Some(0.0));
    let rows = tally([local, lm_studio]);
    assert!(rows
        .iter()
        .all(|row| row.cost_usd.abs() < f64::EPSILON && row.unpriced_calls == 0));
    assert_eq!(rows[0].model, "local/openai/gpt-oss-20b");

    // On an OpenRouter route the same slug is OpenRouter's, and priced.
    let routed_slug = routed(
        "openai/gpt-oss-20b",
        "openrouter/openai/gpt-oss-20b:nitro",
        tokens(),
    );
    assert_eq!(routed_slug.list_price(), Some(1.0));
    assert_eq!(routed_slug.estimate_cost_usd(), Some(1.0));
    // Without a route a slug is not taken for OpenRouter's.
    assert_eq!(call("openai/gpt-oss-20b", tokens()).list_price(), None);

    replace_live_models(ModelBackend::OpenRouter, []);
}

#[test]
fn a_call_is_shaped_by_the_endpoint_it_went_to() {
    // OpenRouter reports Anthropic-shaped usage even for a GPT model:
    // cache reads come on top of `input_tokens`.
    let tokens = || usage(1_000, 50, None, Some(800));
    let rows = tally([
        routed("openai/gpt-5.4", "openrouter/openai/gpt-5.4", tokens()),
        routed("gpt-5.4", "chatgpt/gpt-5.4", tokens()),
    ]);
    let row = |model: &str| rows.iter().find(|r| r.model == model).unwrap();
    assert_eq!(row("openrouter/openai/gpt-5.4").input_tokens, 1_800);
    assert_eq!(row("chatgpt/gpt-5.4").input_tokens, 1_000);
}

#[test]
fn a_failed_call_costs_nothing_even_on_a_route_with_no_price() {
    let route = "openrouter/acme/unlisted-test";
    let failed = routed(route, route, usage(0, 0, None, None));
    assert_eq!(failed.list_price(), Some(0.0));
    let rows = tally([failed, routed(route, route, usage(0, 0, Some(0), Some(0)))]);
    assert_eq!((rows[0].calls, rows[0].unpriced_calls), (2, 0));
    assert!(!rows[0].is_incomplete(), "a failed call is no `+?`");

    // A call that used tokens on it is still unpriced.
    let used = tally([routed(route, route, usage(10, 1, None, None))]);
    assert_eq!(used[0].unpriced_calls, 1);
}

/// A model called twice, a local model, a model with no known price, and
/// both ways providers report the cache.
fn every_kind_of_call() -> Vec<ModelCall> {
    [
        ("claude-haiku-4-5", usage(1_000, 10, None, Some(0))),
        ("claude-opus-4-7", usage(10_000, 500, None, Some(2_000))),
        ("claude-opus-4-7", usage(20_000, 500, None, Some(0))),
        ("local/qwen3:4b", usage(5_000, 100, None, Some(0))),
        ("mystery-model", usage(100, 10, None, None)),
        ("gpt-5.4", usage(1_000, 50, None, Some(800))),
        (
            "claude-sonnet-5-5",
            usage(10, 390, Some(9_391), Some(17_888)),
        ),
    ]
    .into_iter()
    .map(|(model, usage)| call(model, usage))
    .collect()
}

#[test]
fn tallies_are_exactly_what_they_were() {
    let rows = tally(every_kind_of_call());
    // (model, calls, [input, cache read, cache write, output], cost, unpriced)
    let rows: Vec<_> = rows
        .iter()
        .map(|r| {
            let tokens = [
                r.input_tokens,
                r.cache_read_tokens,
                r.cache_write_tokens,
                r.output_tokens,
            ];
            (
                r.model.as_str(),
                r.calls,
                tokens,
                r.cost_usd,
                r.unpriced_calls,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("claude-opus-4-7", 2, [32_000, 2_000, 0, 1_000], 0.176, 0),
            (
                "claude-sonnet-5-5",
                1,
                [27_289, 17_888, 9_391, 390],
                0.0291863,
                0
            ),
            ("gpt-5.4", 1, [1_000, 800, 0, 50], 0.00145, 0),
            ("claude-haiku-4-5", 1, [1_000, 0, 0, 10], 0.00105, 0),
            ("local/qwen3:4b", 1, [5_000, 0, 0, 100], 0.0, 0),
            ("mystery-model", 1, [100, 0, 0, 10], 0.0, 1),
        ]
    );
}

#[test]
fn a_failed_call_shares_the_row_of_its_model_s_answers() {
    let route = "openrouter/openai/gpt-6.1-sol";
    let rows = tally([
        routed(
            "openai/gpt-6.1-sol",
            route,
            usage(1_000, 100, None, Some(0)),
        ),
        routed(
            "openai/gpt-6.1-sol",
            route,
            usage(2_000, 200, None, Some(0)),
        ),
        // A failed call is recorded under the model asked for, no tokens.
        routed(route, route, usage(0, 0, None, None)),
        // An answer from another model keeps its own row.
        routed(
            "claude-haiku-4-5",
            "claude/claude-opus-4-7",
            usage(10, 1, None, None),
        ),
    ]);
    let models: Vec<&str> = rows.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(models, [route, "claude-haiku-4-5"]);
    assert_eq!((rows[0].calls, rows[0].unpriced_calls), (3, 0));
    assert_eq!((rows[0].input_tokens, rows[0].output_tokens), (3_000, 300));
}
