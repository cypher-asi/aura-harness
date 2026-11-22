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

#[test]
fn tally_adds_up_tokens_and_cost_per_model() {
    let rows = tally([
        (
            "claude-haiku-4-5".to_string(),
            usage(1_000, 10, None, Some(0)),
        ),
        (
            "claude-opus-4-7".to_string(),
            usage(10_000, 500, None, Some(2_000)),
        ),
        (
            "claude-opus-4-7".to_string(),
            usage(20_000, 500, None, Some(0)),
        ),
        (
            "local/qwen3:4b".to_string(),
            usage(5_000, 100, None, Some(0)),
        ),
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
        ("claude-sonnet-5-5".to_string(), anthropic),
        ("gpt-5.4".to_string(), openai),
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

/// A model called twice, a local model, a model with no known price, and
/// both ways providers report the cache.
fn every_kind_of_call() -> Vec<(String, Usage)> {
    let calls = [
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
    ];
    calls
        .into_iter()
        .map(|(model, usage)| (model.to_string(), usage))
        .collect()
}

#[test]
fn tallies_are_exactly_what_they_were() {
    let rows = tally(every_kind_of_call());
    // (model, calls, [input, cache read, cache write, output], cost)
    let rows: Vec<_> = rows
        .iter()
        .map(|r| {
            let tokens = [
                r.input_tokens,
                r.cache_read_tokens,
                r.cache_write_tokens,
                r.output_tokens,
            ];
            (r.model.as_str(), r.calls, tokens, r.cost_usd)
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("claude-opus-4-7", 2, [32_000, 2_000, 0, 1_000], 0.176),
            (
                "claude-sonnet-5-5",
                1,
                [27_289, 17_888, 9_391, 390],
                0.0291863
            ),
            ("gpt-5.4", 1, [1_000, 800, 0, 50], 0.00145),
            ("claude-haiku-4-5", 1, [1_000, 0, 0, 10], 0.00105),
            ("local/qwen3:4b", 1, [5_000, 0, 0, 100], 0.0),
            ("mystery-model", 1, [100, 0, 0, 10], 0.0),
        ]
    );
}
