//! The models Gate may route to.
//!
//! The candidates are the catalog models the user can run right now:
//! installed local models, and cloud models whose sign-in, plan or
//! OpenRouter key is active. OpenRouter lists hundreds of models; only
//! those with a curated list price are candidates. An upstream model
//! reachable on several routes is kept once, on the preferred one: a
//! linked plan (already paid for), then the cloud router, then
//! OpenRouter. Candidates are ordered by the list price of a typical
//! turn, local models first, and work goes only to those no later
//! version supersedes ([`current`]). An arm of `/compare` may also be
//! pinned to any model the user can run ([`runnable`]).

use std::collections::hash_map::{Entry, HashMap};

use aura_core_auth::ProviderKind;
use aura_model::model_catalog::{
    backend_for, estimate_cost_usd, infer_tier, model_key, route_label, supersedes, ModelBackend,
    ModelOption, ModelTier,
};
use aura_model::Usage;

use crate::cost::cheaper_first;

/// Tokens of a typical turn, for comparing list prices.
const REFERENCE_TURN: Usage = Usage {
    input_tokens: 20_000,
    output_tokens: 2_000,
    cache_creation_input_tokens: None,
    cache_read_input_tokens: None,
};

/// A model Gate may route a turn to.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// Full model id, route prefix included.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Route that serves it.
    pub backend: ModelBackend,
    /// How capable it is.
    pub tier: ModelTier,
    /// List price of a typical turn in USD: 0 for local models, `None`
    /// when the price is unknown.
    pub cost_usd: Option<f64>,
}

impl Candidate {
    fn from_option(option: &ModelOption) -> Self {
        Self {
            id: option.id.clone(),
            label: option.label.clone(),
            backend: option.backend,
            tier: option.tier,
            cost_usd: turn_price(&option.id, option.backend),
        }
    }

    /// A candidate for a model `id` the catalog did not list, such as an
    /// arm model a recorded comparison names: its tier, route and price
    /// inferred from the id, labelled with the id itself.
    #[must_use]
    pub fn from_id(id: &str) -> Self {
        let backend = backend_for(id);
        Self {
            id: id.to_string(),
            label: id.to_string(),
            backend,
            tier: infer_tier(id),
            cost_usd: turn_price(id, backend),
        }
    }

    /// The model and who serves it, e.g. `claude-sonnet-4-6 (claude plan)`.
    #[must_use]
    pub fn route_label(&self) -> String {
        route_label(&self.id)
    }
}

/// List price of a typical turn on `id`, served by `backend`: 0 for a
/// local model, `None` when the price is unknown.
fn turn_price(id: &str, backend: ModelBackend) -> Option<f64> {
    if backend == ModelBackend::Local {
        Some(0.0)
    } else {
        estimate_cost_usd(id, &REFERENCE_TURN)
    }
}

/// Which route serves an upstream model reachable on several, lower
/// first: a linked plan (already paid for), then the cloud router, then
/// OpenRouter (billed per call to the user's key), then anything else.
const fn route_rank(backend: ModelBackend) -> u8 {
    match backend {
        ModelBackend::Claude | ModelBackend::Chatgpt => 0,
        ModelBackend::Router => 1,
        ModelBackend::OpenRouter => 2,
        ModelBackend::Local => 3,
    }
}

/// Whether the upstream model `id` names has a curated list price.
///
/// The model is asked for in its router-alias form (`aura-<key>`): the
/// live price table is keyed by full OpenRouter ids, so the alias only
/// ever hits the curated table. OpenRouter lists hundreds of models;
/// Gate routes only to those it has a curated price, and so a tier, for.
fn has_curated_price(id: &str) -> bool {
    estimate_cost_usd(&format!("aura-{}", model_key(id)), &REFERENCE_TURN).is_some()
}

/// Whether Gate may route to `option` at all: OpenRouter options only
/// with a curated price ([`has_curated_price`]).
fn is_routable(option: &ModelOption) -> bool {
    option.backend != ModelBackend::OpenRouter || has_curated_price(&option.id)
}

/// The models Gate may route to, cheapest first: the `catalog` options
/// that are installed (local) or whose provider `can_use` allows,
/// leaving out those `dropped` says to and OpenRouter models without a
/// curated price, with one route per upstream model: a linked plan's
/// over the router's, the router's over OpenRouter's.
#[must_use]
pub fn candidates(
    catalog: &[ModelOption],
    can_use: impl Fn(ProviderKind) -> bool,
    dropped: impl Fn(&str) -> bool,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for option in usable(catalog, can_use).filter(|o| is_routable(o) && !dropped(&o.id)) {
        match by_key.entry(model_key(&option.id)) {
            Entry::Occupied(kept) => {
                let kept = &mut out[*kept.get()];
                if route_rank(option.backend) < route_rank(kept.backend) {
                    *kept = Candidate::from_option(option);
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(out.len());
                out.push(Candidate::from_option(option));
            }
        }
    }
    out.sort_by(|a, b| cheaper_first(a.cost_usd, b.cost_usd));
    out
}

/// Every model an arm of `/compare` may be pinned to, cheapest first:
/// the `catalog` options that are installed (local) or whose provider
/// `can_use` allows, leaving out those `dropped` says to. Unlike
/// [`candidates`], every route of a model is kept, and OpenRouter
/// models without a curated price are kept too.
#[must_use]
pub fn runnable(
    catalog: &[ModelOption],
    can_use: impl Fn(ProviderKind) -> bool,
    dropped: impl Fn(&str) -> bool,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = usable(catalog, can_use)
        .filter(|o| !dropped(&o.id))
        .map(Candidate::from_option)
        .collect();
    out.sort_by(|a, b| cheaper_first(a.cost_usd, b.cost_usd));
    out
}

/// The `catalog` options the user can run: local ones when installed,
/// others when `can_use` allows their provider.
fn usable(
    catalog: &[ModelOption],
    can_use: impl Fn(ProviderKind) -> bool,
) -> impl Iterator<Item = &ModelOption> {
    catalog.iter().filter(move |o| match o.required_provider() {
        None => o.installed,
        Some(provider) => can_use(provider),
    })
}

/// Whether none of `candidates` supersedes `candidate`: none is a later
/// version of the same model on the same route that costs no more
/// ([`supersedes`]).
pub(crate) fn is_current(candidate: &Candidate, candidates: &[Candidate]) -> bool {
    !candidates
        .iter()
        .any(|other| other.backend == candidate.backend && supersedes(&other.id, &candidate.id))
}

/// The `candidates` Gate sends work to: every one but those a later
/// version supersedes ([`supersedes`]). An older version is priced like
/// the newer one or above it and belongs to the same tier, so to Gate it
/// looks like a model nothing is known against, and it would get the
/// work whenever the newer one's outcomes ruled that one out. It stays
/// a candidate for what names a model outright, such as an arm of
/// `/compare`.
#[must_use]
pub fn current(candidates: &[Candidate]) -> Vec<&Candidate> {
    candidates
        .iter()
        .filter(|c| is_current(c, candidates))
        .collect()
}

#[cfg(test)]
mod tests;
