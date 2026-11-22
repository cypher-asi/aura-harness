//! The candidates, settings and routing helpers the unit tests share.

use aura_config::GateConfig;
use aura_core_auth::ProviderKind;
use aura_core_types::{Difficulty, PromptCategory, RouteReason};
use aura_model::model_catalog::{catalog, infer_tier, ModelBackend, ModelOption, ModelTier};

use crate::stats::GateStats;
use crate::{candidates, pick, Candidate, Work};

pub(crate) const LOCAL: &str = "local/qwen3:4b";
pub(crate) const HAIKU: &str = "claude/claude-haiku-4-5";
pub(crate) const SONNET: &str = "claude/claude-sonnet-4-6";
pub(crate) const OPUS: &str = "claude/claude-opus-4-7";

/// The newest Sonnet of [`versions`].
pub(crate) const NEW_SONNET: &str = "claude/claude-sonnet-5-5";
/// A Sonnet of [`versions`] that [`NEW_SONNET`] supersedes.
pub(crate) const OLD_SONNET: &str = "claude/claude-sonnet-5";
/// The newest Opus of [`versions`].
pub(crate) const NEW_OPUS: &str = "claude/claude-opus-5-5";

/// The ids of `candidates`, in order.
pub(crate) fn ids(candidates: &[Candidate]) -> Vec<&str> {
    candidates.iter().map(|c| c.id.as_str()).collect()
}

/// Whether a provider is among the signed-in `providers`.
pub(crate) fn signed_in(providers: &[ProviderKind]) -> impl Fn(ProviderKind) -> bool + '_ {
    move |p| providers.contains(&p)
}

/// The catalog, with the local model installed.
pub(crate) fn with_local() -> Vec<ModelOption> {
    catalog(LOCAL, &["qwen3:4b".to_string()])
}

/// The candidates with every sign-in, and the local model.
pub(crate) fn all() -> Vec<Candidate> {
    let providers = [
        ProviderKind::Zos,
        ProviderKind::Claude,
        ProviderKind::Chatgpt,
    ];
    candidates(&with_local(), signed_in(&providers), |_| false)
}

/// The candidates with the Claude plan linked, and the local model.
pub(crate) fn claude() -> Vec<Candidate> {
    candidates(&with_local(), signed_in(&[ProviderKind::Claude]), |_| false)
}

/// Claude plan models by name, as [`candidates`] orders them.
pub(crate) fn plan(names: &[&str]) -> Vec<Candidate> {
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

/// Claude plan models over several versions of each.
pub(crate) fn versions() -> Vec<Candidate> {
    plan(&[
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
    ])
}

/// `candidates` but those of the fast tier.
pub(crate) fn no_fast_tier(candidates: &[Candidate]) -> Vec<Candidate> {
    candidates
        .iter()
        .filter(|c| c.tier != ModelTier::Fast)
        .cloned()
        .collect()
}

/// Routing by success rate only, without exploring.
pub(crate) fn no_exploring() -> GateConfig {
    GateConfig::defaults().without_exploring()
}

/// A turn asking for `category` the classifier found easy.
pub(crate) const fn easy(category: PromptCategory) -> Work<'static> {
    Work::turn(category).of(Some(Difficulty::Easy))
}

/// Where an unrated turn asking for `category` goes, without exploring.
pub(crate) fn chosen(
    stats: &GateStats,
    category: PromptCategory,
    candidates: &[Candidate],
) -> (String, RouteReason) {
    chosen_at(stats, &Work::turn(category), candidates)
}

/// Where `work` goes, without exploring.
pub(crate) fn chosen_at(
    stats: &GateStats,
    work: &Work<'_>,
    candidates: &[Candidate],
) -> (String, RouteReason) {
    let decision = pick(stats, work, candidates, &no_exploring(), &mut || 0.0).unwrap();
    (decision.model.id, decision.reason)
}

/// Take in `n` unrated turns asking for `category` on `model` that all
/// passed (`ok`) or all failed.
pub(crate) fn turns(
    stats: &mut GateStats,
    n: usize,
    category: PromptCategory,
    model: &str,
    ok: bool,
) {
    stats.learn(&Work::turn(category), model, None, (n, ok));
}

/// Take in one turn of `work` on `model` that passed (`ok`) or failed,
/// costing `cost_usd`.
pub(crate) fn priced_turn(
    stats: &mut GateStats,
    work: &Work<'_>,
    model: &str,
    ok: bool,
    cost_usd: f64,
) {
    stats.learn_costing(work, model, None, (1, ok), (cost_usd, 0.0));
}

/// Ten medium multi-file turns the newest Sonnet of [`versions`] failed.
pub(crate) fn new_sonnet_failing() -> GateStats {
    let mut stats = GateStats::default();
    let work = Work::turn(PromptCategory::MultiFileChange).of(Some(Difficulty::Medium));
    stats.learn(&work, NEW_SONNET, None, (10, false));
    stats
}
