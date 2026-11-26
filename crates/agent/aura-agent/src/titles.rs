//! Chat titles: the request a title model gets and what is kept of its
//! answer.
//!
//! The app titles a local agent's chat with these. A cloud agent's machine
//! titles its chat the same way, through the agent's kernel, so both read
//! alike in the agents list.

use crate::{AgentLoopConfig, KernelModelGateway};
use aura_context_prompts::auxiliary::chat_title::{
    chat_title_user_prompt, CHAT_TITLE_SYSTEM_PROMPT,
};
use aura_core_types::text::truncate_with_ellipsis;
use aura_model::{
    Message, ModelCallPurpose, ModelRequest, ModelRequestKind, ReasonerError, ThinkingEffort,
    ToolChoice,
};
use std::time::Duration;
use tracing::{info, warn};

/// How much of the prompt and of the reply the model sees.
pub const MAX_SIDE_CHARS: usize = 2_000;
/// Longest title kept, in characters.
pub const MAX_TITLE_CHARS: usize = 50;
/// Longest title made from the prompt when the model gives none.
pub const MAX_FALLBACK_CHARS: usize = 40;
/// How long the title model gets to answer before the title is made from
/// the prompt instead. A title is cosmetic and its call runs beside the
/// turn, so a stalled provider must not leave the job (and its kernel
/// gateway) waiting for ever; a small model answers well within this.
pub const TITLE_TIMEOUT: Duration = Duration::from_secs(15);

/// Labels a model may put before its answer, though asked not to.
const ANSWER_LABELS: [&str; 4] = ["Title:", "title:", "Subject:", "subject:"];

/// The request asking `model` for a title: `system` and `user` prompts, no
/// tools, no thinking, billed like the agent's own calls with `config`'s
/// token and ids.
///
/// # Errors
/// Fails when `model` is not a usable model name.
pub fn request(
    model: &str,
    system: &str,
    user: String,
    config: &AgentLoopConfig,
) -> Result<ModelRequest, ReasonerError> {
    ModelRequest::builder(model, system)
        .messages(vec![Message::user(user)])
        .tools(Vec::new())
        .tool_choice(ToolChoice::None)
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(config.auth_token.clone())
        .aura_project_id(config.aura_project_id.clone())
        .aura_agent_id(config.aura_agent_id.clone())
        .aura_session_id(config.aura_session_id.clone())
        .aura_org_id(config.aura_org_id.clone())
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Title)
        .try_build()
}

/// The title `model` writes for the chat that opened with `prompt`, asked
/// through `gateway` so the call is recorded with the agent's other calls.
/// `None` when the call fails, says nothing usable or takes longer than
/// [`TITLE_TIMEOUT`].
///
/// Logs never carry the prompt.
pub async fn chat_title(
    gateway: &KernelModelGateway,
    config: &AgentLoopConfig,
    model: &str,
    prompt: &str,
) -> Option<String> {
    let user = chat_title_user_prompt(&truncate_with_ellipsis(prompt, MAX_SIDE_CHARS), "");
    let request = match request(model, CHAT_TITLE_SYSTEM_PROMPT, user, config) {
        Ok(request) => request,
        Err(e) => {
            warn!(%model, error = %e, "Could not build the chat title request");
            return None;
        }
    };
    let response = gateway.answer(request, TITLE_TIMEOUT).await?;
    let title = sanitize(&response.message.text_content(), MAX_TITLE_CHARS);
    if let Some(title) = &title {
        info!(%model, %title, "Titled chat");
    }
    title
}

/// The title in the model's `reply`: its first line without quotes, a
/// `Title:` or `Subject:` label or trailing punctuation, cut to `max`
/// characters. `None` when nothing is left.
#[must_use]
pub fn sanitize(reply: &str, max: usize) -> Option<String> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = ANSWER_LABELS
        .iter()
        .find_map(|label| line.strip_prefix(label))
        .unwrap_or(line);
    let line = line
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '“' | '”'))
        .trim_end_matches(['.', '!', '?', ':', ';', ','])
        .trim();
    let title = cut_at_word(line, max);
    (!title.is_empty()).then_some(title)
}

/// A title made from the start of the user's `prompt`, for when no model
/// writes one.
#[must_use]
pub fn fallback_title(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = cut_at_word(&collapsed, MAX_FALLBACK_CHARS);
    if title.is_empty() {
        "New chat".to_string()
    } else {
        title
    }
}

/// `text` cut to at most `max` characters, at a word boundary when one
/// exists past the first half.
#[must_use]
pub fn cut_at_word(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space >= max / 2 => &cut[..space],
        _ => cut.as_str(),
    };
    cut.trim_end_matches([' ', ',', ';', ':', '-']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_only_the_title() {
        let sanitize = |reply: &str| sanitize(reply, MAX_TITLE_CHARS);
        assert_eq!(
            sanitize("\n  \"Task build issue.\"\nmore").as_deref(),
            Some("Task build issue")
        );
        assert_eq!(
            sanitize("Title: Agent trace persistence").as_deref(),
            Some("Agent trace persistence")
        );
        assert_eq!(sanitize("**Fix login**").as_deref(), Some("Fix login"));
        assert_eq!(sanitize("  \n \"\" "), None);
        let long = sanitize(&"word ".repeat(30)).unwrap();
        assert!(long.chars().count() <= MAX_TITLE_CHARS);
        assert!(!long.ends_with(' '));
    }

    #[test]
    fn fallback_uses_the_start_of_the_prompt() {
        assert_eq!(
            fallback_title("\n  fix   the build\nthen run tests"),
            "fix the build"
        );
        let long =
            fallback_title("agent name in left side should update after there is info, use Haiku");
        assert_eq!(long, "agent name in left side should update");
        assert_eq!(fallback_title("   "), "New chat");
        assert_eq!(fallback_title(&"é".repeat(60)).chars().count(), 40);
    }

    #[test]
    fn a_title_request_is_an_auxiliary_call_without_tools_or_thinking() {
        let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
        let asked = request(
            "aura-claude-haiku-4-5",
            CHAT_TITLE_SYSTEM_PROMPT,
            "the build fails".into(),
            &config,
        )
        .unwrap();
        assert_eq!(asked.metadata.kind, Some(ModelRequestKind::Auxiliary));
        assert_eq!(asked.metadata.purpose, Some(ModelCallPurpose::Title));
        assert!(asked.tools.is_empty());
        assert_eq!(asked.thinking_effort, Some(ThinkingEffort::Off));
        assert!(request("", CHAT_TITLE_SYSTEM_PROMPT, String::new(), &config).is_err());
    }
}
