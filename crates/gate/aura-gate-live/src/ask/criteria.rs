//! The acceptance criteria of a turn: who writes them, the call that
//! does, and what the worker is told of them.

use super::reason_within;
use crate::table::GateLive;
use aura_agent_kernel::Kernel;
use aura_config::ReviewMode;
use aura_core_types::{Difficulty, PromptCategory};
use aura_gate_evidence::verifiable;
use aura_gate_judge::criteria::{self, Criteria};
use aura_gate_route::Candidate;
use aura_model::model_catalog::route_label;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// How long writing a turn's acceptance criteria may take before the turn
/// goes on without them.
const CRITERIA_TIMEOUT: Duration = Duration::from_secs(60);

/// The model that writes the acceptance criteria of a turn asking for
/// `category`, Gate routing it among `candidates`
/// ([`criteria::writer`]). `None` when the criteria are switched off, the
/// work is not code, or no model could review the work against them.
fn criteria_writer_for(
    live: &GateLive,
    candidates: &[Candidate],
    category: PromptCategory,
) -> Option<String> {
    let settings = live.settings();
    if settings.criteria == ReviewMode::Off || !settings.judge || !verifiable(category) {
        return None;
    }
    aura_gate_compare::default_against(candidates)?;
    criteria::writer(candidates).map(|writer| writer.id.clone())
}

/// Ask `model` through `kernel`, so the call is recorded, for the
/// acceptance criteria of `text`.
///
/// # Errors
/// Says why, as a line for the chat, when the request could not be
/// built, the call failed or timed out, or its reply held no criteria
/// ([`criteria::none_because`]). The work then goes on without criteria,
/// reviewed as it would be without them.
async fn write_criteria(
    kernel: &Kernel,
    model: &str,
    text: &str,
    auth_token: Option<String>,
) -> Result<Option<Arc<Criteria>>, String> {
    let failed = |error: String| {
        warn!(%model, %error, "Could not write the acceptance criteria");
        Err(format!(
            "gate: {} wrote no acceptance criteria: {error}",
            route_label(model)
        ))
    };
    let request = match criteria::request(model, text, auth_token) {
        Ok(request) => request,
        Err(e) => return failed(format!("could not build the request ({e})")),
    };
    let response = match reason_within(kernel, request, CRITERIA_TIMEOUT).await {
        Ok(response) => response,
        Err(e) => return failed(e.to_string()),
    };
    let Some(found) = criteria::criteria_of(&response) else {
        return failed(criteria::none_because(&response.message));
    };
    info!(%model, criteria = found.items.len(), strict = found.strict, "Wrote the acceptance criteria");
    Ok(Some(Arc::new(found)))
}

/// The acceptance criteria of `text`, a prompt asking for `category` that
/// Gate routes among `candidates`, written through `kernel` when Gate's
/// settings call for them (`criteria_writer_for`). Work they find
/// exacting is rated hard in `difficulty`, with criteria `log` as with
/// `on`, so it starts on a model that can meet them. `Ok(None)` when none
/// are due.
///
/// # Errors
/// Says why, as a line for the chat, when the writer could not be asked
/// (`write_criteria`).
pub async fn hold_to_criteria(
    kernel: &Kernel,
    (live, candidates): (&GateLive, &[Candidate]),
    category: PromptCategory,
    difficulty: &mut Option<Difficulty>,
    text: &str,
    auth_token: Option<String>,
) -> Result<Option<Arc<Criteria>>, String> {
    let Some(writer) = criteria_writer_for(live, candidates, category) else {
        return Ok(None);
    };
    let Some(found) = write_criteria(kernel, &writer, text, auth_token).await? else {
        return Ok(None);
    };
    if found.exacting() && *difficulty != Some(Difficulty::Hard) {
        info!(was = ?difficulty, "The acceptance criteria rate the work hard");
        *difficulty = Some(Difficulty::Hard);
    }
    Ok(Some(found))
}

/// What the worker is told of `criteria` after its prompt: the list,
/// whether or not a review will act on it, so that what the request
/// demands in passing is in front of the model doing the work. `None`
/// when none were written.
pub fn worker_note_for(criteria: Option<&Criteria>) -> Option<String> {
    Some(format!("\n\n{}", criteria::worker_note(criteria?)))
}
