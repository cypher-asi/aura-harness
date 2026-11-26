use crate::model_catalog::*;
use crate::Usage;

pub(super) fn usage(input: u64, cache_write: u64, cache_read: u64, output: u64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: Some(cache_write),
        cache_read_input_tokens: Some(cache_read),
    }
}

pub(super) fn assert_cost(model: &str, usage: &Usage, expected: f64) {
    let cost = estimate_cost_usd(model, usage).unwrap_or_else(|| panic!("no price: {model}"));
    assert!(
        (cost - expected).abs() < 1e-9,
        "{model}: {cost} != {expected}"
    );
}

pub(super) fn options_of(backend: ModelBackend) -> Vec<String> {
    catalog("local/qwen3:4b", &[])
        .into_iter()
        .filter(|o| o.backend == backend)
        .map(|o| o.id)
        .collect()
}
