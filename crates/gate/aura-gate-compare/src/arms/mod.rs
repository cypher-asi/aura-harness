//! What each arm of a comparison runs on: Gate, a model, or a model for
//! each difficulty of the work ([`DifficultyRoutes`]); and the model Gate
//! is compared with by default.

mod by_difficulty;
#[cfg(test)]
mod tests;

use aura_core_types::ArmSpec;
use aura_gate_route::Candidate;
use aura_model::model_catalog::{model_key, ModelTier};

pub use by_difficulty::DifficultyRoutes;
use by_difficulty::{resolve_by_difficulty, BY_DIFFICULTY_USAGE};

/// What `--against` takes to compare Gate with Gate whose lead does not
/// have to delegate (as with `AURA_GATE_DELEGATE=off`).
pub const NO_DELEGATE: &str = ArmSpec::NO_DELEGATE;

/// What `--a` takes for Gate whose lead must delegate, whatever
/// `AURA_GATE_DELEGATE` says.
const DELEGATE: &str = "gate:delegate";

/// Most model ids an error from `resolve_model` lists.
const LISTED_MODELS: usize = 20;

/// Numbers in a model's name, in order (`claude-opus-4-7` → `[4, 7]`),
/// which order its versions.
fn version(id: &str) -> Vec<u32> {
    model_key(id)
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// The strongest of `candidates`: the highest tier, then the latest
/// version, then the higher list price; the last of equals.
fn strongest<'a>(candidates: impl IntoIterator<Item = &'a Candidate>) -> Option<&'a Candidate> {
    candidates
        .into_iter()
        .map(|candidate| (candidate, version(&candidate.id)))
        .max_by(|(a, a_version), (b, b_version)| {
            a.tier
                .cmp(&b.tier)
                .then_with(|| a_version.cmp(b_version))
                .then_with(|| {
                    a.cost_usd
                        .unwrap_or_default()
                        .total_cmp(&b.cost_usd.unwrap_or_default())
                })
        })
        .map(|(candidate, _)| candidate)
}

fn is_opus(candidate: &Candidate) -> bool {
    model_key(&candidate.id).contains("opus")
}

/// The model Gate is compared with by default, which also judges: the
/// strongest Opus available, else the strongest frontier model, else the
/// strongest model there is.
#[must_use]
pub fn default_against(candidates: &[Candidate]) -> Option<&Candidate> {
    strongest(candidates.iter().filter(|c| is_opus(c)))
        .or_else(|| strongest(candidates.iter().filter(|c| c.tier == ModelTier::Frontier)))
        .or_else(|| strongest(candidates))
}

/// The strongest of `models` whose id or label contains `query`, which
/// is lowercase.
fn containing<'a>(query: &str, models: &'a [Candidate]) -> Option<&'a Candidate> {
    strongest(models.iter().filter(|c| {
        c.id.to_ascii_lowercase().contains(query) || c.label.to_ascii_lowercase().contains(query)
    }))
}

/// That no model matches `query`, with up to `limit` of the ids of
/// `models`.
fn no_match(query: &str, models: &[Candidate], limit: usize) -> String {
    let available = if models.is_empty() {
        "none (sign in or link a plan)".to_string()
    } else {
        let listed: Vec<&str> = models.iter().take(limit).map(|c| c.id.as_str()).collect();
        let more = if models.len() > limit { ", …" } else { "" };
        format!("{}{more}", listed.join(", "))
    };
    format!("No available model matches `{query}`; available: {available}")
}

/// The candidate `query` names: its id or label exactly, else the
/// strongest whose id or label contains it (`opus`, `sonnet`). Fails
/// saying which models are available when none matches.
fn resolve_against<'a>(query: &str, candidates: &'a [Candidate]) -> Result<&'a Candidate, String> {
    let query = query.trim().to_ascii_lowercase();
    let exact = candidates
        .iter()
        .find(|c| c.id.eq_ignore_ascii_case(&query) || c.label.eq_ignore_ascii_case(&query));
    if let Some(candidate) = exact {
        return Ok(candidate);
    }
    containing(&query, candidates).ok_or_else(|| no_match(&query, candidates, usize::MAX))
}

/// The model `query` names for an arm pinned to one: a model of
/// `runnable` whose id (else label) is `query` in any case, else one of
/// `candidates` as `resolve_against` finds it, else the strongest of
/// `runnable` whose id or label contains `query`. `runnable` holds every
/// model the user can run ([`aura_gate_route::runnable`]), `candidates`
/// those Gate routes to. Fails saying which models are available when
/// none matches, or when `query` names Gate itself.
fn resolve_model<'a>(
    query: &str,
    runnable: &'a [Candidate],
    candidates: &'a [Candidate],
) -> Result<&'a Candidate, String> {
    let lower = query.trim().to_ascii_lowercase();
    let exact = (runnable.iter())
        .find(|c| c.id.eq_ignore_ascii_case(&lower))
        .or_else(|| {
            runnable
                .iter()
                .find(|c| c.label.eq_ignore_ascii_case(&lower))
        });
    if let Some(candidate) = exact {
        return Ok(candidate);
    }
    if lower == "gate" {
        return Err(format!(
            "Compare Gate with another model, e.g. --against opus, or with itself \
             without enforced delegation: --against {NO_DELEGATE}"
        ));
    }
    if let Ok(candidate) = resolve_against(query, candidates) {
        return Ok(candidate);
    }
    containing(&lower, runnable).ok_or_else(|| {
        let listed = if runnable.is_empty() {
            candidates
        } else {
            runnable
        };
        no_match(&lower, listed, LISTED_MODELS)
    })
}

/// The arm `query` (an `--a` or `--against` value) names: Gate (whose
/// lead delegates as `delegate` says, unless the query says), a
/// by-difficulty arm (`difficulty:easy=<model>,medium=<model>,hard=<model>`)
/// whose models `resolve_model` finds, else a model as `resolve_model`
/// finds it among `runnable` and `candidates`.
fn resolve_arm(
    query: &str,
    runnable: &[Candidate],
    candidates: &[Candidate],
    delegate: bool,
) -> Result<ArmSpec, String> {
    match query.trim().to_ascii_lowercase().as_str() {
        "gate" => Ok(ArmSpec::Gate { delegate }),
        NO_DELEGATE => Ok(ArmSpec::Gate { delegate: false }),
        DELEGATE => Ok(ArmSpec::Gate { delegate: true }),
        _ => match ArmSpec::parse_by_difficulty(query) {
            Some(Ok(models)) => resolve_by_difficulty(&models, runnable, candidates),
            Some(Err(error)) => Err(format!("{error}; give {BY_DIFFICULTY_USAGE}")),
            None => resolve_model(query, runnable, candidates).map(|c| ArmSpec::Model {
                model: c.id.clone(),
            }),
        },
    }
}

/// Both arms of a comparison from their queries, the models they name
/// found among `runnable`, every model the user can run, and
/// `candidates`, those Gate routes to. Plain `gate` delegates as
/// `delegate` says, except against [`NO_DELEGATE`], where it always
/// delegates so the comparison measures delegation itself.
///
/// # Errors
/// Says which models are available when a query names none of them,
/// with the difficulty for a by-difficulty arm, what is wrong with a
/// malformed by-difficulty query, or that both arms run on the same
/// thing.
pub fn resolve_arms(
    (a, b): (&str, &str),
    runnable: &[Candidate],
    candidates: &[Candidate],
    delegate: bool,
) -> Result<(ArmSpec, ArmSpec), String> {
    let off = |query: &str| query.trim().eq_ignore_ascii_case(NO_DELEGATE);
    let a_spec = resolve_arm(a, runnable, candidates, delegate || off(b))?;
    let b_spec = resolve_arm(b, runnable, candidates, delegate || off(a))?;
    if a_spec == b_spec {
        return Err(format!(
            "Both arms run on {}; pick two different ones",
            a_spec.label()
        ));
    }
    Ok((a_spec, b_spec))
}

/// The model `query` (a `--classifier` value) names to classify each
/// run's prompt, found as an arm's model is ([`resolve_model`]) among
/// `runnable` and `candidates`.
///
/// # Errors
/// Says which models are available when `query` names none of them, and
/// that Gate itself cannot classify.
pub fn resolve_classifier(
    query: &str,
    runnable: &[Candidate],
    candidates: &[Candidate],
) -> Result<String, String> {
    if query.trim().eq_ignore_ascii_case("gate") {
        return Err(
            "Classifier: name a model, e.g. --classifier haiku, or leave it out for Gate's choice"
                .to_string(),
        );
    }
    resolve_model(query, runnable, candidates)
        .map(|c| c.id.clone())
        .map_err(|error| format!("Classifier: {error}"))
}
