//! Gate's candidates and the model it picks for a turn or subtask.
//!
//! The candidates are the catalog models the user can run right now:
//! installed local models, and cloud models whose sign-in or plan is
//! active. An upstream model reachable on two routes (a linked plan and
//! the cloud router) is kept once, on the plan, since the plan is
//! already paid for. Candidates are ordered by the list price of a
//! typical turn, local models first.
//!
//! [`pick`] chooses among them for one category: the cheapest model that
//! has proven itself there, else the category's default, and now and
//! then the next cheaper model instead, to learn how it does. When a
//! subtask's attempt fails, [`escalate`] picks the one model it is
//! retried on.

use std::collections::HashMap;

use aura_config::GateConfig;
use aura_core_auth::ProviderKind;
use aura_core_types::{PromptCategory, Route, RouteLevel, RouteReason};
use aura_model::model_catalog::{
    estimate_cost_usd, infer_tier, model_key, route_label, ModelBackend, ModelOption, ModelTier,
};
use aura_model::Usage;

use crate::stats::GateStats;

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

/// Whether `category` defaults to the strongest model: work whose
/// mistakes cost the most to find and undo.
#[must_use]
pub const fn is_heavy(category: PromptCategory) -> bool {
    matches!(
        category,
        PromptCategory::MultiFileChange | PromptCategory::Debugging | PromptCategory::Planning
    )
}

/// Where Gate sends a piece of work, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The model it runs on.
    pub model: Candidate,
    /// Why: [`RouteReason::Proven`], [`RouteReason::Default`] or
    /// [`RouteReason::Explore`].
    pub reason: RouteReason,
    /// Whether Gate explored instead of taking its pick.
    pub explored: bool,
    /// Probability that Gate's policy makes this choice.
    pub propensity: f64,
}

impl Decision {
    /// The decision as recorded on the prompt of work at `level`.
    #[must_use]
    pub fn route(&self, level: RouteLevel) -> Route {
        Route {
            level,
            model: self.model.id.clone(),
            reason: self.reason,
            explored: self.explored,
            propensity: self.propensity,
        }
    }
}

/// Source of the random draws exploration needs.
pub trait Random {
    /// A number in `[0, 1)`.
    fn unit(&mut self) -> f64;
}

impl<F: FnMut() -> f64> Random for F {
    fn unit(&mut self) -> f64 {
        self()
    }
}

/// A small, fast generator (SplitMix64); plenty for deciding when to
/// explore.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// A generator that always gives the same draws for `seed`.
    #[must_use]
    pub const fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    /// A generator seeded from the clock.
    #[must_use]
    pub fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        // Only the low bits vary between runs; folding the high half in
        // keeps the whole value.
        let folded = (nanos ^ (nanos >> 64)) & u128::from(u64::MAX);
        Self(u64::try_from(folded).unwrap_or_default())
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Random for SplitMix64 {
    fn unit(&mut self) -> f64 {
        // 52 random mantissa bits under exponent 0 give [1, 2).
        f64::from_bits(0x3FF0_0000_0000_0000 | (self.next_u64() >> 12)) - 1.0
    }
}

/// `candidates` ordered by what work at `level` is expected to cost on
/// each: local models first (they cost nothing), then by the mean cost
/// observed at `level`, or for a model not tried there yet, its list
/// price scaled by how observed costs compare with list prices for the
/// models that were. Models with no price known come last; ties keep the
/// order of `candidates`.
#[must_use]
pub fn by_expected_cost<'a>(
    stats: &GateStats,
    level: RouteLevel,
    candidates: &'a [Candidate],
) -> Vec<&'a Candidate> {
    let observed = |c: &Candidate| stats.mean_cost_usd(level, &c.id);
    let (seen, listed) = candidates
        .iter()
        .filter(|c| c.backend != ModelBackend::Local)
        .filter_map(|c| Some((observed(c)?, c.cost_usd.filter(|&p| p > 0.0)?)))
        .fold((0.0, 0.0), |(seen, listed), (o, l)| (seen + o, listed + l));
    let scale = if seen > 0.0 && listed > 0.0 {
        seen / listed
    } else {
        1.0
    };
    let expected = |c: &Candidate| {
        if c.backend == ModelBackend::Local {
            Some(0.0)
        } else {
            observed(c).or_else(|| c.cost_usd.map(|p| p * scale))
        }
    };
    let mut order: Vec<(&Candidate, Option<f64>)> =
        candidates.iter().map(|c| (c, expected(c))).collect();
    order.sort_by(|a, b| match (a.1, b.1) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    order.into_iter().map(|(c, _)| c).collect()
}

/// Where work at `level` asking for `category` goes, among `candidates`
/// (as [`candidates`] returns them), given what `stats` has learned:
///
/// 1. the cheapest model with at least `min_tries` counted outcomes on
///    the category and a success rate of at least `target`;
/// 2. else the category's default: the strongest model for heavy
///    categories ([`is_heavy`]), the cheapest standard-tier model for the
///    rest, falling back to the strongest;
/// 3. with probability `explore_rate`, the next cheaper model than that
///    instead, when there is one.
///
/// Models are ordered by [`by_expected_cost`]. `None` when there are no
/// candidates.
pub fn pick(
    stats: &GateStats,
    level: RouteLevel,
    category: PromptCategory,
    candidates: &[Candidate],
    settings: &GateConfig,
    rng: &mut impl Random,
) -> Option<Decision> {
    let order = by_expected_cost(stats, level, candidates);
    let proven = order.iter().position(|c| {
        stats.row(level, category, &c.id).is_some_and(|row| {
            row.tries >= settings.min_tries
                && row
                    .success_rate()
                    .is_some_and(|rate| rate >= settings.target)
        })
    });
    let (index, reason) = match proven {
        Some(index) => (index, RouteReason::Proven),
        None => (default_index(&order, category)?, RouteReason::Default),
    };
    let rate = settings.explore_rate.clamp(0.0, 1.0);
    let decide = |index: usize, reason, explored, propensity| {
        Some(Decision {
            model: order[index].clone(),
            reason,
            explored,
            propensity,
        })
    };
    if index == 0 {
        return decide(index, reason, false, 1.0);
    }
    if rate > 0.0 && rng.unit() < rate {
        return decide(index - 1, RouteReason::Explore, true, rate);
    }
    decide(index, reason, false, 1.0 - rate)
}

/// Where work at `level` that failed on `failed` is retried, once: the
/// cheapest model (by [`by_expected_cost`]) of the lowest tier above
/// `failed`'s. `None` when no candidate is stronger.
#[must_use]
pub fn escalate(
    stats: &GateStats,
    level: RouteLevel,
    failed: &str,
    candidates: &[Candidate],
) -> Option<Decision> {
    let tier = candidates
        .iter()
        .find(|c| c.id == failed)
        .map_or_else(|| infer_tier(failed), |c| c.tier);
    let stronger: Vec<&Candidate> = by_expected_cost(stats, level, candidates)
        .into_iter()
        .filter(|c| c.tier > tier)
        .collect();
    let next = stronger.iter().map(|c| c.tier).min()?;
    let model = stronger.into_iter().find(|c| c.tier == next)?;
    Some(Decision {
        model: model.clone(),
        reason: RouteReason::Escalate,
        explored: false,
        propensity: 1.0,
    })
}

/// Position in `order` of the default model for `category`.
fn default_index(order: &[&Candidate], category: PromptCategory) -> Option<usize> {
    let strongest = || {
        let top = order.iter().map(|c| c.tier).max()?;
        order.iter().position(|c| c.tier == top)
    };
    if is_heavy(category) {
        return strongest();
    }
    order
        .iter()
        .position(|c| c.tier == ModelTier::Standard)
        .or_else(strongest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_model::model_catalog::{catalog, catalog_with_plans};

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

    mod picking {
        use super::*;
        use aura_core_types::{
            AgentId, Hash, OutcomePayload, OutcomeSignal, PromptPayload, RecordEntry, Transaction,
        };

        const HAIKU: &str = "claude/claude-haiku-4-5";
        const SONNET: &str = "claude/claude-sonnet-4-6";
        const OPUS: &str = "claude/claude-opus-4-7";

        fn all() -> Vec<Candidate> {
            candidates(
                &with_local(),
                signed_in(&[
                    ProviderKind::Zos,
                    ProviderKind::Claude,
                    ProviderKind::Chatgpt,
                ]),
                |_| false,
            )
        }

        fn claude() -> Vec<Candidate> {
            candidates(&with_local(), signed_in(&[ProviderKind::Claude]), |_| false)
        }

        fn no_exploring() -> GateConfig {
            GateConfig {
                explore_rate: 0.0,
                ..GateConfig::defaults()
            }
        }

        /// A record of turns and how they went, for building stats.
        #[derive(Default)]
        struct Log {
            entries: Vec<RecordEntry>,
        }

        impl Log {
            fn push(&mut self, tx: Transaction) -> Hash {
                let hash = tx.hash;
                let seq = u64::try_from(self.entries.len()).unwrap() + 1;
                self.entries.push(RecordEntry::builder(seq, tx).build());
                hash
            }

            /// A turn on `model` asking for `category` that ended with
            /// `signal`, costing `cost_usd`.
            fn turn(
                &mut self,
                category: PromptCategory,
                model: &str,
                signal: OutcomeSignal,
                cost_usd: f64,
            ) {
                let agent = AgentId::generate();
                let mut prompt = PromptPayload::new(format!("turn {}", self.entries.len()));
                prompt.category = category;
                prompt.route = Route::turn(model, RouteReason::Default);
                let prompt_tx = self.push(Transaction::user_prompt_with(agent, &prompt));
                let outcome = OutcomePayload {
                    prompt_tx,
                    category,
                    model: model.to_string(),
                    success: signal.success(),
                    signal,
                    cost_usd,
                    latency_ms: 10,
                };
                self.push(Transaction::outcome(agent, &outcome));
            }

            fn turns(&mut self, n: usize, category: PromptCategory, model: &str, ok: bool) {
                let signal = if ok {
                    OutcomeSignal::ChecksPassed
                } else {
                    OutcomeSignal::ChecksFailed
                };
                for _ in 0..n {
                    self.turn(category, model, signal, 0.01);
                }
            }

            fn stats(&self) -> GateStats {
                GateStats::from_entries(&self.entries)
            }
        }

        fn chosen(
            stats: &GateStats,
            category: PromptCategory,
            candidates: &[Candidate],
        ) -> (String, RouteReason) {
            let decision = pick(
                stats,
                RouteLevel::Turn,
                category,
                candidates,
                &no_exploring(),
                &mut || 0.0,
            )
            .unwrap();
            (decision.model.id, decision.reason)
        }

        #[test]
        fn a_failed_attempt_escalates_to_the_cheapest_model_one_tier_up() {
            let empty = GateStats::default();
            let up = |failed: &str, candidates: &[Candidate]| {
                escalate(&empty, RouteLevel::Subtask, failed, candidates).map(|d| {
                    assert_eq!(d.reason, RouteReason::Escalate);
                    assert!(!d.explored);
                    d.model.id
                })
            };
            assert_eq!(up(HAIKU, &claude()).as_deref(), Some(SONNET));
            assert_eq!(up(SONNET, &claude()).as_deref(), Some(OPUS));
            assert_eq!(
                up(SONNET, &all()).as_deref(),
                Some("chatgpt/gpt-5.4"),
                "the cheaper of the frontier models"
            );
            assert_eq!(up(LOCAL, &claude()).as_deref(), Some(HAIKU));
            assert_eq!(up(OPUS, &claude()), None, "nothing is stronger");
            assert_eq!(up(HAIKU, &[]), None);
        }

        #[test]
        fn heavy_categories_default_to_the_strongest_model() {
            let empty = GateStats::default();
            for category in [
                PromptCategory::MultiFileChange,
                PromptCategory::Debugging,
                PromptCategory::Planning,
            ] {
                assert_eq!(
                    chosen(&empty, category, &claude()),
                    (OPUS.to_string(), RouteReason::Default),
                    "{category}"
                );
                assert_eq!(
                    chosen(&empty, category, &all()).0,
                    "chatgpt/gpt-5.4",
                    "the cheaper of the frontier models"
                );
            }
        }

        #[test]
        fn other_categories_default_to_the_cheapest_standard_model() {
            let empty = GateStats::default();
            for category in [
                PromptCategory::Question,
                PromptCategory::SmallChange,
                PromptCategory::Research,
                PromptCategory::Writing,
                PromptCategory::Other,
                PromptCategory::Unclassified,
            ] {
                assert_eq!(
                    chosen(&empty, category, &all()),
                    (SONNET.to_string(), RouteReason::Default),
                    "{category}"
                );
            }
        }

        #[test]
        fn without_a_standard_model_every_category_gets_the_strongest() {
            let chatgpt = candidates(&with_local(), signed_in(&[ProviderKind::Chatgpt]), |_| {
                false
            });
            let empty = GateStats::default();
            for category in [PromptCategory::Question, PromptCategory::Debugging] {
                assert_eq!(chosen(&empty, category, &chatgpt).0, "chatgpt/gpt-5.4");
            }
            let local = candidates(&with_local(), signed_in(&[]), |_| false);
            assert_eq!(chosen(&empty, PromptCategory::Planning, &local).0, LOCAL);
            assert!(pick(
                &empty,
                RouteLevel::Turn,
                PromptCategory::Question,
                &[],
                &no_exploring(),
                &mut || 0.0
            )
            .is_none());
        }

        #[test]
        fn a_model_becomes_the_pick_after_min_tries_successes() {
            let mut log = Log::default();
            log.turns(9, PromptCategory::Question, HAIKU, true);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Question, &claude()),
                (SONNET.to_string(), RouteReason::Default),
                "nine tries are not enough"
            );
            log.turns(1, PromptCategory::Question, HAIKU, true);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Question, &claude()),
                (HAIKU.to_string(), RouteReason::Proven)
            );
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Writing, &claude()).0,
                SONNET,
                "proven on questions says nothing about writing"
            );
        }

        #[test]
        fn a_proven_model_drops_back_after_failures() {
            let mut log = Log::default();
            log.turns(10, PromptCategory::Question, HAIKU, true);
            log.turns(1, PromptCategory::Question, HAIKU, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Question, &claude()),
                (HAIKU.to_string(), RouteReason::Proven),
                "10 of 11 is still above 85%"
            );
            log.turns(1, PromptCategory::Question, HAIKU, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Question, &claude()),
                (SONNET.to_string(), RouteReason::Default),
                "10 of 12 is below 85%"
            );
        }

        #[test]
        fn the_cheapest_proven_model_wins() {
            let mut log = Log::default();
            log.turns(10, PromptCategory::Debugging, SONNET, true);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Debugging, &claude()),
                (SONNET.to_string(), RouteReason::Proven)
            );
            log.turns(10, PromptCategory::Debugging, HAIKU, true);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Debugging, &claude()).0,
                HAIKU
            );
        }

        #[test]
        fn observed_costs_reorder_the_candidates_and_scale_the_rest() {
            let claude = claude();
            let empty = GateStats::default();
            let ids = |order: Vec<&Candidate>| -> Vec<String> {
                order.into_iter().map(|c| c.id.clone()).collect()
            };
            assert_eq!(
                ids(by_expected_cost(&empty, RouteLevel::Turn, &claude)),
                [LOCAL, HAIKU, SONNET, OPUS]
            );

            let list = |id: &str| {
                claude
                    .iter()
                    .find(|c| c.id == id)
                    .unwrap()
                    .cost_usd
                    .unwrap()
            };
            let mut log = Log::default();
            let signal = OutcomeSignal::ChecksPassed;
            log.turn(PromptCategory::Question, HAIKU, signal, 1_000.0);
            log.turn(PromptCategory::Question, SONNET, signal, list(SONNET));
            let order = ids(by_expected_cost(&log.stats(), RouteLevel::Turn, &claude));
            let at = |id: &str| order.iter().position(|o| o == id).unwrap();
            assert_eq!(order[0], LOCAL);
            assert!(at(SONNET) < at(HAIKU), "{order:?}");

            let mut log = Log::default();
            log.turn(
                PromptCategory::Question,
                SONNET,
                signal,
                list(SONNET) * 10.0,
            );
            assert_eq!(
                ids(by_expected_cost(&log.stats(), RouteLevel::Turn, &claude)),
                [LOCAL, HAIKU, SONNET, OPUS],
                "turns costing 10x list price make the untried models 10x too"
            );
            assert!(by_expected_cost(&log.stats(), RouteLevel::Subtask, &claude)
                .iter()
                .map(|c| c.id.as_str())
                .eq([LOCAL, HAIKU, SONNET, OPUS]));
        }

        #[test]
        fn exploring_takes_the_next_cheaper_model() {
            let empty = GateStats::default();
            let settings = GateConfig::defaults();
            let explored = pick(
                &empty,
                RouteLevel::Turn,
                PromptCategory::Question,
                &claude(),
                &settings,
                &mut || 0.0,
            )
            .unwrap();
            assert_eq!(explored.model.id, HAIKU);
            assert_eq!(explored.reason, RouteReason::Explore);
            assert!(explored.explored);
            assert_eq!(explored.propensity, settings.explore_rate);

            let kept = pick(
                &empty,
                RouteLevel::Turn,
                PromptCategory::Question,
                &claude(),
                &settings,
                &mut || 0.99,
            )
            .unwrap();
            assert_eq!(kept.model.id, SONNET);
            assert!(!kept.explored);
            assert_eq!(kept.propensity, 1.0 - settings.explore_rate);
            assert_eq!(
                kept.route(RouteLevel::Subtask),
                Route {
                    level: RouteLevel::Subtask,
                    model: SONNET.to_string(),
                    reason: RouteReason::Default,
                    explored: false,
                    propensity: 1.0 - settings.explore_rate,
                }
            );
        }

        #[test]
        fn exploration_happens_at_about_the_configured_rate() {
            let empty = GateStats::default();
            let settings = GateConfig {
                explore_rate: 0.2,
                ..GateConfig::defaults()
            };
            let mut rng = SplitMix64::seeded(7);
            let draws = 10_000;
            let explored = (0..draws)
                .filter(|_| {
                    pick(
                        &empty,
                        RouteLevel::Turn,
                        PromptCategory::Question,
                        &claude(),
                        &settings,
                        &mut rng,
                    )
                    .unwrap()
                    .explored
                })
                .count();
            let rate = f64::from(u32::try_from(explored).unwrap()) / f64::from(draws);
            assert!((rate - 0.2).abs() < 0.02, "explored {rate}");
        }

        #[test]
        fn nothing_cheaper_means_no_exploring() {
            let mut log = Log::default();
            log.turns(10, PromptCategory::Question, LOCAL, true);
            let settings = GateConfig {
                explore_rate: 1.0,
                ..GateConfig::defaults()
            };
            let decision = pick(
                &log.stats(),
                RouteLevel::Turn,
                PromptCategory::Question,
                &claude(),
                &settings,
                &mut || 0.0,
            )
            .unwrap();
            assert_eq!(decision.model.id, LOCAL);
            assert_eq!(decision.reason, RouteReason::Proven);
            assert!(!decision.explored);
            assert_eq!(decision.propensity, 1.0);
        }

        #[test]
        fn seeded_draws_repeat_and_stay_in_range() {
            let mut a = SplitMix64::seeded(42);
            let mut b = SplitMix64::seeded(42);
            for _ in 0..1_000 {
                let x = a.unit();
                assert_eq!(x, b.unit());
                assert!((0.0..1.0).contains(&x));
            }
            let mut clock = SplitMix64::from_clock();
            assert!((0.0..1.0).contains(&clock.unit()));
        }
    }
}
