//! What Gate records about each prompt: its category and route on the
//! `UserPrompt` transaction ([`PromptPayload`]), and how the work went on
//! `Outcome` transactions ([`OutcomePayload`]) that refer back to it.
//!
//! A prompt can collect several outcomes: the one written when its turn
//! ends, then later signals (a rewind, a retry on a stronger model, the
//! user's rating). Which one counts is decided by whoever reads them; the
//! record keeps all of them.

mod category;
mod outcome;
mod prompt;
mod route;
#[cfg(test)]
mod tests;

pub use category::{Difficulty, PromptCategory};
pub use outcome::{OutcomePayload, OutcomeSignal};
pub use prompt::PromptPayload;
pub use route::{Route, RouteLevel, RouteReason};

use serde::Serialize;

fn to_json(value: &impl Serialize) -> Vec<u8> {
    // Strings, enums, booleans and numbers always serialize; an empty
    // payload would only come from a serde bug.
    serde_json::to_vec(value).unwrap_or_default()
}
