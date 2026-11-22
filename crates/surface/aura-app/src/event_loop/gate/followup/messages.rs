//! What the model is told when a step sends it back to its work, marked
//! as the harness's, and what the user is told of it.

use crate::event_loop::steering::SteerSource;
use aura_gate::evidence::Flag;
use aura_gate::followup::{self, NUDGE_PROMPT};
use aura_model::model_catalog::route_label;

/// `text` marked as the harness's, so it reads apart from the user's
/// prompts.
fn from_harness(text: &str) -> String {
    format!("{}{text}", SteerSource::Harness.prefix())
}

/// The message of a [`Step::Nudge`](aura_gate::followup::Step::Nudge)
/// for `uncovered`, what the change added that no test names: asked to
/// test that when there is any, else to check the change at all.
pub(crate) fn nudge_message(uncovered: &[String]) -> String {
    if uncovered.is_empty() {
        return from_harness(NUDGE_PROMPT);
    }
    from_harness(&followup::uncovered_prompt(uncovered))
}

/// What the user is told when `model`'s change is nudged for
/// `uncovered`.
pub(crate) fn nudge_notice(model: &str, uncovered: &[String]) -> String {
    let model = route_label(model);
    match uncovered {
        [] => format!(
            "gate: {model} changed files without running a test or build; asking it to check its change"
        ),
        [name, ..] => format!(
            "gate: {model}'s tests passed but none names `{name}`{}, which it added; asking it to test the new code",
            if uncovered.len() > 1 {
                format!(" or {} other", uncovered.len() - 1)
            } else {
                String::new()
            }
        ),
    }
}

/// The message that sends work a review found unfinished back to its
/// model with the `findings`.
pub(crate) fn repair_message(findings: &str) -> String {
    from_harness(&followup::repair_prompt(findings))
}

/// The message that sends work back to its model for the `signs` it
/// shows.
pub(crate) fn sign_repair_message(signs: &[Flag]) -> String {
    from_harness(&followup::sign_repair_prompt(signs))
}

/// What the user is told when work goes back to `model` for `signs`.
pub(crate) fn sign_repair_notice(model: &str, signs: &[Flag]) -> String {
    let names: Vec<&str> = signs.iter().map(|sign| sign.name()).collect();
    format!(
        "gate: sending {}'s work back to it ({})",
        route_label(model),
        names.join(", ")
    )
}

/// What the user is told when work a review found unfinished goes back
/// to `model`.
pub(crate) fn repair_notice(model: &str) -> String {
    format!(
        "gate: sending the review's findings back to {}",
        route_label(model)
    )
}
