//! The models Gate may route to.
//!
//! The candidates are the catalog models the user can run right now:
//! installed local models, and cloud models whose sign-in or plan is
//! active. An upstream model reachable on two routes (a linked plan and
//! the cloud router) is kept once, on the plan, since the plan is
//! already paid for. Candidates are ordered by the list price of a
//! typical turn, local models first, and work goes only to those no
//! later version supersedes ([`current`]).

use std::collections::hash_map::{Entry, HashMap};

use aura_core_auth::ProviderKind;
use aura_model::model_catalog::{
    estimate_cost_usd, model_key, route_label, supersedes, ModelBackend, ModelOption, ModelTier,
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
        let cost_usd = if option.backend == ModelBackend::Local {
            Some(0.0)
        } else {
            estimate_cost_usd(&option.id, &REFERENCE_TURN)
        };
        Self {
            id: option.id.clone(),
            label: option.label.clone(),
            backend: option.backend,
            tier: option.tier,
            cost_usd,
        }
    }

    /// The model and who serves it, e.g. `claude-sonnet-4-6 (claude plan)`.
    #[must_use]
    pub fn route_label(&self) -> String {
        route_label(&self.id)
    }
}

/// Whether `backend` is a plan the user linked, which is already paid for.
const fn is_plan(backend: ModelBackend) -> bool {
    matches!(backend, ModelBackend::Claude | ModelBackend::Chatgpt)
}

/// The models Gate may route to, cheapest first: the `catalog` options
/// that are installed (local) or whose provider `can_use` allows,
/// leaving out those `dropped` says to, with one route per upstream
/// model.
#[must_use]
pub fn candidates(
    catalog: &[ModelOption],
    can_use: impl Fn(ProviderKind) -> bool,
    dropped: impl Fn(&str) -> bool,
) -> Vec<Candidate> {
    let usable = catalog.iter().filter(|o| match o.required_provider() {
        None => o.installed,
        Some(provider) => can_use(provider),
    });
    let mut out: Vec<Candidate> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for option in usable.filter(|o| !dropped(&o.id)) {
        match by_key.entry(model_key(&option.id)) {
            Entry::Occupied(kept) => {
                let kept = &mut out[*kept.get()];
                if is_plan(option.backend) && !is_plan(kept.backend) {
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
