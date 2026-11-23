//! The tokens and list-price cost per model of a run's model calls, its
//! own work apart from each purpose of the calls made beside it.

#[cfg(test)]
mod tests;

use crate::estimate::list_price;
use aura_core_types::ModelTokens;
use aura_model::model_catalog::{
    backend_for, billed_tokens, estimate_cost_usd, model_key, BilledTokens, ModelBackend,
    OPENROUTER_PREFIX,
};
use aura_model::{ModelCallPurpose, Usage};
use std::borrow::Cow;
use std::collections::BTreeMap;

/// One model call a run's record holds.
///
/// How it is priced follows the route it took, when the record kept it:
///
/// - a local route prices by the route alone, whatever the local server
///   named its model (LM Studio answers `openai/gpt-oss-20b`, Ollama
///   `user/model:tag`, names that look like OpenRouter slugs);
/// - an OpenRouter route prices the answer as the OpenRouter id it names
///   (OpenRouter answers with the bare `vendor/model` slug, without the
///   `openrouter/` prefix the request had), else by the route;
/// - any other route prices by the answer, else by the route.
///
/// Without a route, as in older records, only the answer prices it.
#[derive(Debug, Clone)]
pub struct ModelCall {
    /// The model that answered, as its provider named it; the call is
    /// counted under this name.
    pub model: String,
    /// The model id the call asked for, route prefix included
    /// (`openrouter/openai/gpt-6.1-sol`); `None` in records written
    /// before it was kept.
    pub route: Option<String>,
    /// What the call was made for beside the agent's own work, such as
    /// Gate writing a turn's acceptance criteria; `None` for the work
    /// itself.
    pub purpose: Option<ModelCallPurpose>,
    /// What the call used.
    pub usage: Usage,
}

impl ModelCall {
    /// The call's [`list_price`], by the model the route says names its
    /// price (see [`ModelCall`]). 0 for local routes and for a call that
    /// used no tokens (one that failed), `None` when no price is known.
    #[must_use]
    pub fn list_price(&self) -> Option<f64> {
        if used_nothing(&self.usage) {
            return Some(0.0);
        }
        self.priced_by(list_price)
    }

    /// The call's [`estimate_cost_usd`], by the model the route says
    /// names its price (see [`ModelCall`]). `None` for local routes and
    /// when no price is known.
    #[must_use]
    pub fn estimate_cost_usd(&self) -> Option<f64> {
        self.priced_by(estimate_cost_usd)
    }

    /// The model id of the endpoint that reported `usage`, which decides
    /// its shape: the route when the record kept it, else the model that
    /// answered.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        self.route.as_deref().unwrap_or(&self.model)
    }

    /// `usage` split the way it is billed, in the shape of the
    /// [`endpoint`](Self::endpoint) that reported it.
    #[must_use]
    pub fn billed_tokens(&self) -> BilledTokens {
        billed_tokens(self.endpoint(), &self.usage)
    }

    fn priced_by(&self, price: impl Fn(&str, &Usage) -> Option<f64>) -> Option<f64> {
        let by = |model: &str| price(model, &self.usage);
        let Some(route) = self.route.as_deref() else {
            return by(&self.model);
        };
        match backend_for(route) {
            ModelBackend::Local => by(route),
            ModelBackend::OpenRouter => openrouter_id(&self.model)
                .and_then(|id| by(&id))
                .or_else(|| by(route)),
            ModelBackend::Claude | ModelBackend::Chatgpt | ModelBackend::Router => {
                by(&self.model).or_else(|| by(route))
            }
        }
    }

    /// The name the call's answer goes by in price tables and rows: on
    /// an OpenRouter route, the OpenRouter id it names; else as it came.
    fn answer_id(&self) -> Cow<'_, str> {
        match self.route.as_deref().map(backend_for) {
            Some(ModelBackend::OpenRouter) => {
                openrouter_id(&self.model).unwrap_or(Cow::Borrowed(&self.model))
            }
            _ => Cow::Borrowed(&self.model),
        }
    }
}

/// The OpenRouter id an answer on an OpenRouter route names: itself when
/// it has the `openrouter/` prefix, `openrouter/<answer>` when it is a
/// bare `vendor/model[:variant]` slug (one `/` with a name on each
/// side), `None` for anything else.
fn openrouter_id(answer: &str) -> Option<Cow<'_, str>> {
    match backend_for(answer) {
        ModelBackend::OpenRouter => Some(Cow::Borrowed(answer)),
        ModelBackend::Router if is_vendor_slug(answer) => {
            Some(Cow::Owned(format!("{OPENROUTER_PREFIX}{answer}")))
        }
        _ => None,
    }
}

/// Whether `name` reads `vendor/model[:variant]`: one `/`, with a name
/// on each side.
fn is_vendor_slug(name: &str) -> bool {
    name.split_once('/').is_some_and(|(vendor, model)| {
        !vendor.trim().is_empty() && !model.trim().is_empty() && !model.contains('/')
    })
}

/// Whether `usage` counts no tokens of any kind, as a failed call's does.
fn used_nothing(usage: &Usage) -> bool {
    usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens.unwrap_or(0) == 0
        && usage.cache_read_input_tokens.unwrap_or(0) == 0
}

/// Tokens and list-price cost per model of `calls`, most expensive first:
/// a row for a model's calls for the agent's own work, and one for each
/// [`purpose`](ModelCall::purpose) its other calls were made for. A call
/// with no known price adds nothing to its row's cost and is counted in
/// its `unpriced_calls`; a local model's calls, and calls that used no
/// tokens, are priced, at 0.
#[must_use]
pub fn tally(calls: impl IntoIterator<Item = ModelCall>) -> Vec<ModelTokens> {
    let mut by_row: BTreeMap<(String, Option<&str>), ModelTokens> = BTreeMap::new();
    for call in calls {
        let billed = call.billed_tokens();
        let price = call.list_price();
        let purpose = call.purpose.map(ModelCallPurpose::as_str);
        let row = by_row
            .entry((row_name(&call).to_string(), purpose))
            .or_insert_with_key(|(model, purpose)| ModelTokens {
                model: model.clone(),
                purpose: purpose.map(str::to_string),
                ..ModelTokens::default()
            });
        row.calls += 1;
        row.input_tokens += billed.uncached_input + billed.cache_write + billed.cache_read;
        row.output_tokens += billed.output;
        row.cache_read_tokens += billed.cache_read;
        row.cache_write_tokens += billed.cache_write;
        match price {
            Some(cost_usd) => row.cost_usd += cost_usd,
            None => row.unpriced_calls += 1,
        }
    }
    most_expensive_first(by_row.into_values().collect())
}

/// The row `call` is counted in: the model asked for when the answer
/// names the same upstream model, else the model that answered. A failed
/// call is recorded under the model asked for
/// (`openrouter/openai/gpt-6.1-sol`) while answers carry the provider's
/// name (`openai/gpt-6.1-sol`); both belong in one row.
fn row_name(call: &ModelCall) -> &str {
    match call.route.as_deref() {
        Some(route) if model_key(route) == model_key(&call.answer_id()) => route,
        _ => &call.model,
    }
}

/// `rows` added up per model and purpose, most expensive first.
pub(crate) fn merge_tokens<'a>(
    rows: impl IntoIterator<Item = &'a ModelTokens>,
) -> Vec<ModelTokens> {
    let mut by_row: BTreeMap<(&str, Option<&str>), ModelTokens> = BTreeMap::new();
    for row in rows {
        let key = (row.model.as_str(), row.purpose.as_deref());
        let sum = by_row.entry(key).or_insert_with(|| ModelTokens {
            model: row.model.clone(),
            purpose: row.purpose.clone(),
            ..ModelTokens::default()
        });
        sum.calls += row.calls;
        sum.input_tokens += row.input_tokens;
        sum.output_tokens += row.output_tokens;
        sum.cache_read_tokens += row.cache_read_tokens;
        sum.cache_write_tokens += row.cache_write_tokens;
        sum.cost_usd += row.cost_usd;
        sum.unpriced_calls += row.unpriced_calls;
    }
    most_expensive_first(by_row.into_values().collect())
}

/// `rows`, one per model and purpose, most expensive first, then by
/// model, a model's own work before its other calls.
fn most_expensive_first(mut rows: Vec<ModelTokens>) -> Vec<ModelTokens> {
    rows.sort_unstable_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.purpose.cmp(&b.purpose))
    });
    rows
}
