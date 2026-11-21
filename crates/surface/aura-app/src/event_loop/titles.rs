//! Chat titles for the agents list.
//!
//! After an agent's first completed turn a small model names what the chat
//! is about ("Task build issue"), shown in place of placeholder names like
//! `Agent 2`. The call goes through the agent's kernel like every model
//! call, after the turn, while the turn's task still owns the session.

use super::subagents::AgentHub;
use aura_agent::{AgentLoopConfig, KernelModelGateway};
use aura_context_prompts::auxiliary::chat_title::{
    chat_title_user_prompt, CHAT_TITLE_SYSTEM_PROMPT,
};
use aura_core_types::AgentId;
use aura_model::{
    Message, ModelCallPurpose, ModelProvider, ModelRequest, ModelRequestKind, Role, ThinkingEffort,
    ToolChoice,
};
use std::sync::Arc;
use tracing::{info, warn};

/// Environment variable naming the model that writes titles.
const TITLE_MODEL_ENV: &str = "AURA_TITLE_MODEL";
/// How much of the prompt and of the reply the model sees.
const MAX_SIDE_CHARS: usize = 2_000;
/// Longest title kept, in characters.
const MAX_TITLE_CHARS: usize = 50;
/// Longest title made from the prompt when the model gives none.
const MAX_FALLBACK_CHARS: usize = 40;

/// The model that titles chats: `AURA_TITLE_MODEL` when set, else
/// `classifier`, the cheap model Gate classifies prompts with.
pub(super) fn title_model(classifier: String) -> String {
    title_model_with(classifier, std::env::var(TITLE_MODEL_ENV).ok().as_deref())
}

fn title_model_with(classifier: String, configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map_or(classifier, str::to_string)
}

/// Title the chat of `agent` from its first `prompt` in the background,
/// while the turn runs, so the Agents panel shows the name within a second
/// or two instead of after the turn. Only the prompt is used: a turn can
/// take minutes and the prompt already says what the chat is about.
pub(super) fn spawn(
    hub: Arc<AgentHub>,
    agent: AgentId,
    gateway: KernelModelGateway,
    config: AgentLoopConfig,
    model: String,
    prompt: String,
) {
    tokio::spawn(async move {
        let title = generate(&gateway, &config, &model, &prompt, &[]).await;
        hub.apply_title(agent, &title).await;
    });
}

/// A title for the chat that opened with `prompt` and got `messages` in
/// reply, written by `model` through `provider`. Falls back to the start
/// of `prompt` when the call fails or says nothing usable.
pub(super) async fn generate(
    provider: &dyn ModelProvider,
    config: &AgentLoopConfig,
    model: &str,
    prompt: &str,
    messages: &[Message],
) -> String {
    let reply = last_reply(messages);
    let request = ModelRequest::builder(model, CHAT_TITLE_SYSTEM_PROMPT)
        .messages(vec![Message::user(chat_title_user_prompt(
            &shorten(prompt, MAX_SIDE_CHARS),
            &shorten(&reply, MAX_SIDE_CHARS),
        ))])
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
        .try_build();
    let titled = match request {
        Ok(request) => match provider.complete(request).await {
            Ok(response) => sanitize(&response.message.text_content()),
            Err(e) => {
                warn!(%model, error = %e, "Could not title the chat");
                None
            }
        },
        Err(e) => {
            warn!(%model, error = %e, "Could not build the chat title request");
            None
        }
    };
    let title = titled.unwrap_or_else(|| fallback_title(prompt));
    info!(%model, %title, "Titled chat");
    title
}

/// Text of the latest assistant message in `messages`.
fn last_reply(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .filter(|m| m.role == Role::Assistant)
        .map(Message::text_content)
        .find(|text| !text.trim().is_empty())
        .unwrap_or_default()
}

/// The title in the model's `reply`: its first line without quotes, a
/// `Title:` label or trailing punctuation. `None` when nothing is left.
fn sanitize(reply: &str) -> Option<String> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
        .unwrap_or(line);
    let line = line
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '“' | '”'))
        .trim_end_matches(['.', '!', '?', ':', ';', ','])
        .trim();
    let title = cut_at_word(line, MAX_TITLE_CHARS);
    (!title.is_empty()).then_some(title)
}

/// A title made from the start of the user's `prompt`.
fn fallback_title(prompt: &str) -> String {
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
fn cut_at_word(text: &str, max: usize) -> String {
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

/// `text` cut to at most `max` characters, marked when cut.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_model::MockProvider;

    #[test]
    fn titles_are_written_by_the_classifier_unless_configured() {
        let haiku = || "claude/claude-haiku-5-5".to_string();
        assert_eq!(title_model_with(haiku(), None), "claude/claude-haiku-5-5");
        assert_eq!(title_model_with(haiku(), Some(" my-model ")), "my-model");
        assert_eq!(
            title_model_with(haiku(), Some("  ")),
            "claude/claude-haiku-5-5"
        );
    }

    #[test]
    fn sanitize_keeps_only_the_title() {
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
    fn last_reply_skips_empty_assistant_messages() {
        let messages = vec![
            Message::user("hi"),
            Message::assistant("Fixed the import."),
            Message::user("thanks"),
            Message::assistant(""),
        ];
        assert_eq!(last_reply(&messages), "Fixed the import.");
        assert_eq!(last_reply(&[]), "");
    }

    #[tokio::test]
    async fn generate_asks_the_title_model_and_cleans_its_answer() {
        let provider = MockProvider::simple_response("\"Task build issue.\"");
        let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
        let title = generate(
            &provider,
            &config,
            "aura-claude-haiku-4-5",
            "the build fails",
            &[Message::assistant("Fixed it.")],
        )
        .await;
        assert_eq!(title, "Task build issue");
    }
}
