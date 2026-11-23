//! Asking Gate's reviewer again about attempts already in the log, for
//! `aura gate replay`, so a review can be scored against what the tasks'
//! own checks proved before it is trusted with live turns.
//!
//! An attempt can be replayed when Gate chose its model, the model is
//! below the strongest tier, Gate let the work stand, a benchmark task's
//! check then proved it right or wrong, and its run's copy of the
//! workspace is still kept ([`pick`]). The caller reads what the run left
//! (its saved session and its copy) and asks the reviewer; this module
//! says what the reviewer is shown and what each answer came to.
//!
//! - `prepared`: an attempt as the reviewer is shown it ([`Prepared`]),
//!   why it is reviewed, and what asking about it may cost
//! - `asked`: what a review came to ([`Asked`]), as a row, and the
//!   [`tally`] of them all

use crate::{below_strongest, stood, CopyAt, Line, Stood, TruthSource};
use aura_core_types::Difficulty;
use aura_gate_judge::turn::ReviewVariant;
use aura_model::model_catalog::{infer_tier, ModelTier};
use std::collections::{BTreeMap, HashSet};

mod asked;
mod prepared;
#[cfg(test)]
mod tests;

pub use asked::{tally, Asked};
pub use prepared::{estimate, Prepared};

/// Why an attempt is left out when the log does not say where its run's
/// copy is.
const COPY_NOT_NOTED: &str =
    "the log does not say where their run's copy is (starting aura notes it)";

/// Which attempts to ask about.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filter<'a> {
    /// Only attempts at benchmark tasks whose id contains this
    pub task: Option<&'a str>,
    /// Attempts of any difficulty, not only those rated hard
    pub any_difficulty: bool,
    /// Ask again about attempts already replayed the same way
    pub again: bool,
    /// At most this many attempts
    pub limit: Option<usize>,
}

/// An attempt picked for replay.
#[derive(Debug, Clone, Copy)]
pub struct Pick<'a> {
    pub stood: Stood<'a>,
    /// Where its run's copy is kept
    pub copy: &'a CopyAt,
}

/// The attempts in `lines` to replay as `filter` says, oldest first, and
/// how many were left out for each reason. `done` holds the agent and
/// prompt of the attempts already replayed the same way ([`replayed`]),
/// and `kept` says whether a run's copy is still kept.
#[must_use]
pub fn pick<'a>(
    lines: &'a [Line],
    filter: &Filter<'_>,
    done: &HashSet<(&str, &str)>,
    kept: impl Fn(&CopyAt) -> bool,
) -> (Vec<Pick<'a>>, BTreeMap<&'static str, usize>) {
    let mut left_out: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut picks = Vec::new();
    for stood in stood(lines) {
        let wanted = filter.task.is_none_or(|wanted| {
            (stood.truth.task.as_deref()).is_some_and(|task| task.contains(wanted))
        });
        if !wanted {
            continue;
        }
        if let Some(why_not) = why_not(stood, filter, done) {
            *left_out.entry(why_not).or_default() += 1;
            continue;
        }
        match &stood.truth.copy {
            Some(copy) if kept(copy) => picks.push(Pick { stood, copy }),
            Some(_) => *left_out.entry("their run's copy is not kept").or_default() += 1,
            None => *left_out.entry(COPY_NOT_NOTED).or_default() += 1,
        }
    }
    if let Some(limit) = filter.limit {
        picks.truncate(limit);
    }
    (picks, left_out)
}

/// Why the attempt of `stood` is not replayed as `filter` says, before
/// its copy is looked for; `None` when nothing but that stops it.
fn why_not(
    stood: Stood<'_>,
    filter: &Filter<'_>,
    done: &HashSet<(&str, &str)>,
) -> Option<&'static str> {
    let attempt = stood.attempt;
    if stood.truth.source != TruthSource::TaskCheck {
        Some("no benchmark task's check proved them")
    } else if !attempt.routed {
        Some("Gate did not choose their model")
    } else if !below_strongest(&attempt.model) {
        Some("on the strongest tier")
    } else if !filter.any_difficulty && attempt.difficulty != Some(Difficulty::Hard) {
        Some("not rated hard (--all takes them)")
    } else if stood.attempts_by_agent > 1 {
        Some("their run moved between models or prompts")
    } else if !filter.again && done.contains(&(attempt.agent.as_str(), attempt.prompt_tx.as_str()))
    {
        Some("already replayed this way (--again asks anew)")
    } else {
        None
    }
}

/// The agent and prompt of each attempt in `lines` that `judge` was asked
/// about again as `asked_as` (see [`asked_as`]).
#[must_use]
pub fn replayed<'a>(
    lines: &'a [Line],
    asked_as: &str,
    judge: Option<&str>,
) -> HashSet<(&'a str, &'a str)> {
    lines
        .iter()
        .filter_map(|line| match line {
            Line::Review(review)
                if review.replayed
                    && review.variant == asked_as
                    && Some(review.judge.as_str()) == judge =>
            {
                Some((review.agent.as_str(), review.prompt_tx.as_str()))
            }
            _ => None,
        })
        .collect()
}

/// How a review asked as `variant` is logged: `plain`, or
/// `plain+criteria` when the work is held to criteria, as a live review
/// is.
#[must_use]
pub fn asked_as(variant: ReviewVariant, criteria: bool) -> String {
    if criteria {
        format!("{}+criteria", variant.name())
    } else {
        variant.name().to_string()
    }
}

/// The strongest model the attempts in `lines` ran on, an Opus when one
/// is among them: the model that reviews when none is named.
#[must_use]
pub fn strongest_in(lines: &[Line]) -> Option<String> {
    let mut models: Vec<&str> = attempt_models(lines)
        .filter(|model| infer_tier(model) == ModelTier::Frontier)
        .collect();
    models.sort_unstable();
    models.dedup();
    models
        .iter()
        .rev()
        .find(|model| model.contains("opus"))
        .or(models.last())
        .map(|model| (*model).to_string())
}

/// The latest standard-tier model the attempts in `lines` ran on: the
/// model that writes the criteria when none is named.
#[must_use]
pub fn writer_in(lines: &[Line]) -> Option<String> {
    attempt_models(lines)
        .filter(|model| infer_tier(model) == ModelTier::Standard)
        .max()
        .map(str::to_string)
}

/// The model of each attempt in `lines`.
fn attempt_models(lines: &[Line]) -> impl Iterator<Item = &str> {
    lines.iter().filter_map(|line| match line {
        Line::Attempt(attempt) => Some(attempt.model.as_str()),
        _ => None,
    })
}
