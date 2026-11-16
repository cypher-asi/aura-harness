//! Gate's candidates and the model it picks for a turn.
//!
//! The candidates are the catalog models the user can run right now:
//! installed local models, and cloud models whose sign-in or plan is
//! active. An upstream model reachable on two routes (a linked plan and
//! the cloud router) is kept once, on the plan, since the plan is
//! already paid for. Candidates are ordered by the list price of a
//! typical turn, local models first.

use std::collections::HashMap;

use aura_core_auth::ProviderKind;
use aura_model_reasoner::model_catalog::{
    estimate_cost_usd, model_key, route_label, ModelBackend, ModelOption, ModelTier,
};
use aura_model_reasoner::Usage;

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
    let mut out: Vec<Candidate> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let usable = catalog.iter().filter(|o| match o.required_provider() {
        None => o.installed,
        Some(provider) => can_use(provider),
    });
    for option in usable.filter(|o| !dropped(&o.id)) {
        let candidate = Candidate::from_option(option);
        match by_key.get(&model_key(&option.id)) {
            Some(&i) => {
                if is_plan(candidate.backend) && !is_plan(out[i].backend) {
                    out[i] = candidate;
                }
            }
            None => {
                by_key.insert(model_key(&option.id), out.len());
                out.push(candidate);
            }
        }
    }
    out.sort_by(|a, b| match (a.cost_usd, b.cost_usd) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    out
}

/// The model every turn goes to until Gate learns per category: the
/// cheapest standard-tier candidate, else the strongest one (the cheapest
/// of that tier). `candidates` must be ordered as [`candidates`] returns
/// them.
#[must_use]
pub fn fixed_pick(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates
        .iter()
        .find(|c| c.tier == ModelTier::Standard)
        .or_else(|| candidates.iter().rev().max_by_key(|c| c.tier))
}

/// The order [`fixed_pick`] goes through `candidates` as each pick is
/// dropped.
#[must_use]
pub fn pick_order(candidates: &[Candidate]) -> Vec<&Candidate> {
    let mut left: Vec<Candidate> = candidates.to_vec();
    let mut order = Vec::with_capacity(left.len());
    while let Some(pick) = fixed_pick(&left).map(|c| c.id.clone()) {
        if let Some(c) = candidates.iter().find(|c| c.id == pick) {
            order.push(c);
        }
        left.retain(|c| c.id != pick);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_model_reasoner::model_catalog::{catalog, catalog_with_plans};

    const LOCAL: &str = "local/qwen3:4b";

    fn ids(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.id.as_str()).collect()
    }

    fn signed_in(providers: &[ProviderKind]) -> impl Fn(ProviderKind) -> bool + '_ {
        move |p| providers.contains(&p)
    }

    fn with_local() -> Vec<ModelOption> {
        catalog(LOCAL, &["qwen3:4b".to_string()])
    }

    #[test]
    fn nothing_signed_in_routes_among_installed_local_models() {
        let none = signed_in(&[]);
        assert!(candidates(&catalog(LOCAL, &[]), &none, |_| false).is_empty());
        let local = candidates(&with_local(), &none, |_| false);
        assert_eq!(ids(&local), [LOCAL]);
        assert_eq!(local[0].cost_usd, Some(0.0));
        assert_eq!(fixed_pick(&local).map(|c| c.id.as_str()), Some(LOCAL));
    }

    #[test]
    fn zos_sign_in_adds_the_router_aliases() {
        let zos = candidates(&with_local(), signed_in(&[ProviderKind::Zos]), |_| false);
        assert_eq!(
            ids(&zos),
            [
                LOCAL,
                "aura-claude-haiku-4-5",
                "aura-gpt-5-4",
                "aura-claude-sonnet-4-6",
                "aura-claude-opus-4-7",
            ]
        );
        assert_eq!(
            fixed_pick(&zos).map(Candidate::route_label).as_deref(),
            Some("aura-claude-sonnet-4-6 (cloud)")
        );
    }

    #[test]
    fn a_claude_plan_adds_only_its_models() {
        let claude = candidates(
            &catalog(LOCAL, &[]),
            signed_in(&[ProviderKind::Claude]),
            |_| false,
        );
        assert_eq!(
            ids(&claude),
            [
                "claude/claude-haiku-4-5",
                "claude/claude-sonnet-4-6",
                "claude/claude-opus-4-7",
            ]
        );
        assert_eq!(
            fixed_pick(&claude).map(Candidate::route_label).as_deref(),
            Some("claude-sonnet-4-6 (claude plan)")
        );
    }

    #[test]
    fn every_sign_in_keeps_one_route_per_model_on_the_plan() {
        let all = candidates(
            &with_local(),
            signed_in(&[
                ProviderKind::Zos,
                ProviderKind::Claude,
                ProviderKind::Chatgpt,
            ]),
            |_| false,
        );
        assert_eq!(
            ids(&all),
            [
                LOCAL,
                "claude/claude-haiku-4-5",
                "chatgpt/gpt-5.4",
                "claude/claude-sonnet-4-6",
                "claude/claude-opus-4-7",
            ]
        );
        assert_eq!(
            fixed_pick(&all).map(|c| c.id.as_str()),
            Some("claude/claude-sonnet-4-6")
        );
    }

    #[test]
    fn plan_models_outside_the_router_list_are_kept_beside_it() {
        let live = [ModelOption {
            id: "claude/claude-sonnet-5-5".to_string(),
            label: "Claude Sonnet 5.5".to_string(),
            backend: ModelBackend::Claude,
            installed: true,
            tier: ModelTier::Standard,
        }];
        let options = catalog_with_plans(LOCAL, &[], &live);
        let both = candidates(
            &options,
            signed_in(&[ProviderKind::Zos, ProviderKind::Claude]),
            |_| false,
        );
        assert!(both.iter().any(|c| c.id == "aura-claude-sonnet-4-6"));
        assert_eq!(
            fixed_pick(&both).map(|c| c.id.as_str()),
            Some("claude/claude-sonnet-5-5"),
            "the cheaper standard model wins"
        );
    }

    #[test]
    fn without_a_standard_model_the_strongest_is_picked() {
        let chatgpt = candidates(&with_local(), signed_in(&[ProviderKind::Chatgpt]), |_| {
            false
        });
        assert_eq!(
            fixed_pick(&chatgpt).map(|c| c.id.as_str()),
            Some("chatgpt/gpt-5.4")
        );
    }

    #[test]
    fn a_dropped_model_is_not_a_candidate() {
        let claude = signed_in(&[ProviderKind::Claude]);
        let dropped = |id: &str| id == "claude/claude-sonnet-4-6";
        let left = candidates(&with_local(), &claude, dropped);
        assert!(!left.iter().any(|c| dropped(&c.id)));
        assert_eq!(
            fixed_pick(&left).map(|c| c.id.as_str()),
            Some("claude/claude-opus-4-7")
        );
    }

    #[test]
    fn picks_go_down_the_order_as_each_is_dropped() {
        let all = candidates(
            &with_local(),
            signed_in(&[
                ProviderKind::Zos,
                ProviderKind::Claude,
                ProviderKind::Chatgpt,
            ]),
            |_| false,
        );
        let order: Vec<&str> = pick_order(&all).iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            order,
            [
                "claude/claude-sonnet-4-6",
                "chatgpt/gpt-5.4",
                "claude/claude-opus-4-7",
                "claude/claude-haiku-4-5",
                LOCAL,
            ]
        );
    }
}
