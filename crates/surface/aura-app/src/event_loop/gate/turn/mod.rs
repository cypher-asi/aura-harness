//! Gate's part of a terminal turn whose prompt it labels: the classifier
//! says what the prompt asks for, Gate picks the model the turn runs on
//! and whether it is led, and the prompt is recorded with what Gate
//! decided. The turn's attempts then go through Gate's follow-up and on
//! to stronger models until one stands, and its outcome is recorded once
//! the turn's result is handled ([`Ending`]).
//!
//! - `label`: what the prompt is recorded with, and its classification
//! - `route`: Gate's choice of model and way of working, and how it shows
//! - `attempts`: the attempts at the turn, from the first to the last
//! - `ending`: the outcome a turn is recorded with once it ends

mod attempts;
mod ending;
mod label;
mod route;
#[cfg(test)]
mod tests;

pub(crate) use ending::Ending;
pub(crate) use label::Routing;
pub(crate) use route::GateTurn;

use crate::event_loop::agents::AgentSession;
use crate::event_loop::turns::{begin_files, model_message, Prompt, Ran, TurnParts};
use attempts::{Asked, Current};
use aura_core_types::time::now_ms;
use aura_gate::live;
use aura_ui::UiCommand;
use label::Label;
use std::time::Instant;

/// Run the turn of `prompt` on `session` as Gate labels it, the turn
/// having begun at `started`: the prompt is classified and, when Gate
/// routes the turn, held to criteria and routed; then it is recorded
/// and the attempts at the turn run (see [`attempts::run`]). Returns how
/// the last attempt ended; `None` when the prompt could not be
/// classified or recorded, which the chat shows as the turn's error.
pub(crate) async fn run(
    session: &mut AgentSession,
    mut prompt: Prompt,
    turn: &mut TurnParts<'_>,
    started: Instant,
) -> Option<Ran> {
    let started_ms = now_ms();
    let gate = turn.gate.take();
    let mut label = Label::new(prompt.text.clone(), &turn.routing, turn.agent_loop.config());
    let category = match label.classify(session).await {
        Ok(category) => category,
        Err(problem) => return fail(turn, problem).await,
    };
    let mut criteria = None;
    if let Some(gate) = gate.as_deref() {
        criteria = gate.criteria(session, &mut label, category, turn.ui).await;
        if let Some(note) = live::worker_note_for(criteria.as_deref()) {
            prompt.text.push_str(&note);
        }
        gate.route(session, turn, &mut label, category).await;
    }
    let prompt_tx = match label.record(session, category).await {
        Ok(prompt_tx) => prompt_tx,
        Err(problem) => return fail(turn, problem).await,
    };
    begin_files(session, &label.prompt.text);
    if let Some(files) = &session.file_checkpoints {
        files.note_prompt_tx(session.agent_id(), prompt_tx);
    }
    let first = Current {
        prompt_tx,
        turn_start: session.messages.len(),
        started,
    };
    session.messages.push(model_message(session, prompt).await);
    let asked = Asked {
        task: &label.prompt.text,
        category,
        difficulty: label.prompt.difficulty,
        led: label.prompt.led,
        criteria,
        started_ms,
        first,
    };
    Some(attempts::run(session, turn, gate, asked).await)
}

/// End the turn on `problem`, shown as its error.
async fn fail(turn: &TurnParts<'_>, problem: String) -> Option<Ran> {
    turn.ui.send(UiCommand::ShowError(problem)).await;
    turn.ui.send(UiCommand::Complete).await;
    None
}

/// Milliseconds since `started`.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
