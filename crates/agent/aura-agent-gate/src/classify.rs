//! Gate's prompt classifier: one short call to a cheap model that labels
//! a prompt (a turn's, or later a subtask's) with one of
//! [`PromptCategory::CLASSES`] and how hard its work looks.
//!
//! This module builds the request and reads the reply; the caller sends
//! the request through its agent's kernel, so the call is recorded like
//! any other model call. Whatever goes wrong — the call fails, or the
//! reply names no category — the prompt is
//! [`PromptCategory::Unclassified`], with no difficulty.

use aura_core_types::{Difficulty, PromptCategory};
use aura_model::model_catalog::ModelTier;
use aura_model::{
    Message, ModelCallPurpose, ModelRequest, ModelRequestKind, ModelResponse, ReasonerError,
    ThinkingEffort, ToolChoice,
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
You label requests sent to a coding agent. Reply with exactly one label from this list, a space, \
and one difficulty, and nothing else (for example `debugging hard`). Use only the words listed.
Labels:
question: a quick question or explanation
small_change: a small code change in one place, whether a fix, a tweak or a small feature
multi_file_change: a change across several files, such as a new feature, an extension or a refactor
debugging: fixing a failure (an error, a failing test, a bug)
research: investigating a codebase or topic
planning: designing or planning work
writing: prose, docs or messages
other: anything else
Difficulties:
easy: routine, clearly specified work
medium: ordinary work
hard: subtle, ambiguous or wide-reaching work that is easy to get wrong";

/// What the classifier says of a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label {
    /// What the prompt asks for.
    pub category: PromptCategory,
    /// How hard its work looks; `None` when the reply did not say.
    pub difficulty: Option<Difficulty>,
}

impl Label {
    /// No usable answer: unclassified, difficulty unknown.
    pub const UNCLASSIFIED: Self = Self {
        category: PromptCategory::Unclassified,
        difficulty: None,
    };
}

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
        .message(Message::user(format!("Request:\n{prompt}\n\n{}", answer_cue())))
        .tool_choice(ToolChoice::None)
        .max_tokens(MAX_REPLY_TOKENS)
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(auth_token)
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Classify)
        .try_build()
}

/// What the request ends with: the labels and difficulties again. After a
/// long prompt a cheap model loses track of the list and makes labels up
/// (`feature`, `implementation`).
fn answer_cue() -> String {
    let labels = PromptCategory::CLASSES.map(PromptCategory::as_str);
    let difficulties = Difficulty::ALL.map(Difficulty::as_str);
    format!(
        "Label ({}) and difficulty ({}):",
        either(&labels),
        either(&difficulties)
    )
}

/// `words` as a list ending in `or`: `a, b or c`.
fn either(words: &[&str]) -> String {
    match words.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => words.concat(),
    }
}

/// What the classifier's `response` says.
#[must_use]
pub fn label_of(response: &ModelResponse) -> Label {
    let reply = response.message.text_content();
    let category = parse(&reply);
    Label {
        category,
        difficulty: parse_difficulty(&reply).filter(|_| category != PromptCategory::Unclassified),
    }
}

/// The words of `reply`'s answer, lowercase: what follows a local model's
/// `<think>` block, since it may weigh several labels before answering.
fn answer_words(reply: &str) -> Vec<String> {
    reply
        .rsplit_once("</think>")
        .map_or(reply, |(_, after)| after)
        .to_ascii_lowercase()
        .replace('-', "_")
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The first category named in `reply`, [`PromptCategory::Unclassified`]
/// when it names none.
#[must_use]
pub fn parse(reply: &str) -> PromptCategory {
    answer_words(reply)
        .iter()
        .find_map(|word| {
            PromptCategory::from_name(word).filter(|c| *c != PromptCategory::Unclassified)
        })
        .unwrap_or(PromptCategory::Unclassified)
}

/// The first difficulty named in `reply`, if any.
#[must_use]
pub fn parse_difficulty(reply: &str) -> Option<Difficulty> {
    answer_words(reply)
        .iter()
        .find_map(|word| Difficulty::from_name(word))
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
    fn a_difficulty_after_the_label_is_read() {
        assert_eq!(parse_difficulty("debugging hard"), Some(Difficulty::Hard));
        assert_eq!(parse_difficulty("Question: EASY."), Some(Difficulty::Easy));
        assert_eq!(
            parse_difficulty("<think>easy? no</think>\nplanning medium"),
            Some(Difficulty::Medium)
        );
        assert_eq!(parse_difficulty("planning"), None);
        assert_eq!(parse("small_change easy"), PromptCategory::SmallChange);
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
        assert!(sent.len() < MAX_PROMPT_CHARS + 200, "prompt is cut short");
        assert!(
            sent.ends_with(
                "\n\nLabel (question, small_change, multi_file_change, debugging, research, \
                 planning, writing or other) and difficulty (easy, medium or hard):"
            ),
            "the labels come again after the prompt: {sent}"
        );
        for category in PromptCategory::CLASSES {
            assert!(built.system.contains(category.as_str()), "{category}");
        }
        for difficulty in Difficulty::ALL {
            assert!(built.system.contains(difficulty.as_str()), "{difficulty}");
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
