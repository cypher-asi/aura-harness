//! One prompt of a run, sent as a turn: classified, routed as the run's
//! arm routes it, recorded with route reason `compare`, then attempted
//! until an attempt stands.
//!
//! A Gate run's turn is followed up and judged as a routed turn is: an
//! attempt that checked nothing gets Gate's follow-up, work about to
//! stand is reviewed when it is hard or shows a sign of failure, and an
//! attempt that failed goes to the judge, then on in the same
//! conversation to the next stronger model. The run's own check is not
//! consulted, as no turn outside a benchmark has one. An attempt whose
//! model's route refused the user's sign-in goes on on Gate's next
//! choice, as a routed turn's does, and the run's later turns leave that
//! model out; a pinned or by-difficulty run stops there.
//!
//! - `route`: the prompt classified, routed, announced and recorded
//! - `attempts`: the attempts at its turn, and how the turn ended
//! - `refused`: the move off a route that refused the user's sign-in
//! - `carry`: Gate's follow-up of an attempt, and the same model carrying
//!   on with the work
//! - `stronger`: the judge of a failed attempt, and the move to a
//!   stronger model

mod attempts;
mod carry;
mod refused;
mod route;
mod stronger;

use super::turn::RunParts;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::gate::followup::Before;
use attempts::Attempting;
use aura_core_types::{Hash, OutcomePayload, RunTurn};

/// Prompt `i` of the `of` a run sends.
pub(super) struct Prompt<'a> {
    pub text: &'a str,
    pub i: usize,
    pub of: usize,
    /// What the copy held before it, when a review of a Gate run's work
    /// may look at what its turn changed
    pub before: Option<Before>,
    /// Models whose routes refused the user's sign-in earlier in the run,
    /// which its turn leaves out; it adds those that refuse it
    pub refused: &'a mut Vec<String>,
}

/// One turn of a run: what its prompt got.
pub(super) struct Turned {
    /// The prompt as recorded; `None` when it could not be recorded
    pub prompt_tx: Option<Hash>,
    pub reply: String,
    pub error: Option<String>,
    /// Its outcome as recorded, for the check's verdict to follow
    pub ended: Option<OutcomePayload>,
    /// Whether the model's provider cut its last attempt short, so the
    /// work the check looks at was never finished
    pub cut_short: bool,
    /// The turn as the run's record keeps it, but for what it cost, which
    /// the run measures
    pub turn: RunTurn,
}

/// Send `prompt` on `session` as a turn of `parts`' run, and attempt it
/// until an attempt stands.
pub(super) async fn run_prompt(
    parts: &RunParts<'_>,
    session: &mut AgentSession,
    prompt: Prompt<'_>,
) -> Turned {
    let routed = match route::route(parts, session, prompt).await {
        Ok(routed) => routed,
        Err(turned) => return turned,
    };
    let mut attempting = Attempting::start(parts, session, routed);
    let ended = attempting.run().await;
    attempting.end(ended).await
}
