//! What an arm is called where room is short, and how the turns and
//! subtasks of its runs are routed.

use super::plan::ComparePlan;
use crate::event_loop::subagents::ArmRouting;
use aura_core_types::{ArmSpec, CompareArm};
use aura_gate::compare::DifficultyRoutes;
use std::borrow::Cow;

/// How an arm is named where room is short: its label, or `by difficulty`
/// for an arm whose label lists a model per difficulty.
pub(super) fn short_label(spec: &ArmSpec) -> Cow<'_, str> {
    if spec.is_by_difficulty() {
        Cow::Borrowed("by difficulty")
    } else {
        spec.label()
    }
}

/// What the agent grouping `arm`'s runs, on `spec`, is called:
/// `A · gate`, `B · claude-opus-5-5`, `B · by difficulty`.
pub(super) fn arm_name(arm: CompareArm, spec: &ArmSpec) -> String {
    let label = short_label(spec);
    let short = label.rsplit('/').next().unwrap_or(&label);
    format!("{} · {short}", arm.as_str().to_uppercase())
}

/// How the turns and subtasks of a run of the arm `spec` are routed.
pub(super) fn arm_routing(spec: &ArmSpec, plan: &ComparePlan) -> ArmRouting {
    match (spec, DifficultyRoutes::of(spec, &plan.runnable)) {
        (_, Some(routes)) => ArmRouting::ByDifficulty(routes),
        (ArmSpec::Gate { delegate }, None) => ArmRouting::Gate {
            candidates: plan.candidates.clone(),
            delegate: *delegate,
        },
        (ArmSpec::Model { model }, None) => ArmRouting::Pinned(model.clone()),
        // `DifficultyRoutes::of` gives routes for every by-difficulty arm.
        (ArmSpec::ByDifficulty { medium, .. }, None) => ArmRouting::Pinned(medium.clone()),
    }
}
