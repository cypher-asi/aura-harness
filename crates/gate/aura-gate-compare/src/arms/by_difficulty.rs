//! Arms that run each piece of work on the model given for its
//! difficulty.

use aura_core_types::{ArmSpec, Difficulty, RouteReason};
use aura_gate_route::{effort_on, Candidate, Decision, Work};

use super::resolve_model;

/// What `--a` and `--against` take for an arm that runs work on a model
/// chosen by its difficulty.
pub(super) const BY_DIFFICULTY_USAGE: &str = "difficulty:easy=<model>,medium=<model>,hard=<model>";

/// The by-difficulty arm whose easy, medium and hard `queries` each name
/// a model as `resolve_model` finds it among `runnable` and
/// `candidates`.
pub(super) fn resolve_by_difficulty(
    queries: &[String; 3],
    runnable: &[Candidate],
    candidates: &[Candidate],
) -> Result<ArmSpec, String> {
    let [easy, medium, hard] = queries;
    let resolve = |query: &str, name: &str| {
        resolve_model(query, runnable, candidates)
            .map(|c| c.id.clone())
            .map_err(|error| format!("{name} model: {error}"))
    };
    Ok(ArmSpec::ByDifficulty {
        easy: resolve(easy, "Easy")?,
        medium: resolve(medium, "Medium")?,
        hard: resolve(hard, "Hard")?,
    })
}

/// The models a by-difficulty arm runs easy, medium and hard work on.
#[derive(Debug, Clone, PartialEq)]
pub struct DifficultyRoutes {
    /// The model easy work runs on.
    pub easy: Candidate,
    /// The model medium work, and work of unknown difficulty, runs on.
    pub medium: Candidate,
    /// The model hard work runs on.
    pub hard: Candidate,
}

impl DifficultyRoutes {
    /// The models of `spec` when it is a by-difficulty arm, as listed in
    /// `runnable`; a model `runnable` lacks gets its tier, route and
    /// price from its id. `None` for any other arm.
    #[must_use]
    pub fn of(spec: &ArmSpec, runnable: &[Candidate]) -> Option<Self> {
        let ArmSpec::ByDifficulty { easy, medium, hard } = spec else {
            return None;
        };
        let route = |id: &str| {
            runnable
                .iter()
                .find(|c| c.id == id)
                .cloned()
                .unwrap_or_else(|| Candidate::from_id(id))
        };
        Some(Self {
            easy: route(easy),
            medium: route(medium),
            hard: route(hard),
        })
    }

    /// The model work of `difficulty` runs on; medium when unknown.
    #[must_use]
    pub const fn model(&self, difficulty: Option<Difficulty>) -> &Candidate {
        match difficulty {
            Some(Difficulty::Easy) => &self.easy,
            Some(Difficulty::Hard) => &self.hard,
            Some(Difficulty::Medium) | None => &self.medium,
        }
    }

    /// Where `work` goes: the model of its difficulty, at the effort its
    /// difficulty starts at there ([`effort_on`]), recorded as a run of
    /// `/compare`.
    #[must_use]
    pub fn decide(&self, work: &Work<'_>) -> Decision {
        let model = self.model(work.difficulty).clone();
        Decision {
            effort: effort_on(work, &model),
            model,
            reason: RouteReason::Compare,
            explored: false,
            propensity: 1.0,
        }
    }
}
