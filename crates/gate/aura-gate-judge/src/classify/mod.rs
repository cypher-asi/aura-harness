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
//! [`LABEL_TOOL`] with an [`Assessment`]: its reasoning first, then a
//! category and what decides how capable a model the work needs, each
//! from a fixed list. The difficulty is worked out from those
//! ([`difficulty_of`]). Local models, which may not support a forced
//! tool, name a category and a difficulty in text. A reply that does not
//! give both is no label: Gate does not route work it could not classify.
//!
//! - `prompt`: the system prompt, the label tool and the message the classifier is sent
//! - `label`: the label read out of its reply, its tool call or its text
//! - `difficulty`: the difficulty an assessment adds up to

#[cfg(test)]
mod choice_tests;
mod difficulty;
mod label;
mod prompt;
#[cfg(test)]
mod tests;

pub use difficulty::{difficulty_of, points, EASY_AT_MOST, HARD_AT_LEAST};

use aura_core_types::{Assessment, Difficulty, PromptCategory};
use aura_gate_route::Candidate;
use aura_model::model_catalog::{ModelBackend, ModelTier};
use aura_model::{ModelCallPurpose, ModelRequest, ReasonerError, ToolChoice};

use crate::request::{answers_in_text, Ask};

pub use label::{label_of, reply_shown};
pub use prompt::{LABEL_TOOL, SYSTEM_PROMPT};

/// What the classifier says of a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// What the prompt asks for.
    pub category: PromptCategory,
    /// How hard its work looks.
    pub difficulty: Difficulty,
    /// What the classifier found about the work, which `difficulty` was
    /// worked out from; `None` for a text answer, which names it.
    pub assessment: Option<Assessment>,
}

/// The model that classifies prompts: a fast-tier candidate, a local one
/// first and then one on a linked Claude or ChatGPT plan, as calls on
/// either cost nothing more; else the cheapest. `None` when there is
/// none; then the caller uses its local default. `candidates` must be
/// ordered as [`aura_gate_route::candidates`] returns them.
#[must_use]
pub fn classifier(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates
        .iter()
        .filter(|c| c.tier == ModelTier::Fast)
        // `min_by_key` keeps the first of equals: the cheapest of a rank.
        .min_by_key(|c| match c.backend {
            ModelBackend::Local => 0,
            ModelBackend::Claude | ModelBackend::Chatgpt => 1,
            ModelBackend::Router | ModelBackend::OpenRouter => 2,
        })
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
