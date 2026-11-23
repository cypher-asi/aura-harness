//! Gate's prompt classifier: one short call to a cheap model that labels
//! a prompt (a turn's, or a subtask's) with one of
//! [`PromptCategory::CLASSES`] and how hard its work looks.
//!
//! This module builds the request and reads the reply; the caller sends
//! the request through its agent's kernel, so the call is recorded like
//! any other model call. The prompt reaches the model normalized, cut to
//! its start and end, and fenced as text to label, so a prompt that reads
//! as instructions ("Your task is to implement...") is not taken for the
//! classifier's own task. The model answers by calling the
//! [`LABEL_TOOL`], whose arguments can only be one category and one
//! difficulty; local models, which may not support a forced tool, answer
//! in text. A reply that does not name both a category and a difficulty
//! is no label: Gate does not route work it could not classify.
//!
//! - `prompt`: the system prompt, the label tool and the message the classifier is sent
//! - `label`: the label read out of its reply, its tool call or its text

mod label;
mod prompt;
#[cfg(test)]
mod tests;

use aura_core_types::{Difficulty, PromptCategory};
use aura_gate_route::Candidate;
use aura_model::model_catalog::ModelTier;
use aura_model::{ModelCallPurpose, ModelRequest, ReasonerError, ToolChoice};

use crate::request::{answers_in_text, Ask};

pub use label::{label_of, reply_shown};
pub use prompt::{LABEL_TOOL, SYSTEM_PROMPT};

/// What the classifier says of a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label {
    /// What the prompt asks for.
    pub category: PromptCategory,
    /// How hard its work looks.
    pub difficulty: Difficulty,
}

/// The model that classifies prompts: the cheapest fast-tier candidate.
/// `None` when there is none; then the caller uses its local default.
/// `candidates` must be ordered as [`aura_gate_route::candidates`]
/// returns them.
#[must_use]
pub fn classifier(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates.iter().find(|c| c.tier == ModelTier::Fast)
}

/// The request that asks `model` to classify `prompt`, sent with the
/// user's `auth_token` when the route needs one. It sets no temperature:
/// Claude 5 models reject any but the default.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn request(
    model: &str,
    prompt: &str,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    let in_text = answers_in_text(model);
    let message = prompt::message(prompt, in_text);
    let ask = Ask::new(ModelCallPurpose::Classify, SYSTEM_PROMPT, message);
    if in_text {
        ask
    } else {
        ask.tool(prompt::label_tool(), ToolChoice::tool(LABEL_TOOL))
    }
    .request(model, auth_token)
}
