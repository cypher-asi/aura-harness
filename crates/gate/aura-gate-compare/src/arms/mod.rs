//! What each arm of a comparison runs on, Gate or a model, and the model
//! Gate is compared with by default.

#[cfg(test)]
mod tests;

use aura_core_types::ArmSpec;
use aura_gate_route::Candidate;
use aura_model::model_catalog::{model_key, ModelTier};

/// What `--against` takes to compare Gate with Gate whose lead does not
/// have to delegate (as with `AURA_GATE_DELEGATE=off`).
pub const NO_DELEGATE: &str = ArmSpec::NO_DELEGATE;

/// What `--a` takes for Gate whose lead must delegate, whatever
/// `AURA_GATE_DELEGATE` says.
const DELEGATE: &str = "gate:delegate";

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
    strongest(candidates.iter().filter(|c| {
        c.id.to_ascii_lowercase().contains(&query) || c.label.to_ascii_lowercase().contains(&query)
    }))
    .ok_or_else(|| {
        let available: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        let available = if available.is_empty() {
            "none (sign in or link a plan)".to_string()
        } else {
            available.join(", ")
        };
        format!("No available model matches `{query}`; available: {available}")
    })
}

/// The arm `query` (an `--a` or `--against` value) names among
/// `candidates`: Gate (whose lead delegates as `delegate` says, unless
/// the query says), else a model as `resolve_against` finds it.
fn resolve_arm(query: &str, candidates: &[Candidate], delegate: bool) -> Result<ArmSpec, String> {
    match query.trim().to_ascii_lowercase().as_str() {
        "gate" => Ok(ArmSpec::Gate { delegate }),
        NO_DELEGATE => Ok(ArmSpec::Gate { delegate: false }),
        DELEGATE => Ok(ArmSpec::Gate { delegate: true }),
        _ => resolve_against(query, candidates).map(|c| ArmSpec::Model {
            model: c.id.clone(),
        }),
    }
}

/// Both arms of a comparison from their queries. Plain `gate` delegates
/// as `delegate` says, except against [`NO_DELEGATE`], where it always
/// delegates so the comparison measures delegation itself.
///
/// # Errors
/// Says which models are available when a query names none of them, or
/// that both arms run on the same thing.
pub fn resolve_arms(
    (a, b): (&str, &str),
    candidates: &[Candidate],
    delegate: bool,
) -> Result<(ArmSpec, ArmSpec), String> {
    let off = |query: &str| query.trim().eq_ignore_ascii_case(NO_DELEGATE);
    let a_spec = resolve_arm(a, candidates, delegate || off(b))?;
    let b_spec = resolve_arm(b, candidates, delegate || off(a))?;
    if a_spec == b_spec {
        return Err(format!(
            "Both arms run on {}; pick two different ones",
            a_spec.label()
        ));
    }
    Ok((a_spec, b_spec))
}
