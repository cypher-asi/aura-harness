//! The tokens and list-price cost per model of a run's model calls.

#[cfg(test)]
mod tests;

use crate::estimate::list_price;
use aura_core_types::ModelTokens;
use aura_model::model_catalog::billed_tokens;
use aura_model::Usage;
use std::collections::BTreeMap;

/// Tokens and list-price cost per model of `calls` (each a model call's
/// model and usage), most expensive first.
#[must_use]
pub fn tally(calls: impl IntoIterator<Item = (String, Usage)>) -> Vec<ModelTokens> {
    let mut by_model: BTreeMap<String, ModelTokens> = BTreeMap::new();
    for (model, usage) in calls {
        let billed = billed_tokens(&model, &usage);
        let cost_usd = list_price(&model, &usage).unwrap_or(0.0);
        let row = by_model
            .entry(model)
            .or_insert_with_key(|model| ModelTokens {
                model: model.clone(),
                ..ModelTokens::default()
            });
        row.calls += 1;
        row.input_tokens += billed.uncached_input + billed.cache_write + billed.cache_read;
        row.output_tokens += usage.output_tokens;
        row.cache_read_tokens += billed.cache_read;
        row.cache_write_tokens += billed.cache_write;
        row.cost_usd += cost_usd;
    }
    most_expensive_first(by_model.into_values().collect())
}

/// `rows` added up per model, most expensive first.
pub(crate) fn merge_tokens<'a>(
    rows: impl IntoIterator<Item = &'a ModelTokens>,
) -> Vec<ModelTokens> {
    let mut by_model: BTreeMap<&str, ModelTokens> = BTreeMap::new();
    for row in rows {
        let sum = by_model.entry(&row.model).or_insert_with(|| ModelTokens {
            model: row.model.clone(),
            ..ModelTokens::default()
        });
        sum.calls += row.calls;
        sum.input_tokens += row.input_tokens;
        sum.output_tokens += row.output_tokens;
        sum.cache_read_tokens += row.cache_read_tokens;
        sum.cache_write_tokens += row.cache_write_tokens;
        sum.cost_usd += row.cost_usd;
    }
    most_expensive_first(by_model.into_values().collect())
}

/// `rows`, one per model, most expensive first, then by model.
fn most_expensive_first(mut rows: Vec<ModelTokens>) -> Vec<ModelTokens> {
    rows.sort_unstable_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then_with(|| a.model.cmp(&b.model))
    });
    rows
}
