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
//! has proven itself there, or the model the category starts on when
//! that is cheaper, and now and then the next cheaper model instead, to
//! learn how it does. Work starts cheap ([`start_tier`]) and a category
//! moves up a tier once its start model has failed too often to reach
//! the target. When an attempt fails, [`escalate`] picks the model the
//! work goes on with.

use std::collections::HashMap;

use aura_config::GateConfig;
use aura_core_auth::ProviderKind;
use aura_core_types::{Difficulty, PromptCategory, Route, RouteLevel, RouteReason};
use aura_model::model_catalog::{
    estimate_cost_usd, infer_tier, model_key, route_label, ModelBackend, ModelOption, ModelTier,
};
use aura_model::Usage;

use crate::stats::{GateStats, Row};
use crate::{compare, turn_judge};

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

/// The tier work asking for `category`, of `difficulty` when the
/// classifier said, starts on before Gate has learned better.
///
/// Small changes and work the classifier found easy start on the fast
/// tier, and everything else on the standard one: plans, whatever their
/// difficulty, and small changes it found hard. A failing test or build
/// moves code work up a tier, and a category whose start model fails too
/// often starts higher ([`pick`]). The frontier tier is reached that way,
/// not by default: on the work it is asked for, it costs several times a
/// standard model for results the checks rarely tell apart.
#[must_use]
pub const fn start_tier(category: PromptCategory, difficulty: Option<Difficulty>) -> ModelTier {
    match (category, difficulty) {
        (PromptCategory::Planning, _) | (PromptCategory::SmallChange, Some(Difficulty::Hard)) => {
            ModelTier::Standard
        }
        (PromptCategory::SmallChange, _) | (_, Some(Difficulty::Easy)) => ModelTier::Fast,
        _ => ModelTier::Standard,
    }
}

/// A piece of work Gate routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Work<'a> {
    /// Turn or subtask.
    pub level: RouteLevel,
    /// What it asks for.
    pub category: PromptCategory,
    /// How hard the classifier judged it; `None` when it did not say.
    pub difficulty: Option<Difficulty>,
    /// The permanent agent a subtask goes to.
    pub kind: Option<&'a str>,
}

impl<'a> Work<'a> {
    /// Work at `level` asking for `category`, for the permanent agent
    /// `kind` when it is a subtask, of unknown difficulty.
    #[must_use]
    pub const fn new(level: RouteLevel, category: PromptCategory, kind: Option<&'a str>) -> Self {
        Self {
            level,
            category,
            difficulty: None,
            kind,
        }
    }

    /// A turn asking for `category`, of unknown difficulty.
    #[must_use]
    pub const fn turn(category: PromptCategory) -> Self {
        Self::new(RouteLevel::Turn, category, None)
    }

    /// The same work, of `difficulty`.
    #[must_use]
    pub const fn of(self, difficulty: Option<Difficulty>) -> Self {
        Self { difficulty, ..self }
    }
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

/// How far apart two tiers are, in tiers.
const fn tiers_apart(a: ModelTier, b: ModelTier) -> u8 {
    const fn rank(tier: ModelTier) -> u8 {
        match tier {
            ModelTier::Local => 0,
            ModelTier::Fast => 1,
            ModelTier::Standard => 2,
            ModelTier::Frontier => 3,
        }
    }
    rank(a).abs_diff(rank(b))
}

/// `candidates` ordered by what work at `level` is expected to cost on
/// each: local models first (they cost nothing), then by the mean cost
/// observed at `level`, or for a model not tried there yet, its list
/// price scaled by how observed costs compare with list prices for the
/// tried models of its tier, or of the nearest tiers when none of its own
/// was tried. Heavier work goes to stronger models, so what one tier's
/// work costs says less about another's the further apart they are.
/// Models with no price known come last, with no cost; ties keep the
/// order of `candidates`.
#[must_use]
pub fn expected_costs<'a>(
    stats: &GateStats,
    level: RouteLevel,
    candidates: &'a [Candidate],
) -> Vec<(&'a Candidate, Option<f64>)> {
    let observed = |c: &Candidate| stats.mean_cost_usd(level, &c.id);
    // Tier, observed cost and list price of every tried model.
    let tried: Vec<(ModelTier, f64, f64)> = candidates
        .iter()
        .filter(|c| c.backend != ModelBackend::Local)
        .filter_map(|c| Some((c.tier, observed(c)?, c.cost_usd.filter(|&p| p > 0.0)?)))
        .collect();
    let scale = |tier: ModelTier| {
        let nearest = tried.iter().map(|(t, ..)| tiers_apart(*t, tier)).min();
        let (seen, listed) = tried
            .iter()
            .filter(|(t, ..)| Some(tiers_apart(*t, tier)) == nearest)
            .fold((0.0, 0.0), |(seen, listed), (_, o, l)| {
                (seen + o, listed + l)
            });
        if seen > 0.0 && listed > 0.0 {
            seen / listed
        } else {
            1.0
        }
    };
    let expected = |c: &Candidate| {
        if c.backend == ModelBackend::Local {
            Some(0.0)
        } else {
            observed(c).or_else(|| c.cost_usd.map(|p| p * scale(c.tier)))
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
    order
}

/// `candidates` ordered by what work at `level` is expected to cost on
/// each; see [`expected_costs`].
#[must_use]
pub fn by_expected_cost<'a>(
    stats: &GateStats,
    level: RouteLevel,
    candidates: &'a [Candidate],
) -> Vec<&'a Candidate> {
    expected_costs(stats, level, candidates)
        .into_iter()
        .map(|(c, _)| c)
        .collect()
}

/// Where `work` goes, among `candidates` (as [`candidates`] returns
/// them), given what `stats` has learned:
///
/// 1. the cheapest model with at least `min_tries` counted outcomes on
///    the category and a success rate of at least `target`;
/// 2. or, when it is cheaper than that (or nothing is proven), the model
///    the work starts on: the cheapest model of its [`start_tier`],
///    or of the next tier up while the category's outcomes on that model
///    rule it out ([`crate::stats::Row::can_still_reach`]), ending at the strongest;
/// 3. with probability `explore_rate`, a cheaper model than that
///    instead, when there is one: the nearest the category's outcomes
///    have not ruled out, else the nearest. So a model that was ruled
///    out is tried again once nothing cheaper is more promising, and
///    comes back when those tries prove it.
///
/// Models are judged by their [`judged`] rows and ordered by
/// [`by_expected_cost`]. `None` when there are no candidates.
///
/// Work a check verifies ([`verifiable`]) goes by [`cascade`] instead
/// while `settings.cascade` is on.
pub fn pick(
    stats: &GateStats,
    work: &Work<'_>,
    candidates: &[Candidate],
    settings: &GateConfig,
    rng: &mut impl Random,
) -> Option<Decision> {
    if settings.cascade && verifiable(work.category) {
        if let Some(decision) = cascade(stats, work, candidates, settings, rng) {
            return Some(decision);
        }
    }
    let Work {
        level,
        category,
        kind,
        ..
    } = *work;
    let row = |model: &str| judged(stats, level, category, kind, model, settings.min_tries);
    let order = by_expected_cost(stats, level, candidates);
    let proven = order.iter().position(|c| {
        row(&c.id).is_some_and(|row| {
            row.tries >= settings.min_tries
                && row
                    .success_rate()
                    .is_some_and(|rate| rate >= settings.target)
        })
    });
    let start = start_index(
        &row,
        start_tier(category, work.difficulty),
        &order,
        settings,
    )?;
    let (index, reason) = match proven {
        Some(index) if index <= start => (index, RouteReason::Proven),
        _ => (start, RouteReason::Default),
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
        let open = (0..index).rev().find(|&cheaper| {
            row(&order[cheaper].id)
                .is_none_or(|row| row.can_still_reach(settings.target, settings.min_tries))
        });
        return decide(open.unwrap_or(index - 1), RouteReason::Explore, true, rate);
    }
    decide(index, reason, false, 1.0 - rate)
}

/// Where work at `level` that failed on `failed` goes next: the cheapest
/// model (by [`by_expected_cost`]) of the lowest tier above `failed`'s.
/// `None` when no candidate is stronger.
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

/// What a subtask is first routed among when the agent dispatching it
/// works on `lead`: the `candidates` below the frontier tier when `lead`
/// is a frontier model and a standard one is among them, so the most
/// expensive model plans the work and a cheaper one does it. Every
/// candidate otherwise: a lead on a cheaper model already costs little,
/// and with nothing between frontier and fast the work would drop two
/// tiers. A failed attempt still escalates among all `candidates`.
#[must_use]
pub fn below_lead(candidates: &[Candidate], lead: Option<&str>) -> Vec<Candidate> {
    let frontier = lead.is_some_and(|lead| {
        candidates
            .iter()
            .find(|c| c.id == lead)
            .map_or_else(|| infer_tier(lead), |c| c.tier)
            == ModelTier::Frontier
    });
    let standard = candidates.iter().any(|c| c.tier == ModelTier::Standard);
    if frontier && standard {
        candidates
            .iter()
            .filter(|c| c.tier < ModelTier::Frontier)
            .cloned()
            .collect()
    } else {
        candidates.to_vec()
    }
}

/// The outcomes [`pick`] judges `model` by for work at `level` asking for
/// `category`: for a subtask, those of subtasks for the permanent agent
/// `kind` once they number `min_tries`, as a model can suit one agent's
/// work and not another's, else every subtask on the category; for a
/// turn, the turns it did itself ([`GateStats::unled_row`]), since a led
/// turn that failed may have failed in its subtasks.
#[must_use]
pub fn judged(
    stats: &GateStats,
    level: RouteLevel,
    category: PromptCategory,
    kind: Option<&str>,
    model: &str,
    min_tries: u32,
) -> Option<Row> {
    match level {
        RouteLevel::Turn => stats.unled_row(category, model),
        RouteLevel::Subtask => kind
            .and_then(|kind| stats.kind_row(category, kind, model))
            .filter(|row| row.settled(min_tries))
            .or_else(|| stats.row(level, category, model))
            .copied(),
    }
}

/// Position in `order` of the model work starts on: the cheapest model of
/// its `start` tier ([`start_tier`]), or of the lowest tier above it
/// whose cheapest model the category's outcomes (`row`) have not ruled
/// out. The cheapest model of the strongest tier when every tier is ruled
/// out or none is at or above the start tier.
fn start_index(
    row: &impl Fn(&str) -> Option<Row>,
    start: ModelTier,
    order: &[&Candidate],
    settings: &GateConfig,
) -> Option<usize> {
    let top = order.iter().map(|c| c.tier).max()?;
    let strongest = order.iter().position(|c| c.tier == top);
    let mut tiers: Vec<ModelTier> = order
        .iter()
        .map(|c| c.tier)
        .filter(|tier| *tier >= start)
        .collect();
    tiers.sort_unstable();
    tiers.dedup();
    tiers
        .into_iter()
        .filter_map(|tier| order.iter().position(|c| c.tier == tier))
        .find(|&index| {
            row(&order[index].id)
                .is_none_or(|row| row.can_still_reach(settings.target, settings.min_tries))
        })
        .or(strongest)
}

/// Whether a check verifies work asking for `category`: code changes and
/// debugging end in a test or build that passes or fails, and a failed
/// attempt goes on with a stronger model ([`escalate`]). So such work can
/// start on a cheaper model at little risk to the result.
#[must_use]
pub const fn verifiable(category: PromptCategory) -> bool {
    matches!(
        category,
        PromptCategory::SmallChange | PromptCategory::MultiFileChange | PromptCategory::Debugging
    )
}

/// How often Gate expects a model of `tier` to succeed before it has
/// seen the model work.
const fn prior_success(tier: ModelTier) -> f64 {
    match tier {
        ModelTier::Local => 0.5,
        ModelTier::Fast => 0.7,
        ModelTier::Standard => 0.85,
        ModelTier::Frontier => 0.92,
    }
}

/// How many outcomes the prior of a [`Belief`] weighs as.
const PRIOR_WEIGHT: f64 = 4.0;

/// Success [`cascade`] expects of a model before starting work on it,
/// unless it is of the strongest tier: below that, the work most likely
/// takes two attempts, which costs the user time however cheap the
/// first one is.
const MIN_START_SUCCESS: f64 = 0.6;

/// Tokens of a typical review of a failed attempt by the judge, for
/// pricing it.
const JUDGE_CALL: Usage = Usage {
    input_tokens: 3_000,
    output_tokens: 200,
    cache_creation_input_tokens: None,
    cache_read_input_tokens: None,
};

/// Outcomes a model needs on a category before their mean cost stands
/// for what the category costs on it.
const COST_TRIES: u32 = 3;

/// Draws that estimate how often exploring picks a model.
const PROPENSITY_DRAWS: u32 = 32;

/// What Gate believes of how often a model succeeds at some work: a Beta
/// distribution over its success rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Belief {
    /// Successes, the prior's included.
    pub alpha: f64,
    /// Failures, the prior's included.
    pub beta: f64,
}

impl Belief {
    /// A prior of mean `mean` weighing [`PRIOR_WEIGHT`] outcomes, updated
    /// with the outcomes of `row`.
    fn new(mean: f64, row: Option<Row>) -> Self {
        let row = row.unwrap_or_default();
        Self {
            alpha: PRIOR_WEIGHT.mul_add(mean, f64::from(row.successes)),
            beta: PRIOR_WEIGHT.mul_add(
                1.0 - mean,
                f64::from(row.tries.saturating_sub(row.successes)),
            ),
        }
    }

    /// The expected success rate.
    #[must_use]
    pub fn mean(self) -> f64 {
        self.alpha / (self.alpha + self.beta)
    }

    /// A success rate drawn from the belief.
    fn sample(self, rng: &mut dyn Random) -> f64 {
        let a = gamma(self.alpha, rng);
        let b = gamma(self.beta, rng);
        if a + b > 0.0 {
            a / (a + b)
        } else {
            self.mean()
        }
    }
}

/// How much more often than usual work of `difficulty` fails, before Gate
/// has seen how a model does on it.
const fn failure_scale(difficulty: Difficulty) -> f64 {
    match difficulty {
        Difficulty::Easy => 0.5,
        Difficulty::Medium => 1.0,
        Difficulty::Hard => 1.6,
    }
}

/// What Gate believes of how often `candidate` succeeds at `work`: the
/// outcomes of its [`judged`] row, on a prior whose mean is its tier's
/// expected success moved toward how it did on the other categories at
/// the work's level. When the work's difficulty is known, the chance of
/// failing is then scaled by how much more or less often the model fails
/// at that difficulty than overall ([`failure_scale`] until it has
/// outcomes there), keeping the belief's weight.
#[must_use]
pub fn belief(stats: &GateStats, work: &Work<'_>, candidate: &Candidate, min_tries: u32) -> Belief {
    let Work {
        level,
        category,
        kind,
        difficulty,
    } = *work;
    let own = judged(stats, level, category, kind, &candidate.id, min_tries);
    let overall = stats.level_row(level, &candidate.id);
    let elsewhere = overall.without(
        &stats
            .row(level, category, &candidate.id)
            .copied()
            .unwrap_or_default(),
    );
    let tier_success = prior_success(candidate.tier);
    let mean = PRIOR_WEIGHT.mul_add(tier_success, f64::from(elsewhere.successes))
        / (PRIOR_WEIGHT + f64::from(elsewhere.tries));
    let belief = Belief::new(mean, own);
    let Some(difficulty) = difficulty else {
        return belief;
    };
    let failing = |row: Row, prior: f64| {
        PRIOR_WEIGHT.mul_add(prior, f64::from(row.tries.saturating_sub(row.successes)))
            / (PRIOR_WEIGHT + f64::from(row.tries))
    };
    let usual = failing(overall, 1.0 - tier_success);
    let there = stats
        .difficulty_row(level, &candidate.id, difficulty)
        .copied()
        .unwrap_or_default();
    let scale = failing(there, (usual * failure_scale(difficulty)).min(1.0)) / usual;
    let weight = belief.alpha + belief.beta;
    let mean = 1.0 - ((1.0 - belief.mean()) * scale).clamp(0.001, 0.999);
    Belief {
        alpha: mean * weight,
        beta: (1.0 - mean) * weight,
    }
}

/// Where verifiable work goes: the model it is expected to cost least
/// from, counting what a failed attempt costs to go on with. An attempt
/// on model `i` costs `c(i)` and fails with probability `1 - p(i)`; the
/// work then pays the judge's review `j` and goes on with the cheapest
/// model of the next tier up, so it is expected to cost
/// `c(i) + (1 - p(i)) * (j + E(next))`. A turn goes on until the
/// strongest tier; a subtask goes on once. `p` is each model's [`belief`]
/// mean, and a model must be expected to succeed at least
/// [`MIN_START_SUCCESS`] of the time to start the work unless it is of
/// the strongest tier. `c` is a model's mean cost on the category, its
/// subtasks' included, once it has [`COST_TRIES`] outcomes there, else
/// its [`expected_costs`].
///
/// With probability `explore_rate`, the success rates are drawn from the
/// beliefs instead of taken at their means (Thompson sampling): a model
/// little is known of is tried about as often as it may be the cheapest.
/// `None` when no candidate's price is known.
fn cascade(
    stats: &GateStats,
    work: &Work<'_>,
    candidates: &[Candidate],
    settings: &GateConfig,
    rng: &mut impl Random,
) -> Option<Decision> {
    let Work {
        level,
        category,
        kind,
        ..
    } = *work;
    let priced: Vec<(&Candidate, f64)> = expected_costs(stats, level, candidates)
        .into_iter()
        .filter_map(|(c, cost)| Some((c, cost?)))
        .collect();
    let top = priced.iter().map(|(c, _)| c.tier).max()?;
    let rows: Vec<Option<Row>> = priced
        .iter()
        .map(|(c, _)| judged(stats, level, category, kind, &c.id, settings.min_tries))
        .collect();
    let costs: Vec<f64> = priced
        .iter()
        .zip(&rows)
        .map(|((_, cost), row)| {
            row.filter(|row| row.settled(COST_TRIES))
                .and_then(|row| row.mean_total_cost_usd())
                .unwrap_or(*cost)
        })
        .collect();
    let next: Vec<Option<usize>> = priced
        .iter()
        .map(|(c, _)| {
            let tier = priced
                .iter()
                .map(|(o, _)| o.tier)
                .filter(|t| *t > c.tier)
                .min()?;
            (0..priced.len())
                .filter(|&o| priced[o].0.tier == tier)
                .min_by(|&a, &b| costs[a].total_cmp(&costs[b]))
        })
        .collect();
    let judge = compare::default_against(candidates)
        .filter(|_| settings.judge)
        .and_then(|judge| {
            Some((
                judge.id.as_str(),
                estimate_cost_usd(&judge.id, &JUDGE_CALL)?,
            ))
        });
    let review = |i: usize| {
        judge
            .filter(|(id, _)| turn_judge::judges(id, &priced[i].0.id, candidates))
            .map_or(0.0, |(_, cost)| cost)
    };
    let retries = match level {
        RouteLevel::Turn => priced.len(),
        RouteLevel::Subtask => 1,
    };
    let expected = |start: usize, success: &[f64]| {
        let mut chain = vec![start];
        while chain.len() <= retries {
            match next[chain[chain.len() - 1]] {
                Some(up) => chain.push(up),
                None => break,
            }
        }
        chain.iter().rev().fold(None, |after: Option<f64>, &i| {
            Some(after.map_or(costs[i], |after| {
                (1.0 - success[i]).mul_add(review(i) + after, costs[i])
            }))
        })
    };
    let best = |success: &[f64]| {
        (0..priced.len())
            .filter(|&i| success[i] >= MIN_START_SUCCESS || priced[i].0.tier == top)
            .filter_map(|i| Some((i, expected(i, success)?)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let beliefs: Vec<Belief> = priced
        .iter()
        .map(|(c, _)| belief(stats, work, c, settings.min_tries))
        .collect();
    let means: Vec<f64> = beliefs.iter().map(|b| b.mean()).collect();
    let kept = best(&means)?;
    let sampled = |rng: &mut dyn Random| {
        let draws: Vec<f64> = beliefs.iter().map(|b| b.sample(&mut *rng)).collect();
        best(&draws)
    };
    let rng: &mut dyn Random = rng;
    let rate = settings.explore_rate.clamp(0.0, 1.0);
    let explores = rate > 0.0 && rng.unit() < rate;
    let chosen = if explores {
        sampled(&mut *rng).unwrap_or(kept)
    } else {
        kept
    };
    let propensity = if rate > 0.0 {
        let hits = (0..PROPENSITY_DRAWS)
            .filter(|_| sampled(&mut *rng) == Some(chosen))
            .count();
        let hits = u32::try_from(hits).unwrap_or(PROPENSITY_DRAWS) + u32::from(explores);
        let draws = PROPENSITY_DRAWS + u32::from(explores);
        let kept_share = if chosen == kept { 1.0 - rate } else { 0.0 };
        rate.mul_add(f64::from(hits) / f64::from(draws), kept_share)
    } else {
        1.0
    };
    let explored = chosen != kept;
    let reason = if explored {
        RouteReason::Explore
    } else if rows[chosen].is_some_and(|row| row.settled(settings.min_tries)) {
        RouteReason::Proven
    } else {
        RouteReason::Default
    };
    Some(Decision {
        model: priced[chosen].0.clone(),
        reason,
        explored,
        propensity,
    })
}

/// A draw in `(0, 1)`, never exactly 0 or 1, for taking logarithms.
fn open_unit(rng: &mut dyn Random) -> f64 {
    rng.unit().clamp(f64::MIN_POSITIVE, 1.0 - f64::EPSILON)
}

/// A draw from the standard normal distribution (Box–Muller).
fn normal(rng: &mut dyn Random) -> f64 {
    let radius = (-2.0 * open_unit(rng).ln()).sqrt();
    radius * (std::f64::consts::TAU * rng.unit()).cos()
}

/// A draw from the Gamma distribution of `shape` and scale 1
/// (Marsaglia–Tsang). Gives the mode when the draws never settle, which
/// only a generator that keeps returning the same value makes happen.
fn gamma(shape: f64, rng: &mut dyn Random) -> f64 {
    if shape <= 0.0 {
        return 0.0;
    }
    if shape < 1.0 {
        return gamma(shape + 1.0, rng) * open_unit(rng).powf(1.0 / shape);
    }
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    for _ in 0..64 {
        let x = normal(rng);
        let v = c.mul_add(x, 1.0).powi(3);
        if v <= 0.0 {
            continue;
        }
        if open_unit(rng).ln() < (0.5 * x).mul_add(x, d - d * v + d * v.ln()) {
            return d * v;
        }
    }
    d
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

        /// Routing by success rate only, without exploring.
        fn no_exploring() -> GateConfig {
            GateConfig {
                explore_rate: 0.0,
                cascade: false,
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
                    subtask_cost_usd: 0.0,
                    latency_ms: 10,
                };
                self.push(Transaction::outcome(agent, &outcome));
            }

            /// A turn the classifier judged of `difficulty`.
            fn rated_turn(
                &mut self,
                category: PromptCategory,
                model: &str,
                difficulty: Difficulty,
                ok: bool,
            ) {
                let agent = AgentId::generate();
                let mut prompt = PromptPayload::new(format!("turn {}", self.entries.len()));
                prompt.category = category;
                prompt.route = Route::turn(model, RouteReason::Default);
                prompt.difficulty = Some(difficulty);
                let prompt_tx = self.push(Transaction::user_prompt_with(agent, &prompt));
                let signal = if ok {
                    OutcomeSignal::ChecksPassed
                } else {
                    OutcomeSignal::ChecksFailed
                };
                let outcome = OutcomePayload {
                    prompt_tx,
                    category,
                    model: model.to_string(),
                    success: signal.success(),
                    signal,
                    cost_usd: 0.01,
                    subtask_cost_usd: 0.0,
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

            /// `n` subtasks for the permanent agent `kind` on `model`
            /// asking for `category`, that all passed or all failed.
            fn subtasks(
                &mut self,
                n: usize,
                category: PromptCategory,
                model: &str,
                kind: &str,
                ok: bool,
            ) {
                let signal = if ok {
                    OutcomeSignal::ChecksPassed
                } else {
                    OutcomeSignal::ChecksFailed
                };
                for _ in 0..n {
                    let agent = AgentId::generate();
                    let mut prompt = PromptPayload::new(format!("task {}", self.entries.len()));
                    prompt.category = category;
                    prompt.route = Route::subtask(model, RouteReason::Default);
                    prompt.kind = Some(kind.to_string());
                    let prompt_tx = self.push(Transaction::user_prompt_with(agent, &prompt));
                    let outcome = OutcomePayload {
                        prompt_tx,
                        category,
                        model: model.to_string(),
                        success: signal.success(),
                        signal,
                        cost_usd: 0.01,
                        subtask_cost_usd: 0.0,
                        latency_ms: 10,
                    };
                    self.push(Transaction::outcome(agent, &outcome));
                }
            }

            fn stats(&self) -> GateStats {
                GateStats::from_entries(&self.entries)
            }
        }

        #[test]
        fn a_subtask_kind_with_enough_outcomes_is_judged_on_its_own() {
            let mut log = Log::default();
            let category = PromptCategory::Debugging;
            log.subtasks(60, category, HAIKU, "explorer", true);
            log.subtasks(10, category, HAIKU, "rust-engineer", false);
            let stats = log.stats();
            let chosen = |kind| {
                let decision = pick(
                    &stats,
                    &Work::new(RouteLevel::Subtask, category, kind),
                    &claude(),
                    &no_exploring(),
                    &mut || 0.0,
                )
                .unwrap();
                (decision.model.id, decision.reason)
            };

            let proven = (HAIKU.to_string(), RouteReason::Proven);
            assert_eq!(chosen(None), proven, "60 of 70 overall");
            assert_eq!(chosen(Some("explorer")), proven);
            assert_eq!(chosen(Some("tests")), proven, "no outcomes of its own yet");
            let (model, reason) = chosen(Some("rust-engineer"));
            assert_ne!(model, HAIKU, "0 of 10 for this kind rules it out");
            assert_eq!(reason, RouteReason::Default);

            let turn = pick(
                &stats,
                &Work::new(RouteLevel::Turn, category, Some("rust-engineer")),
                &claude(),
                &no_exploring(),
                &mut || 0.0,
            )
            .unwrap();
            assert_ne!(
                turn.reason,
                RouteReason::Proven,
                "subtasks teach turns nothing"
            );
        }

        fn chosen(
            stats: &GateStats,
            category: PromptCategory,
            candidates: &[Candidate],
        ) -> (String, RouteReason) {
            let decision = pick(
                stats,
                &Work::new(RouteLevel::Turn, category, None),
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
        fn planning_starts_on_the_standard_tier() {
            let empty = GateStats::default();
            assert_eq!(
                chosen(&empty, PromptCategory::Planning, &claude()),
                (SONNET.to_string(), RouteReason::Default)
            );
            assert_eq!(chosen(&empty, PromptCategory::Planning, &all()).0, SONNET);
        }

        #[test]
        fn failures_of_led_turns_do_not_rule_the_lead_model_out() {
            let category = PromptCategory::MultiFileChange;
            let mut log = Log::default();
            for _ in 0..3 {
                let agent = AgentId::generate();
                let mut prompt = PromptPayload::new(format!("led {}", log.entries.len()));
                prompt.category = category;
                prompt.route = Route::turn(SONNET, RouteReason::Default);
                prompt.led = Some(true);
                let prompt_tx = log.push(Transaction::user_prompt_with(agent, &prompt));
                let signal = OutcomeSignal::ChecksFailed;
                log.push(Transaction::outcome(
                    agent,
                    &OutcomePayload {
                        prompt_tx,
                        category,
                        model: SONNET.to_string(),
                        success: signal.success(),
                        signal,
                        cost_usd: 0.1,
                        subtask_cost_usd: 0.1,
                        latency_ms: 10,
                    },
                ));
            }
            assert_eq!(chosen(&log.stats(), category, &claude()).0, SONNET);
            log.turns(2, category, SONNET, false);
            assert_eq!(
                chosen(&log.stats(), category, &claude()).0,
                OPUS,
                "two failures of its own do"
            );
        }

        #[test]
        fn small_changes_start_on_the_cheapest_fast_model() {
            let empty = GateStats::default();
            assert_eq!(
                chosen(&empty, PromptCategory::SmallChange, &all()),
                (HAIKU.to_string(), RouteReason::Default)
            );
            let no_fast: Vec<Candidate> = claude()
                .into_iter()
                .filter(|c| c.tier != ModelTier::Fast)
                .collect();
            assert_eq!(
                chosen(&empty, PromptCategory::SmallChange, &no_fast).0,
                SONNET,
                "the next tier up when there is no fast model"
            );
        }

        #[test]
        fn a_start_model_that_fails_too_often_hands_the_category_up_a_tier() {
            let mut log = Log::default();
            log.turns(1, PromptCategory::Debugging, SONNET, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Debugging, &claude()).0,
                SONNET,
                "nine of ten can still pass"
            );
            log.turns(1, PromptCategory::Debugging, SONNET, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Debugging, &claude()),
                (OPUS.to_string(), RouteReason::Default),
                "eight of ten cannot reach 85%"
            );
            assert_eq!(
                chosen(&log.stats(), PromptCategory::MultiFileChange, &claude()).0,
                SONNET,
                "failing at debugging says nothing about other work"
            );

            let mut log = Log::default();
            log.turns(2, PromptCategory::SmallChange, HAIKU, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::SmallChange, &claude()).0,
                SONNET
            );
            log.turns(2, PromptCategory::SmallChange, SONNET, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::SmallChange, &claude()).0,
                OPUS
            );
            log.turns(2, PromptCategory::SmallChange, OPUS, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::SmallChange, &claude()).0,
                OPUS,
                "the strongest model when every tier is ruled out"
            );
        }

        #[test]
        fn a_proven_stronger_model_does_not_outrank_a_cheaper_start() {
            let mut log = Log::default();
            log.turns(10, PromptCategory::Debugging, OPUS, true);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Debugging, &claude()),
                (SONNET.to_string(), RouteReason::Default)
            );
            log.turns(2, PromptCategory::Debugging, SONNET, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Debugging, &claude()),
                (OPUS.to_string(), RouteReason::Proven)
            );
        }

        #[test]
        fn other_categories_start_on_the_cheapest_standard_model() {
            let empty = GateStats::default();
            for category in [
                PromptCategory::Question,
                PromptCategory::MultiFileChange,
                PromptCategory::Debugging,
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
                &Work::new(RouteLevel::Turn, PromptCategory::Question, None),
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
        fn an_untried_model_is_priced_from_the_tried_models_of_its_tier() {
            const SONNET_5: &str = "claude/claude-sonnet-5-5";
            const OPUS_5: &str = "claude/claude-opus-5-5";
            let live: Vec<ModelOption> = [
                (HAIKU, ModelTier::Fast),
                (SONNET, ModelTier::Standard),
                (SONNET_5, ModelTier::Standard),
                (OPUS, ModelTier::Frontier),
                (OPUS_5, ModelTier::Frontier),
            ]
            .into_iter()
            .map(|(id, tier)| ModelOption {
                id: id.to_string(),
                label: id.to_string(),
                backend: ModelBackend::Claude,
                installed: true,
                tier,
            })
            .collect();
            let options = catalog_with_plans(LOCAL, &[], &live);
            let plan = candidates(&options, signed_in(&[ProviderKind::Claude]), |_| false);

            // Sonnet 4.6 took the light work and Opus 4.7 the heavy, so
            // Sonnet's turns cost far less against list price than Opus's.
            let mut log = Log::default();
            let signal = OutcomeSignal::ChecksPassed;
            log.turn(PromptCategory::SmallChange, SONNET, signal, 0.02);
            log.turn(PromptCategory::Debugging, OPUS, signal, 0.60);
            let stats = log.stats();
            let order: Vec<&str> = by_expected_cost(&stats, RouteLevel::Turn, &plan)
                .into_iter()
                .map(|c| c.id.as_str())
                .collect();
            assert_eq!(
                order,
                [HAIKU, SONNET_5, SONNET, OPUS_5, OPUS],
                "the untried fast model is priced from the standard tier's turns"
            );
            assert_eq!(
                chosen(&stats, PromptCategory::Question, &plan).0,
                SONNET_5,
                "the cheaper standard model stays the default"
            );
            assert_eq!(chosen(&stats, PromptCategory::Debugging, &plan).0, SONNET_5);
            assert_eq!(chosen(&stats, PromptCategory::Planning, &plan).0, SONNET_5);
        }

        #[test]
        fn a_frontier_lead_keeps_its_subtasks_below_its_tier() {
            let ids = |among: Vec<Candidate>| -> Vec<String> {
                among.into_iter().map(|c| c.id).collect()
            };
            let claude = claude();
            assert_eq!(
                ids(below_lead(&claude, Some(OPUS))),
                [LOCAL, HAIKU, SONNET],
                "nothing of the lead's tier"
            );
            let among = below_lead(&claude, Some(OPUS));
            let empty = GateStats::default();
            assert_eq!(
                chosen(&empty, PromptCategory::Planning, &among).0,
                SONNET,
                "work for the strongest model goes to the strongest left"
            );
            assert_eq!(
                escalate(&empty, RouteLevel::Subtask, SONNET, &claude).map(|d| d.model.id),
                Some(OPUS.to_string()),
                "a failed attempt may still go up to the lead's tier"
            );

            assert_eq!(below_lead(&claude, Some(SONNET)), claude);
            assert_eq!(below_lead(&claude, Some(HAIKU)), claude);
            assert_eq!(below_lead(&claude, None), claude, "an unknown lead");
            assert_eq!(
                below_lead(&claude, Some("claude/claude-opus-5-5")).len(),
                3,
                "a lead outside the candidates is placed by its name"
            );

            let no_standard: Vec<Candidate> = claude
                .iter()
                .filter(|c| c.tier != ModelTier::Standard)
                .cloned()
                .collect();
            assert_eq!(
                below_lead(&no_standard, Some(OPUS)),
                no_standard,
                "without a standard model the work stays where it was"
            );
        }

        #[test]
        fn exploring_skips_ruled_out_models_until_nothing_else_is_cheaper() {
            let settings = GateConfig {
                cascade: false,
                ..GateConfig::defaults()
            };
            let explored = |stats: &GateStats| {
                let decision = pick(
                    stats,
                    &Work::new(RouteLevel::Turn, PromptCategory::Debugging, None),
                    &claude(),
                    &settings,
                    &mut || 0.0,
                )
                .unwrap();
                assert_eq!(decision.reason, RouteReason::Explore);
                decision.model.id
            };
            let mut log = Log::default();
            log.turns(2, PromptCategory::Debugging, SONNET, false);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Debugging, &claude()).0,
                OPUS
            );
            assert_eq!(
                explored(&log.stats()),
                HAIKU,
                "Sonnet is ruled out, Haiku is not"
            );
            log.turns(2, PromptCategory::Debugging, HAIKU, false);
            let local = claude()
                .into_iter()
                .find(|c| c.tier == ModelTier::Local)
                .unwrap()
                .id;
            assert_eq!(explored(&log.stats()), local);
            log.turns(2, PromptCategory::Debugging, &local, false);
            assert_eq!(
                explored(&log.stats()),
                SONNET,
                "with every cheaper model ruled out, the nearest gets another try"
            );
            log.turns(12, PromptCategory::Debugging, SONNET, true);
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Debugging, &claude()),
                (SONNET.to_string(), RouteReason::Proven),
                "and comes back once those tries prove it"
            );
        }

        #[test]
        fn exploring_takes_the_next_cheaper_model() {
            let empty = GateStats::default();
            let settings = GateConfig::defaults();
            let explored = pick(
                &empty,
                &Work::new(RouteLevel::Turn, PromptCategory::Question, None),
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
                &Work::new(RouteLevel::Turn, PromptCategory::Question, None),
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
                        &Work::new(RouteLevel::Turn, PromptCategory::Question, None),
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
                &Work::new(RouteLevel::Turn, PromptCategory::Question, None),
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

        fn cascading() -> GateConfig {
            GateConfig {
                explore_rate: 0.0,
                ..GateConfig::defaults()
            }
        }

        fn cascaded(stats: &GateStats, level: RouteLevel, category: PromptCategory) -> Decision {
            pick(
                stats,
                &Work::new(level, category, None),
                &claude(),
                &cascading(),
                &mut || 0.0,
            )
            .unwrap()
        }

        #[test]
        fn verifiable_work_starts_where_it_is_expected_to_cost_least() {
            let empty = GateStats::default();
            for category in [
                PromptCategory::SmallChange,
                PromptCategory::MultiFileChange,
                PromptCategory::Debugging,
            ] {
                let decision = cascaded(&empty, RouteLevel::Turn, category);
                assert_eq!(decision.model.id, HAIKU, "{category:?}");
                assert_eq!(decision.reason, RouteReason::Default);
                assert_eq!(decision.propensity, 1.0);
            }
            assert_eq!(
                cascaded(&empty, RouteLevel::Turn, PromptCategory::Question)
                    .model
                    .id,
                SONNET,
                "no check verifies an answer"
            );
            assert_eq!(
                pick(
                    &empty,
                    &Work::new(RouteLevel::Turn, PromptCategory::Debugging, None),
                    &claude(),
                    &no_exploring(),
                    &mut || 0.0
                )
                .unwrap()
                .model
                .id,
                SONNET,
                "with the cascade off it starts on its start tier"
            );
        }

        #[test]
        fn a_model_failing_verifiable_work_no_longer_starts_it() {
            let mut log = Log::default();
            log.turns(6, PromptCategory::Debugging, HAIKU, false);
            assert_eq!(
                cascaded(&log.stats(), RouteLevel::Turn, PromptCategory::SmallChange)
                    .model
                    .id,
                SONNET,
                "failing elsewhere lowers what is expected of it"
            );
            log.turns(6, PromptCategory::SmallChange, HAIKU, true);
            let stats = log.stats();
            assert_eq!(
                cascaded(&stats, RouteLevel::Turn, PromptCategory::Debugging)
                    .model
                    .id,
                SONNET
            );
            assert_eq!(
                cascaded(&stats, RouteLevel::Turn, PromptCategory::SmallChange)
                    .model
                    .id,
                HAIKU,
                "its own outcomes outweigh the others"
            );
        }

        #[test]
        fn a_model_proven_on_verifiable_work_starts_it_as_proven() {
            let mut log = Log::default();
            log.turns(12, PromptCategory::Debugging, HAIKU, true);
            let decision = cascaded(&log.stats(), RouteLevel::Turn, PromptCategory::Debugging);
            assert_eq!(
                (decision.model.id, decision.reason),
                (HAIKU.to_string(), RouteReason::Proven)
            );
        }

        #[test]
        fn success_on_other_work_raises_what_is_expected_of_a_model() {
            let empty = GateStats::default();
            let local = claude()
                .into_iter()
                .find(|c| c.tier == ModelTier::Local)
                .unwrap();
            let unseen = belief(
                &empty,
                &Work::new(RouteLevel::Turn, PromptCategory::Debugging, None),
                &local,
                10,
            );
            assert!((unseen.mean() - 0.5).abs() < 1e-9);
            assert_ne!(
                cascaded(&empty, RouteLevel::Turn, PromptCategory::Debugging)
                    .model
                    .id,
                local.id,
                "too likely to fail to start on"
            );

            let mut log = Log::default();
            log.turns(20, PromptCategory::Question, &local.id, true);
            let stats = log.stats();
            let seen = belief(
                &stats,
                &Work::new(RouteLevel::Turn, PromptCategory::Debugging, None),
                &local,
                10,
            );
            assert!(seen.mean() > 0.9, "{seen:?}");
            assert_eq!(
                cascaded(&stats, RouteLevel::Turn, PromptCategory::Debugging)
                    .model
                    .id,
                local.id
            );
        }

        #[test]
        fn exploring_verifiable_work_draws_from_the_beliefs() {
            let empty = GateStats::default();
            let settings = GateConfig {
                explore_rate: 1.0,
                ..GateConfig::defaults()
            };
            let mut rng = SplitMix64::seeded(11);
            let mut seen: HashMap<String, usize> = HashMap::new();
            for _ in 0..500 {
                let decision = pick(
                    &empty,
                    &Work::new(RouteLevel::Turn, PromptCategory::Debugging, None),
                    &claude(),
                    &settings,
                    &mut rng,
                )
                .unwrap();
                assert!(decision.propensity > 0.0 && decision.propensity <= 1.0);
                assert_eq!(decision.explored, decision.model.id != HAIKU);
                assert_eq!(decision.reason == RouteReason::Explore, decision.explored);
                *seen.entry(decision.model.id).or_default() += 1;
            }
            assert!(seen.len() >= 2, "{seen:?}");
            assert_eq!(
                seen.iter()
                    .max_by_key(|(_, n)| **n)
                    .map(|(id, _)| id.as_str()),
                Some(HAIKU),
                "the expected cheapest stays the most likely: {seen:?}"
            );

            for unit in [0.0, 0.5, 1.0 - f64::EPSILON] {
                let decision = pick(
                    &empty,
                    &Work::new(RouteLevel::Turn, PromptCategory::Debugging, None),
                    &claude(),
                    &settings,
                    &mut || unit,
                );
                assert!(decision.is_some(), "a constant generator still decides");
            }
        }

        #[test]
        fn difficulty_moves_where_work_starts() {
            use Difficulty::{Easy, Hard, Medium};
            assert_eq!(
                start_tier(PromptCategory::Question, Some(Easy)),
                ModelTier::Fast
            );
            assert_eq!(
                start_tier(PromptCategory::Question, Some(Medium)),
                ModelTier::Standard
            );
            assert_eq!(
                start_tier(PromptCategory::SmallChange, Some(Hard)),
                ModelTier::Standard
            );
            assert_eq!(
                start_tier(PromptCategory::Planning, Some(Easy)),
                ModelTier::Standard,
                "a plan is never easy enough for the fast tier"
            );

            let empty = GateStats::default();
            let start = |category, difficulty| {
                pick(
                    &empty,
                    &Work::turn(category).of(difficulty),
                    &claude(),
                    &no_exploring(),
                    &mut || 0.0,
                )
                .unwrap()
                .model
                .id
            };
            assert_eq!(start(PromptCategory::Question, Some(Easy)), HAIKU);
            assert_eq!(start(PromptCategory::Question, None), SONNET);
            assert_eq!(start(PromptCategory::SmallChange, Some(Hard)), SONNET);

            let cascaded = |difficulty| {
                pick(
                    &empty,
                    &Work::turn(PromptCategory::Debugging).of(difficulty),
                    &claude(),
                    &cascading(),
                    &mut || 0.0,
                )
                .unwrap()
                .model
                .id
            };
            assert_eq!(cascaded(None), HAIKU);
            assert_eq!(
                cascaded(Some(Hard)),
                SONNET,
                "hard work fails too often on the fast tier to start there"
            );
        }

        #[test]
        fn a_models_record_at_a_difficulty_replaces_the_assumed_scale() {
            let haiku = claude().into_iter().find(|c| c.id == HAIKU).unwrap();
            let hard = Work::turn(PromptCategory::Debugging).of(Some(Difficulty::Hard));
            let mean = |stats: &GateStats| belief(stats, &hard, &haiku, 10).mean();
            let assumed = mean(&GateStats::default());
            assert!((assumed - (1.0 - 0.3 * 1.6)).abs() < 1e-9, "{assumed}");

            let mut log = Log::default();
            for i in 0..12 {
                log.rated_turn(PromptCategory::Writing, HAIKU, Difficulty::Hard, i % 6 != 0);
            }
            let learned = mean(&log.stats());
            assert!(
                learned > assumed,
                "haiku did well on hard work elsewhere: {learned} > {assumed}"
            );
            let unrated = belief(
                &log.stats(),
                &Work::turn(PromptCategory::Debugging),
                &haiku,
                10,
            )
            .mean();
            assert!(learned < unrated, "hard is still harder than usual");
        }

        #[test]
        fn belief_draws_average_to_its_mean() {
            let belief = Belief {
                alpha: 3.0,
                beta: 7.0,
            };
            let mut rng = SplitMix64::seeded(3);
            let draws = 20_000;
            let sum: f64 = (0..draws).map(|_| belief.sample(&mut rng)).sum();
            let mean = sum / f64::from(draws);
            assert!((mean - 0.3).abs() < 0.01, "{mean}");
            let small = Belief {
                alpha: 0.4,
                beta: 0.2,
            };
            let sum: f64 = (0..draws).map(|_| small.sample(&mut rng)).sum();
            assert!((sum / f64::from(draws) - small.mean()).abs() < 0.02);
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
