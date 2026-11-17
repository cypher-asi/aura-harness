//! Gate's prompt classifier: one short call to a cheap model that labels
//! a prompt (a turn's, or later a subtask's) with one of
//! [`PromptCategory::CLASSES`].
//!
//! This module builds the request and reads the reply; the caller sends
//! the request through its agent's kernel, so the call is recorded like
//! any other model call. Whatever goes wrong — the call fails, or the
//! reply names no category — the prompt is
//! [`PromptCategory::Unclassified`].

use aura_core_types::PromptCategory;
use aura_model::model_catalog::ModelTier;
use aura_model::{
    Message, ModelRequest, ModelRequestKind, ModelResponse, ReasonerError, ThinkingEffort,
    ToolChoice,
};

use crate::router::Candidate;

/// Longest prompt prefix the classifier sees, in characters. The start of
/// a prompt says what it asks for; long pastes only add cost.
const MAX_PROMPT_CHARS: usize = 4_000;

/// Room for the label, plus a little for models that add words.
const MAX_REPLY_TOKENS: u32 = 32;

/// System prompt of every classifier request; tells them apart from the
/// agent's own calls.
pub const SYSTEM_PROMPT: &str = "\
You label requests sent to a coding agent. Reply with exactly one label from this list and nothing else:
question: a quick question or explanation
small_change: a small code change in one place
multi_file_change: a change across several files
debugging: fixing a failure (an error, a failing test, a bug)
research: investigating a codebase or topic
planning: designing or planning work
writing: prose, docs or messages
other: anything else";

/// The model that classifies prompts: the cheapest fast-tier candidate.
/// `None` when there is none; then the caller uses its local default.
/// `candidates` must be ordered as [`crate::candidates`] returns them.
#[must_use]
pub fn classifier(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates.iter().find(|c| c.tier == ModelTier::Fast)
}

/// The request that asks `model` to classify `prompt`, sent with the
/// user's `auth_token` when the route needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn request(
    model: &str,
    prompt: &str,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    let prompt: String = prompt.chars().take(MAX_PROMPT_CHARS).collect();
    ModelRequest::builder(model, SYSTEM_PROMPT)
        .message(Message::user(format!("Request:\n{prompt}\n\nLabel:")))
        .tool_choice(ToolChoice::None)
        .max_tokens(MAX_REPLY_TOKENS)
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(auth_token)
        .request_kind(ModelRequestKind::Auxiliary)
        .try_build()
}

/// The category the classifier's `response` names.
#[must_use]
pub fn category_of(response: &ModelResponse) -> PromptCategory {
    parse(&response.message.text_content())
}

/// The first category named in `reply`, [`PromptCategory::Unclassified`]
/// when it names none. A local model's `<think>` block is skipped, since
/// it may weigh several labels before answering.
#[must_use]
pub fn parse(reply: &str) -> PromptCategory {
    let answer = reply
        .rsplit_once("</think>")
        .map_or(reply, |(_, after)| after)
        .to_ascii_lowercase()
        .replace('-', "_");
    answer
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .find_map(|word| {
            PromptCategory::from_name(word).filter(|c| *c != PromptCategory::Unclassified)
        })
        .unwrap_or(PromptCategory::Unclassified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::candidates;
    use aura_core_auth::ProviderKind;
    use aura_model::model_catalog::catalog;

    const LOCAL: &str = "local/qwen3:4b";

    #[test]
    fn plain_labels_parse() {
        for category in PromptCategory::CLASSES {
            assert_eq!(parse(category.as_str()), category);
        }
        assert_eq!(parse("  Debugging\n"), PromptCategory::Debugging);
        assert_eq!(parse("multi-file_change"), PromptCategory::MultiFileChange);
    }

    #[test]
    fn labels_inside_a_sentence_or_after_thinking_parse() {
        assert_eq!(
            parse("Label: small_change."),
            PromptCategory::SmallChange,
            "the first label wins"
        );
        assert_eq!(
            parse("<think>maybe question, or research</think>\nplanning"),
            PromptCategory::Planning
        );
    }

    #[test]
    fn replies_without_a_label_are_unclassified() {
        assert_eq!(parse(""), PromptCategory::Unclassified);
        assert_eq!(parse("I can't tell"), PromptCategory::Unclassified);
        assert_eq!(parse("unclassified"), PromptCategory::Unclassified);
        assert_eq!(
            parse("smallchange"),
            PromptCategory::Unclassified,
            "near misses are not guessed"
        );
    }

    #[test]
    fn the_request_is_short_and_has_no_tools() {
        let long = "x".repeat(MAX_PROMPT_CHARS * 2);
        let built = request("claude/claude-haiku-4-5", &long, Some("jwt".into())).unwrap();
        assert_eq!(built.model.as_str(), "claude/claude-haiku-4-5");
        assert!(built.tools.is_empty());
        assert!(matches!(built.tool_choice, ToolChoice::None));
        assert_eq!(built.max_tokens.get(), MAX_REPLY_TOKENS);
        assert_eq!(built.auth_token.as_deref(), Some("jwt"));
        let sent = built.messages[0].text_content();
        assert!(sent.len() < MAX_PROMPT_CHARS + 100, "prompt is cut short");
        for category in PromptCategory::CLASSES {
            assert!(built.system.contains(category.as_str()), "{category}");
        }
        assert!(request("", "hi", None).is_err());
    }

    #[test]
    fn the_cheapest_fast_model_classifies() {
        let options = catalog(LOCAL, &["qwen3:4b".to_string()]);
        let all = candidates(
            &options,
            |p| matches!(p, ProviderKind::Zos | ProviderKind::Claude),
            |_| false,
        );
        assert_eq!(
            classifier(&all).map(|c| c.id.as_str()),
            Some("claude/claude-haiku-4-5")
        );
        let local_only = candidates(&options, |_| false, |_| false);
        assert_eq!(classifier(&local_only), None);
    }
}
