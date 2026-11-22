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
//!
//! Gate picks the reasoning effort with the model: work starts at the
//! effort its difficulty calls for on the model it goes to
//! ([`effort_on`]), and only work that failed on the strongest model is
//! tried again at a higher one, up to max ([`escalate`]). At max one call
//! can think for minutes, so it is kept for work nothing less got right.

use std::collections::HashMap;

use aura_config::GateConfig;
use aura_core_auth::ProviderKind;
use aura_core_types::{Difficulty, PromptCategory, Route, RouteLevel, RouteReason};
use aura_model::model_catalog::{
    estimate_cost_usd, has_max_effort, infer_tier, model_key, route_label, supersedes,
    ModelBackend, ModelOption, ModelTier,
};
use aura_model::{ThinkingEffort, Usage};

use crate::stats::{GateStats, Row};

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

/// Whether none of `candidates` supersedes `candidate`: none is a later
/// version of the same model on the same route that costs no more
/// ([`supersedes`]).
fn is_current(candidate: &Candidate, candidates: &[Candidate]) -> bool {
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
pub fn current(candidates: &[Candidate]) -> Vec<Candidate> {
    candidates
        .iter()
        .filter(|c| is_current(c, candidates))
        .cloned()
        .collect()
}

/// The model every turn goes to until Gate learns per category: the
/// cheapest standard-tier candidate, else the strongest one (the cheapest
/// of that tier), leaving out those a later version supersedes
/// ([`current`]). `candidates` must be ordered as [`candidates`] returns
/// them.
#[must_use]
pub fn fixed_pick(candidates: &[Candidate]) -> Option<&Candidate> {
    let current = || candidates.iter().filter(|c| is_current(c, candidates));
    current()
        .find(|c| c.tier == ModelTier::Standard)
        .or_else(|| current().rev().max_by_key(|c| c.tier))
}

/// The tier work asking for `category`, of `difficulty` when the
/// classifier said, starts on before Gate has learned better.
///
/// Only work the classifier found easy starts on the fast tier, plans
/// excepted; everything else starts on the standard one: plans, whatever
/// their difficulty, and medium, hard or unrated work of any kind, small
/// changes included. A failing test or build moves code work up a tier,
/// and a category whose start model fails too often starts higher
/// ([`pick`]). The frontier tier is reached that way, not by default: on
/// the work it is asked for, it costs several times a standard model for
/// results the checks rarely tell apart.
#[must_use]
pub const fn start_tier(category: PromptCategory, difficulty: Option<Difficulty>) -> ModelTier {
    match (category, difficulty) {
        (PromptCategory::Planning, _) => ModelTier::Standard,
        (_, Some(Difficulty::Easy)) => ModelTier::Fast,
        _ => ModelTier::Standard,
    }
}

/// The reasoning effort work of `difficulty` starts at, on whichever
/// model it goes to: low for easy work, high for hard work, medium for
/// the rest and when the classifier did not say. Never max: that is
/// reached only by [`escalate`].
#[must_use]
pub const fn start_effort(difficulty: Option<Difficulty>) -> ThinkingEffort {
    match difficulty {
        Some(Difficulty::Easy) => ThinkingEffort::Low,
        Some(Difficulty::Hard) => ThinkingEffort::High,
        Some(Difficulty::Medium) | None => ThinkingEffort::Medium,
    }
}

/// The reasoning effort `work` starts at on `model`: that of its
/// difficulty ([`start_effort`]), except on the fast tier, where code
/// work ([`verifiable`]) reasons at medium effort or above and other work
/// does not think. At low effort a model may skip thinking on steps it
/// finds simple, and code is where that costs a failed attempt.
#[must_use]
pub const fn effort_on(work: &Work<'_>, model: &Candidate) -> ThinkingEffort {
    let effort = start_effort(work.difficulty);
    match model.tier {
        ModelTier::Fast if !verifiable(work.category) => ThinkingEffort::Off,
        ModelTier::Fast => match effort {
            ThinkingEffort::Off | ThinkingEffort::Minimal | ThinkingEffort::Low => {
                ThinkingEffort::Medium
            }
            effort => effort,
        },
        _ => effort,
    }
}

/// The effort after `effort` on `model`'s ladder (low, medium, high,
/// max), when Gate may still raise it; `None` at the top. Max is a step
/// only on a model that reasons harder there than at high
/// ([`has_max_effort`]).
#[must_use]
pub fn raise_effort(effort: ThinkingEffort, model: &str) -> Option<ThinkingEffort> {
    match effort {
        ThinkingEffort::Off | ThinkingEffort::Minimal | ThinkingEffort::Low => {
            Some(ThinkingEffort::Medium)
        }
        ThinkingEffort::Medium => Some(ThinkingEffort::High),
        ThinkingEffort::High | ThinkingEffort::XHigh => {
            has_max_effort(model).then_some(ThinkingEffort::Max)
        }
        ThinkingEffort::Max => None,
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
    /// How hard the model reasons on it.
    pub effort: ThinkingEffort,
    /// Why: [`RouteReason::Proven`], [`RouteReason::Default`] or
    /// [`RouteReason::Explore`].
    pub reason: RouteReason,
    /// Whether Gate explored instead of taking its pick.
    pub explored: bool,
    /// Probability that Gate's policy makes this choice.
    pub propensity: f64,
}

impl Decision {
    /// The model, who serves it and its effort, e.g.
    /// `claude-sonnet-4-6 (claude plan) at high effort`, or
    /// `claude-haiku-5-5 (claude plan) with thinking off`.
    #[must_use]
    pub fn label(&self) -> String {
        let model = self.model.route_label();
        match self.effort.reasoning_effort_wire() {
            Some(effort) => format!("{model} at {effort} effort"),
            None => format!("{model} with thinking off"),
        }
    }

    /// The decision as recorded on the prompt of work at `level`.
    #[must_use]
    pub fn route(&self, level: RouteLevel) -> Route {
        Route {
            level,
            model: self.model.id.clone(),
            reason: self.reason,
            explored: self.explored,
            propensity: self.propensity,
            effort: self.effort.reasoning_effort_wire().map(str::to_string),
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

/// Tier, observed cost and list price of each priced, non-local model of
/// `candidates` that `observed` has a cost for.
fn observed_prices<'a>(
    candidates: impl IntoIterator<Item = &'a Candidate>,
    observed: impl Fn(&Candidate) -> Option<f64>,
) -> Vec<(ModelTier, f64, f64)> {
    candidates
        .into_iter()
        .filter(|c| c.backend != ModelBackend::Local)
        .filter_map(|c| Some((c.tier, observed(c)?, c.cost_usd.filter(|&p| p > 0.0)?)))
        .collect()
}

/// How observed costs compare with list prices among the `tried` models
/// ([`observed_prices`]) of `tier`, or of the nearest tiers when none of
/// its own was tried. `None` when nothing was.
fn price_scale(tried: &[(ModelTier, f64, f64)], tier: ModelTier) -> Option<f64> {
    let nearest = tried.iter().map(|(t, ..)| tiers_apart(*t, tier)).min();
    let (seen, listed) = tried
        .iter()
        .filter(|(t, ..)| Some(tiers_apart(*t, tier)) == nearest)
        .fold((0.0, 0.0), |(seen, listed), (_, o, l)| {
            (seen + o, listed + l)
        });
    (seen > 0.0 && listed > 0.0).then(|| seen / listed)
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
    let tried = observed_prices(candidates, observed);
    let expected = |c: &Candidate| {
        if c.backend == ModelBackend::Local {
            Some(0.0)
        } else {
            observed(c).or_else(|| {
                c.cost_usd
                    .map(|p| p * price_scale(&tried, c.tier).unwrap_or(1.0))
            })
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
/// [`by_expected_cost`]. `None` when there are no candidates. Work that
/// is not easy starts no lower than [`start_floor`], whether proven, by
/// default or explored, and no work goes to a model a later version
/// supersedes ([`current`]). The work runs at the effort of its
/// difficulty on the model it goes to ([`effort_on`]).
pub fn pick(
    stats: &GateStats,
    work: &Work<'_>,
    candidates: &[Candidate],
    settings: &GateConfig,
    rng: &mut impl Random,
) -> Option<Decision> {
    let candidates = &current(candidates);
    let Work {
        level,
        category,
        kind,
        ..
    } = *work;
    let row = |model: &str| judged(stats, level, category, kind, model, settings.min_tries);
    let order = by_expected_cost(stats, level, candidates);
    let floor = start_floor(work.difficulty, order.iter().map(|c| c.tier));
    let may_start = |c: &Candidate| floor.is_none_or(|floor| c.tier >= floor);
    let proven = order.iter().position(|c| {
        may_start(c)
            && row(&c.id).is_some_and(|row| {
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
            effort: effort_on(work, order[index]),
            reason,
            explored,
            propensity,
        })
    };
    let cheaper: Vec<usize> = (0..index).filter(|&i| may_start(order[i])).collect();
    let Some(&nearest) = cheaper.last() else {
        return decide(index, reason, false, 1.0);
    };
    if rate > 0.0 && rng.unit() < rate {
        let open = cheaper.iter().rev().copied().find(|&cheaper| {
            row(&order[cheaper].id)
                .is_none_or(|row| row.can_still_reach(settings.target, settings.min_tries))
        });
        return decide(open.unwrap_or(nearest), RouteReason::Explore, true, rate);
    }
    decide(index, reason, false, 1.0 - rate)
}

/// Where work at `level` that failed on `failed` at `effort` goes next:
/// the cheapest model (by [`by_expected_cost`]) of the lowest tier above
/// `failed`'s, among those no later version supersedes ([`current`]), at
/// the same effort, or at medium when it ran with thinking off (not every
/// stronger model can turn thinking off). With no stronger model, the
/// same model one effort higher ([`raise_effort`]), so max is the last
/// step on the strongest model. `effort` is `None` when Gate does not choose the effort (the
/// user pinned one): then only the model moves. `None` when nothing is
/// left to try.
#[must_use]
pub fn escalate(
    stats: &GateStats,
    level: RouteLevel,
    (failed, effort): (&str, Option<ThinkingEffort>),
    candidates: &[Candidate],
) -> Option<Decision> {
    let ran = candidates.iter().find(|c| c.id == failed);
    let tier = ran.map_or_else(|| infer_tier(failed), |c| c.tier);
    let current = current(candidates);
    let stronger: Vec<&Candidate> = by_expected_cost(stats, level, &current)
        .into_iter()
        .filter(|c| c.tier > tier)
        .collect();
    let decide = |model: &Candidate, effort| {
        Some(Decision {
            model: model.clone(),
            effort,
            reason: RouteReason::Escalate,
            explored: false,
            propensity: 1.0,
        })
    };
    let Some(next) = stronger.iter().map(|c| c.tier).min() else {
        let effort = raise_effort(effort?, failed)?;
        return decide(ran?, effort);
    };
    let model = stronger.into_iter().find(|c| c.tier == next)?;
    let effort = effort.filter(|&effort| effort != ThinkingEffort::Off);
    decide(model, effort.unwrap_or(ThinkingEffort::Medium))
}

/// Whether work at `level` that `ran` on a model, at an effort as
/// [`escalate`] takes it, has a stronger model to go on to among
/// `candidates`. One effort higher on the same model is not one: work on
/// the strongest model that checked nothing gets only the judge's
/// verdict ([`crate::followup`]).
#[must_use]
pub fn has_stronger_model(
    stats: &GateStats,
    level: RouteLevel,
    ran: (&str, Option<ThinkingEffort>),
    candidates: &[Candidate],
) -> bool {
    escalate(stats, level, ran, candidates).is_some_and(|next| next.model.id != ran.0)
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

/// The weakest tier work of `difficulty` may start on, among candidates of
/// `tiers`: the standard tier for all but easy work (unrated work counts
/// as medium) when a model at least that strong is there, else any. Below
/// it, a model's own test passes on work it got wrong (tests it wrote for
/// its own code, or tests that miss the change), so the work is never
/// moved up.
fn start_floor(
    difficulty: Option<Difficulty>,
    mut tiers: impl Iterator<Item = ModelTier>,
) -> Option<ModelTier> {
    (difficulty != Some(Difficulty::Easy) && tiers.any(|tier| tier >= ModelTier::Standard))
        .then_some(ModelTier::Standard)
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
                prompt.category = Some(category);
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
                self.priced_turn((category, Some(difficulty)), model, ok, 0.01);
            }

            /// A turn asking for `category` of `difficulty` that cost
            /// `cost_usd`.
            fn priced_turn(
                &mut self,
                (category, difficulty): (PromptCategory, Option<Difficulty>),
                model: &str,
                ok: bool,
                cost_usd: f64,
            ) {
                let agent = AgentId::generate();
                let mut prompt = PromptPayload::new(format!("turn {}", self.entries.len()));
                prompt.category = Some(category);
                prompt.route = Route::turn(model, RouteReason::Default);
                prompt.difficulty = difficulty;
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
                    cost_usd,
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
                    prompt.category = Some(category);
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
                    &Work::new(RouteLevel::Subtask, category, kind).of(Some(Difficulty::Easy)),
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

        /// Where an unrated turn asking for `category` goes, without
        /// exploring.
        fn chosen(
            stats: &GateStats,
            category: PromptCategory,
            candidates: &[Candidate],
        ) -> (String, RouteReason) {
            chosen_at(stats, &Work::turn(category), candidates)
        }

        /// Where `work` goes, without exploring.
        fn chosen_at(
            stats: &GateStats,
            work: &Work<'_>,
            candidates: &[Candidate],
        ) -> (String, RouteReason) {
            let decision = pick(stats, work, candidates, &no_exploring(), &mut || 0.0).unwrap();
            (decision.model.id, decision.reason)
        }

        /// A turn asking for `category` the classifier found easy.
        fn easy(category: PromptCategory) -> Work<'static> {
            Work::turn(category).of(Some(Difficulty::Easy))
        }

        #[test]
        fn a_failed_attempt_escalates_to_the_cheapest_model_one_tier_up() {
            let empty = GateStats::default();
            let up = |failed: &str, candidates: &[Candidate]| {
                escalate(&empty, RouteLevel::Subtask, (failed, None), candidates).map(|d| {
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
        fn work_starts_at_its_difficultys_effort_and_only_the_strongest_model_climbs_to_max() {
            let empty = GateStats::default();
            let effort = |difficulty, candidates: &[Candidate]| {
                pick(
                    &empty,
                    &Work::turn(PromptCategory::MultiFileChange).of(difficulty),
                    candidates,
                    &no_exploring(),
                    &mut || 0.0,
                )
                .unwrap()
                .effort
            };
            assert_eq!(
                effort(Some(Difficulty::Easy), &no_fast_tier(&claude())),
                ThinkingEffort::Low
            );
            assert_eq!(effort(None, &claude()), ThinkingEffort::Medium);
            assert_eq!(
                effort(Some(Difficulty::Medium), &claude()),
                ThinkingEffort::Medium
            );
            assert_eq!(
                effort(Some(Difficulty::Hard), &claude()),
                ThinkingEffort::High
            );

            let up = |failed: &str, effort| {
                escalate(&empty, RouteLevel::Turn, (failed, Some(effort)), &claude())
                    .map(|d| (d.model.id, d.effort))
            };
            assert_eq!(
                up(SONNET, ThinkingEffort::High),
                Some((OPUS.to_string(), ThinkingEffort::High)),
                "a stronger model first, at the same effort"
            );
            assert_eq!(
                up(HAIKU, ThinkingEffort::Off),
                Some((SONNET.to_string(), ThinkingEffort::Medium)),
                "work that ran with thinking off moves up at medium"
            );
            assert_eq!(
                up(OPUS, ThinkingEffort::Medium),
                Some((OPUS.to_string(), ThinkingEffort::High))
            );
            assert_eq!(
                up(OPUS, ThinkingEffort::High),
                Some((OPUS.to_string(), ThinkingEffort::Max)),
                "max only once high failed on the strongest model"
            );
            assert_eq!(up(OPUS, ThinkingEffort::Max), None);
            let route = escalate(
                &empty,
                RouteLevel::Turn,
                (OPUS, Some(ThinkingEffort::High)),
                &claude(),
            )
            .unwrap()
            .route(RouteLevel::Turn);
            assert_eq!(route.effort.as_deref(), Some("max"));

            let gpt = candidates(
                &catalog(LOCAL, &[]),
                signed_in(&[ProviderKind::Chatgpt]),
                |_| false,
            );
            let top = gpt.iter().max_by_key(|c| c.tier).unwrap().id.clone();
            assert_eq!(
                escalate(
                    &empty,
                    RouteLevel::Turn,
                    (&top, Some(ThinkingEffort::High)),
                    &gpt
                ),
                None,
                "no max step on a model that folds max into high"
            );
        }

        #[test]
        fn only_another_model_is_a_stronger_one() {
            let empty = GateStats::default();
            let stronger = |ran: &str, effort| {
                has_stronger_model(&empty, RouteLevel::Turn, (ran, Some(effort)), &claude())
            };
            assert!(stronger(SONNET, ThinkingEffort::High));
            assert!(
                !stronger(OPUS, ThinkingEffort::High),
                "max on the same model is not a stronger model"
            );
            assert!(!stronger(OPUS, ThinkingEffort::Max));
        }

        #[test]
        fn the_fast_tier_thinks_on_code_work_only() {
            let empty = GateStats::default();
            let start = |work: &Work<'_>, candidates: &[Candidate]| {
                pick(&empty, work, candidates, &no_exploring(), &mut || 0.0).unwrap()
            };
            for category in [
                PromptCategory::SmallChange,
                PromptCategory::MultiFileChange,
                PromptCategory::Debugging,
            ] {
                let decision = start(&easy(category), &claude());
                assert_eq!(
                    (decision.model.id.as_str(), decision.effort),
                    (HAIKU, ThinkingEffort::Medium),
                    "{category}"
                );
            }
            for category in [
                PromptCategory::Question,
                PromptCategory::Research,
                PromptCategory::Writing,
                PromptCategory::Other,
            ] {
                let decision = start(&easy(category), &claude());
                assert_eq!(
                    (decision.model.id.as_str(), decision.effort),
                    (HAIKU, ThinkingEffort::Off),
                    "{category}"
                );
                assert_eq!(
                    decision.label(),
                    "claude-haiku-4-5 (claude plan) with thinking off"
                );
                assert_eq!(decision.route(RouteLevel::Turn).effort, None);
            }

            let below_standard: Vec<Candidate> = claude()
                .into_iter()
                .filter(|c| c.tier < ModelTier::Standard)
                .collect();
            let hard = start(
                &Work::turn(PromptCategory::Debugging).of(Some(Difficulty::Hard)),
                &below_standard,
            );
            assert_eq!(
                (hard.model.id.as_str(), hard.effort),
                (HAIKU, ThinkingEffort::High),
                "harder code work it gets keeps its effort"
            );
            let other = start(&easy(PromptCategory::Question), &no_fast_tier(&claude()));
            assert_eq!(
                (other.model.id.as_str(), other.effort),
                (SONNET, ThinkingEffort::Low),
                "other tiers keep the effort of the difficulty"
            );
        }

        fn no_fast_tier(candidates: &[Candidate]) -> Vec<Candidate> {
            candidates
                .iter()
                .filter(|c| c.tier != ModelTier::Fast)
                .cloned()
                .collect()
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
                prompt.category = Some(category);
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
        fn only_easy_work_starts_on_the_cheapest_fast_model() {
            let empty = GateStats::default();
            let small = easy(PromptCategory::SmallChange);
            assert_eq!(
                chosen_at(&empty, &small, &all()),
                (HAIKU.to_string(), RouteReason::Default)
            );
            assert_eq!(
                chosen_at(&empty, &small, &no_fast_tier(&claude())).0,
                SONNET,
                "the next tier up when there is no fast model"
            );
            for difficulty in [None, Some(Difficulty::Medium), Some(Difficulty::Hard)] {
                assert_eq!(
                    chosen_at(&empty, &small.of(difficulty), &all()),
                    (SONNET.to_string(), RouteReason::Default),
                    "{difficulty:?}"
                );
            }
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
            let small = easy(PromptCategory::SmallChange);
            assert_eq!(chosen_at(&log.stats(), &small, &claude()).0, HAIKU);
            log.turns(2, PromptCategory::SmallChange, HAIKU, false);
            assert_eq!(chosen_at(&log.stats(), &small, &claude()).0, SONNET);
            log.turns(2, PromptCategory::SmallChange, SONNET, false);
            assert_eq!(chosen_at(&log.stats(), &small, &claude()).0, OPUS);
            log.turns(2, PromptCategory::SmallChange, OPUS, false);
            assert_eq!(
                chosen_at(&log.stats(), &small, &claude()).0,
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
        fn unrated_work_starts_on_the_cheapest_standard_model() {
            let empty = GateStats::default();
            for category in [
                PromptCategory::SmallChange,
                PromptCategory::Question,
                PromptCategory::MultiFileChange,
                PromptCategory::Debugging,
                PromptCategory::Research,
                PromptCategory::Writing,
                PromptCategory::Other,
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
            let question = easy(PromptCategory::Question);
            log.turns(9, PromptCategory::Question, LOCAL, true);
            assert_eq!(
                chosen_at(&log.stats(), &question, &claude()),
                (HAIKU.to_string(), RouteReason::Default),
                "nine tries are not enough"
            );
            log.turns(1, PromptCategory::Question, LOCAL, true);
            assert_eq!(
                chosen_at(&log.stats(), &question, &claude()),
                (LOCAL.to_string(), RouteReason::Proven)
            );
            assert_eq!(
                chosen_at(&log.stats(), &easy(PromptCategory::Writing), &claude()).0,
                HAIKU,
                "proven on questions says nothing about writing"
            );
            assert_eq!(
                chosen(&log.stats(), PromptCategory::Question, &claude()).0,
                SONNET,
                "nor about work that is not easy"
            );
        }

        #[test]
        fn a_proven_model_drops_back_after_failures() {
            let mut log = Log::default();
            let question = easy(PromptCategory::Question);
            log.turns(10, PromptCategory::Question, LOCAL, true);
            log.turns(1, PromptCategory::Question, LOCAL, false);
            assert_eq!(
                chosen_at(&log.stats(), &question, &claude()),
                (LOCAL.to_string(), RouteReason::Proven),
                "10 of 11 is still above 85%"
            );
            log.turns(1, PromptCategory::Question, LOCAL, false);
            assert_eq!(
                chosen_at(&log.stats(), &question, &claude()),
                (HAIKU.to_string(), RouteReason::Default),
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
                SONNET,
                "unrated work starts no lower than the standard tier"
            );
            let debugging = easy(PromptCategory::Debugging);
            assert_eq!(chosen_at(&log.stats(), &debugging, &claude()).0, HAIKU);
            log.turns(10, PromptCategory::Debugging, LOCAL, true);
            assert_eq!(chosen_at(&log.stats(), &debugging, &claude()).0, LOCAL);
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
                escalate(&empty, RouteLevel::Subtask, (SONNET, None), &claude).map(|d| d.model.id),
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
                ..GateConfig::defaults()
            };
            let debugging = easy(PromptCategory::Debugging);
            let explored = |stats: &GateStats| {
                let decision = pick(stats, &debugging, &claude(), &settings, &mut || 0.0).unwrap();
                assert_eq!(decision.reason, RouteReason::Explore);
                decision.model.id
            };
            let mut log = Log::default();
            log.turns(2, PromptCategory::Debugging, HAIKU, false);
            assert_eq!(chosen_at(&log.stats(), &debugging, &claude()).0, SONNET);
            assert_eq!(
                explored(&log.stats()),
                LOCAL,
                "Haiku is ruled out, the local model is not"
            );
            log.turns(2, PromptCategory::Debugging, LOCAL, false);
            assert_eq!(
                explored(&log.stats()),
                HAIKU,
                "with every cheaper model ruled out, the nearest gets another try"
            );
            log.turns(12, PromptCategory::Debugging, HAIKU, true);
            assert_eq!(
                chosen_at(&log.stats(), &debugging, &claude()),
                (HAIKU.to_string(), RouteReason::Proven),
                "and comes back once those tries prove it"
            );
            assert_eq!(
                pick(
                    &log.stats(),
                    &Work::turn(PromptCategory::Debugging),
                    &claude(),
                    &settings,
                    &mut || 0.0
                )
                .unwrap()
                .model
                .id,
                SONNET,
                "work that is not easy is never explored below the standard tier"
            );
        }

        #[test]
        fn exploring_takes_the_next_cheaper_model() {
            let empty = GateStats::default();
            let settings = GateConfig::defaults();
            let debugging = easy(PromptCategory::Debugging);
            let explored = pick(&empty, &debugging, &claude(), &settings, &mut || 0.0).unwrap();
            assert_eq!(explored.model.id, LOCAL);
            assert_eq!(explored.reason, RouteReason::Explore);
            assert!(explored.explored);
            assert_eq!(explored.propensity, settings.explore_rate);

            let kept = pick(&empty, &debugging, &claude(), &settings, &mut || 0.99).unwrap();
            assert_eq!(kept.model.id, HAIKU);
            assert!(!kept.explored);
            assert_eq!(kept.propensity, 1.0 - settings.explore_rate);
            assert_eq!(
                kept.route(RouteLevel::Subtask),
                Route {
                    level: RouteLevel::Subtask,
                    model: HAIKU.to_string(),
                    reason: RouteReason::Default,
                    explored: false,
                    propensity: 1.0 - settings.explore_rate,
                    effort: Some("medium".to_string()),
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
                        &easy(PromptCategory::Question),
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
                &easy(PromptCategory::Question),
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
                start_tier(PromptCategory::SmallChange, Some(Easy)),
                ModelTier::Fast
            );
            for difficulty in [None, Some(Medium), Some(Hard)] {
                assert_eq!(
                    start_tier(PromptCategory::SmallChange, difficulty),
                    ModelTier::Standard,
                    "{difficulty:?}"
                );
            }
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
            assert_eq!(start(PromptCategory::SmallChange, Some(Easy)), HAIKU);
            assert_eq!(start(PromptCategory::SmallChange, Some(Medium)), SONNET);
            assert_eq!(start(PromptCategory::SmallChange, Some(Hard)), SONNET);
        }

        #[test]
        fn work_that_is_not_easy_never_starts_below_the_standard_tier() {
            let mut log = Log::default();
            for _ in 0..12 {
                log.rated_turn(
                    PromptCategory::MultiFileChange,
                    HAIKU,
                    Difficulty::Hard,
                    true,
                );
            }
            let stats = log.stats();
            let work = Work::turn(PromptCategory::MultiFileChange);
            let not_easy = [None, Some(Difficulty::Medium), Some(Difficulty::Hard)];
            let start = |settings: &GateConfig, candidates: &[Candidate], work: &Work<'_>| {
                pick(&stats, work, candidates, settings, &mut || 0.0)
                    .unwrap()
                    .model
                    .id
            };
            for difficulty in not_easy {
                assert_eq!(
                    start(&no_exploring(), &claude(), &work.of(difficulty)),
                    SONNET,
                    "not even where it is proven: {difficulty:?}"
                );
            }
            assert_eq!(
                start(&no_exploring(), &claude(), &work.of(Some(Difficulty::Easy))),
                HAIKU,
                "easy work still starts cheap"
            );
            let below_standard: Vec<Candidate> = claude()
                .into_iter()
                .filter(|c| c.tier < ModelTier::Standard)
                .collect();
            assert_eq!(
                start(
                    &no_exploring(),
                    &below_standard,
                    &work.of(Some(Difficulty::Hard))
                ),
                HAIKU,
                "with nothing stronger it starts where it can"
            );

            let mut rng = SplitMix64::seeded(5);
            let settings = GateConfig {
                explore_rate: 1.0,
                ..GateConfig::defaults()
            };
            for difficulty in not_easy {
                for _ in 0..200 {
                    let decision =
                        pick(&stats, &work.of(difficulty), &claude(), &settings, &mut rng).unwrap();
                    assert!(
                        decision.model.tier >= ModelTier::Standard,
                        "exploring stays at or above the floor: {decision:?}"
                    );
                }
            }
        }

        /// Claude plan models by name, as [`candidates`] orders them.
        fn plan(names: &[&str]) -> Vec<Candidate> {
            let options: Vec<ModelOption> = names
                .iter()
                .map(|name| ModelOption {
                    id: format!("claude/{name}"),
                    label: (*name).to_string(),
                    backend: ModelBackend::Claude,
                    installed: false,
                    tier: infer_tier(name),
                })
                .collect();
            candidates(&options, signed_in(&[ProviderKind::Claude]), |_| false)
        }

        #[test]
        fn an_older_version_of_a_model_gets_no_work() {
            const NEW_SONNET: &str = "claude/claude-sonnet-5-5";
            const OLD_SONNET: &str = "claude/claude-sonnet-5";
            const NEW_OPUS: &str = "claude/claude-opus-5-5";
            let all = plan(&[
                "claude-fable-5-2",
                "claude-haiku-5-5",
                "claude-sonnet-5-5",
                "claude-opus-5-5",
                "claude-fable-5-1",
                "claude-opus-5",
                "claude-sonnet-5",
                "claude-fable-5",
                "claude-opus-4-8",
                "claude-sonnet-4-6",
                "claude-haiku-4-5",
            ]);
            assert_eq!(all.len(), 11, "each is still a candidate, to name outright");
            let mut kept = ids(&current(&all))
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            kept.sort();
            assert_eq!(
                kept,
                [
                    "claude/claude-fable-5-1",
                    "claude/claude-fable-5-2",
                    "claude/claude-haiku-5-5",
                    NEW_OPUS,
                    NEW_SONNET,
                ],
                "fable 5.2 has no known price, so nothing is known against 5.1"
            );
            assert_eq!(fixed_pick(&all).map(|c| c.id.as_str()), Some(NEW_SONNET));

            // The newer sonnet keeps failing: the work does not fall to the
            // older one, which nothing is known against and which lists
            // at the same price.
            let category = PromptCategory::MultiFileChange;
            let mut log = Log::default();
            for _ in 0..3 {
                log.rated_turn(category, NEW_SONNET, Difficulty::Medium, false);
            }
            let stats = log.stats();
            let sonnets_and_opus: Vec<Candidate> = all
                .iter()
                .filter(|c| c.tier >= ModelTier::Standard)
                .cloned()
                .collect();
            let work = Work::turn(category).of(Some(Difficulty::Medium));
            let picked = pick(
                &stats,
                &work,
                &sonnets_and_opus,
                &no_exploring(),
                &mut || 0.0,
            )
            .unwrap()
            .model
            .id;
            assert_eq!(picked, NEW_OPUS, "up a tier, not back a version");

            for (failed, next) in [
                ("claude/claude-haiku-5-5", NEW_SONNET),
                (NEW_SONNET, NEW_OPUS),
                (OLD_SONNET, NEW_OPUS),
            ] {
                assert_eq!(
                    escalate(&stats, RouteLevel::Turn, (failed, None), &all)
                        .unwrap()
                        .model
                        .id,
                    next,
                    "after {failed}"
                );
            }
            let mut rng = SplitMix64::seeded(11);
            let exploring = GateConfig {
                explore_rate: 1.0,
                ..GateConfig::defaults()
            };
            for difficulty in [None, Some(Difficulty::Easy), Some(Difficulty::Hard)] {
                for category in [PromptCategory::MultiFileChange, PromptCategory::Research] {
                    let work = Work::turn(category).of(difficulty);
                    for _ in 0..50 {
                        let model = pick(&stats, &work, &all, &exploring, &mut rng)
                            .unwrap()
                            .model;
                        assert!(is_current(&model, &all), "{} was explored", model.id);
                    }
                }
            }
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
