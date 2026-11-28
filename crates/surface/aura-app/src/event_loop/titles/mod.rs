//! Chat titles for the agents list.
//!
//! After an agent's first completed turn a small model names what the chat
//! is about ("Task build issue"), shown in place of placeholder names like
//! `Agent 2`. The call goes through the agent's kernel like every model
//! call, after the turn, while the turn's task still owns the session.
//!
//! Subtasks get a subtitle, shown as `<code>: <subtask>` (`REN36: Parser
//! escapes`): the description the `task` call gave, else one the same
//! model writes from the subtask's prompt, else one made from the
//! prompt's first meaningful sentence ([`subtask`]). Agents a cloud
//! agent spawned on its machine skip the model: they are named from the
//! description or the prompt as they are listed here.
//!
//! INBOX threads named after the first line of their opening message (a
//! `task` prompt, or a message sent without a subject) get a subject the
//! same way, sent to the UI as [`UiCommand::SetThreadSubject`]. When the
//! model gives none the thread keeps the subject it has, so no fallback is
//! sent.

use super::subagents::AgentHub;
use aura_agent::titles::{
    fallback_title, sanitize, MAX_SIDE_CHARS, MAX_TITLE_CHARS, TITLE_TIMEOUT,
};
use aura_agent::{AgentLoopConfig, KernelModelGateway};
use aura_context_prompts::auxiliary::chat_title::{
    chat_title_user_prompt, mail_subject_user_prompt, subtask_title_user_prompt,
    CHAT_TITLE_SYSTEM_PROMPT, MAIL_SUBJECT_SYSTEM_PROMPT, SUBTASK_TITLE_SYSTEM_PROMPT,
};
use aura_core_types::text::truncate_with_ellipsis;
use aura_core_types::{AgentId, BenchSource};
use aura_model::{Message, ModelProvider, Role};
use aura_ui::UiCommand;
use std::sync::Arc;
pub(super) use subtask::{described_subtitle, fallback_subtitle, subtitle_of_spawn};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Environment variable naming the model that writes titles.
const TITLE_MODEL_ENV: &str = "AURA_TITLE_MODEL";
/// Longest subtask name kept, in characters. Subtask rows sit indented in
/// the tree after the helper's code, so they get less room than chats.
const MAX_SUBTITLE_CHARS: usize = 28;
/// Longest INBOX thread subject kept, in characters. Thread rows share
/// their width with a count, the participants and a time.
const MAX_SUBJECT_CHARS: usize = 48;
/// What a generated title names, which sets its prompt and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitleKind {
    /// A chat in the agents list.
    Chat,
    /// A subtask a spawned helper works on, shown after its code.
    Subtask,
    /// An INBOX thread, from the message that opened it.
    Subject,
}

impl TitleKind {
    fn system_prompt(self) -> &'static str {
        match self {
            Self::Chat => CHAT_TITLE_SYSTEM_PROMPT,
            Self::Subtask => SUBTASK_TITLE_SYSTEM_PROMPT,
            Self::Subject => MAIL_SUBJECT_SYSTEM_PROMPT,
        }
    }

    fn user_prompt(self, prompt: &str, reply: &str) -> String {
        match self {
            Self::Chat => chat_title_user_prompt(prompt, reply),
            Self::Subtask => subtask_title_user_prompt(prompt),
            Self::Subject => mail_subject_user_prompt(prompt),
        }
    }

    fn max_chars(self) -> usize {
        match self {
            Self::Chat => MAX_TITLE_CHARS,
            Self::Subtask => MAX_SUBTITLE_CHARS,
            Self::Subject => MAX_SUBJECT_CHARS,
        }
    }

    fn fallback(self, prompt: &str) -> String {
        match self {
            Self::Chat | Self::Subject => fallback_title(prompt),
            Self::Subtask => fallback_subtitle(prompt),
        }
    }
}

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

/// Name the subtask `agent` was spawned for from its task `prompt` in the
/// background, the way [`spawn`] titles a chat, so the Agents panel shows
/// `<code>: <subtask>` while the subtask still runs.
pub(super) fn spawn_subtitle(
    hub: Arc<AgentHub>,
    agent: AgentId,
    gateway: KernelModelGateway,
    config: AgentLoopConfig,
    model: String,
    prompt: String,
) {
    tokio::spawn(async move {
        let subtitle =
            generate_kind(TitleKind::Subtask, &gateway, &config, &model, &prompt, &[]).await;
        hub.apply_subtitle(agent, &subtitle).await;
    });
}

/// Write a subject for INBOX thread `thread_id` from `text`, the message
/// that opened it, in the background, and send it to the UI over
/// `commands`. Nothing is sent when the model gives no subject: the thread
/// already has one made from `text`.
pub(super) fn spawn_subject(
    commands: mpsc::Sender<UiCommand>,
    thread_id: String,
    gateway: KernelModelGateway,
    config: AgentLoopConfig,
    model: String,
    text: String,
) {
    tokio::spawn(async move {
        if let Some(subject) = subject_for(&gateway, &config, &model, &text).await {
            // A closed channel means the UI is gone; nobody is left to show it.
            let _ = commands
                .send(UiCommand::SetThreadSubject { thread_id, subject })
                .await;
        }
    });
}

/// The subject `model` writes for a thread opened by `text`, or `None`
/// when the call fails, says nothing usable or times out.
async fn subject_for(
    provider: &dyn ModelProvider,
    config: &AgentLoopConfig,
    model: &str,
    text: &str,
) -> Option<String> {
    ask(TitleKind::Subject, provider, config, model, text, &[]).await
}

/// The subtitle of a subtask spawned under a benchmark's run: the start of
/// its `prompt`, without a model call, as [`bench_title`] names the
/// benchmark, so a run costs only what it measures.
pub(super) fn bench_subtitle(prompt: &str) -> String {
    fallback_subtitle(prompt)
}

/// A title for the chat that opened with `prompt` and got `messages` in
/// reply, written by `model` through `provider`. Falls back to the start
/// of `prompt` when the call fails, says nothing usable or takes longer
/// than [`TITLE_TIMEOUT`].
///
/// Logs never carry the prompt: a title the model wrote is logged, one
/// made from the prompt is not.
pub(super) async fn generate(
    provider: &dyn ModelProvider,
    config: &AgentLoopConfig,
    model: &str,
    prompt: &str,
    messages: &[Message],
) -> String {
    generate_kind(TitleKind::Chat, provider, config, model, prompt, messages).await
}

/// [`generate`] for a title of `kind`: its prompt, its length and its
/// fallback.
async fn generate_kind(
    kind: TitleKind,
    provider: &dyn ModelProvider,
    config: &AgentLoopConfig,
    model: &str,
    prompt: &str,
    messages: &[Message],
) -> String {
    match ask(kind, provider, config, model, prompt, messages).await {
        Some(title) => title,
        None => {
            // Made from the prompt, so the title itself is not logged.
            info!(%model, "Titled chat from its prompt");
            kind.fallback(prompt)
        }
    }
}

/// The title of `kind` the model writes, or `None` when the call fails,
/// says nothing usable or takes longer than [`TITLE_TIMEOUT`].
async fn ask(
    kind: TitleKind,
    provider: &dyn ModelProvider,
    config: &AgentLoopConfig,
    model: &str,
    prompt: &str,
    messages: &[Message],
) -> Option<String> {
    let reply = last_reply(messages);
    let request = aura_agent::titles::request(
        model,
        kind.system_prompt(),
        kind.user_prompt(
            &truncate_with_ellipsis(prompt, MAX_SIDE_CHARS),
            &truncate_with_ellipsis(&reply, MAX_SIDE_CHARS),
        ),
        config,
    );
    let titled = match request {
        Ok(request) => {
            match tokio::time::timeout(TITLE_TIMEOUT, provider.complete(request)).await {
                Ok(Ok(response)) => sanitize(&response.message.text_content(), kind.max_chars()),
                Ok(Err(e)) => {
                    warn!(%model, error = %e, "Could not title the chat");
                    None
                }
                Err(_) => {
                    warn!(
                        %model,
                        timeout_secs = TITLE_TIMEOUT.as_secs(),
                        "The title model did not answer in time; titling from the prompt"
                    );
                    None
                }
            }
        }
        Err(e) => {
            warn!(%model, error = %e, "Could not build the chat title request");
            None
        }
    };
    if let Some(title) = &titled {
        info!(%model, %title, "Titled chat");
    }
    titled
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

/// The title of an agent that ran a benchmark of `task` from `source`
/// before it got one: what the BENCH tab names the benchmark, without a
/// model call.
pub(super) fn bench_title(source: &BenchSource, task: &str) -> String {
    match source {
        BenchSource::Session { title, .. } => fallback_title(title),
        BenchSource::Prompt => fallback_title(task),
        BenchSource::Examples { .. } => fallback_title(&format!("Bench: {task}")),
    }
}

mod subtask;
#[cfg(test)]
mod tests;
