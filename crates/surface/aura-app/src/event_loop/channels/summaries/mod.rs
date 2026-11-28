//! One-line summaries of the user's long channel messages, for the
//! channel's HISTORY tab ([`UiEvent::SummarizeMessages`]).
//!
//! Each summary is written once by a small model (Claude Haiku when the
//! user can run it), kept in `<data_dir>/message_summaries.json`
//! ([`cache`]) and sent to the UI as [`UiCommand::SetMessageSummary`].
//! A message already summarized, in this run or an earlier one, is
//! answered from the file without a call.
//!
//! # Invariants
//!
//! * Calls run on a task of their own, one after another, so asking
//!   never holds up the event loop and a long history doesn't fire
//!   hundreds of calls at once.
//! * Every call is given up after [`SUMMARY_TIMEOUT`], sees at most
//!   [`MAX_SIDE_CHARS`] of the message and may answer in at most
//!   [`MAX_SUMMARY_TOKENS`] tokens.
//! * Every request gets an answer: its summary, or `None` when the call
//!   failed, timed out or there is no kernel to make it (a coordinator
//!   on its cloud machine). A `None` is not kept, so a later run tries
//!   again.
//! * Logs never carry the message or its summary.
//!
//! [`UiEvent::SummarizeMessages`]: aura_ui::UiEvent::SummarizeMessages

mod cache;
#[cfg(test)]
mod tests;

use crate::event_loop::subagents::lock;
use crate::event_loop::{titles, LoopState};
use aura_agent::titles::{sanitize, MAX_SIDE_CHARS};
use aura_agent::{AgentLoopConfig, KernelModelGateway};
use aura_context_prompts::auxiliary::message_summary::{
    message_summary_user_prompt, MESSAGE_SUMMARY_SYSTEM_PROMPT,
};
use aura_core_types::text::truncate_with_ellipsis;
use aura_core_types::time::now_ms;
use aura_model::{MaxTokens, ModelProvider, ModelRequest};
use aura_ui::events::{MessageSummaryRequest, MessageSummaryView};
use aura_ui::UiCommand;
pub(crate) use cache::SummaryCache;
use cache::{text_key, CACHE_FILE};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// How long the summary model gets to answer before the UI keeps its own
/// cut of the message.
pub(crate) const SUMMARY_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest summary kept, in characters: a row of the side panel.
const MAX_SUMMARY_CHARS: usize = 72;

/// Most tokens a summary may take, so a call costs a fraction of a cent.
const MAX_SUMMARY_TOKENS: u32 = 48;

/// What a model id is matched on to find Claude Haiku.
const HAIKU: &str = "haiku";

/// What a run of summary calls needs besides the messages.
pub(super) struct SummaryJob {
    /// Where the summaries are kept
    pub(super) path: PathBuf,
    /// The model that writes them
    pub(super) model: String,
    /// Billing and ids, as the coordinator's own calls carry
    pub(super) config: AgentLoopConfig,
    /// Where the answers go
    pub(super) commands: mpsc::Sender<UiCommand>,
}

/// [`UiEvent::SummarizeMessages`](aura_ui::UiEvent::SummarizeMessages):
/// answer `requests` in the background, through the kernel of
/// `agent_id`, the channel's coordinator.
pub(crate) fn handle(state: &LoopState<'_>, agent_id: &str, requests: Vec<MessageSummaryRequest>) {
    if requests.is_empty() {
        return;
    }
    let coordinator = state.agent_or_active(agent_id);
    let gateway = state.kernel_for(coordinator).map(KernelModelGateway::new);
    let candidates: Vec<String> = state
        .models
        .gate_candidates()
        .into_iter()
        .map(|candidate| candidate.id)
        .collect();
    let job = SummaryJob {
        path: lock(&state.hub.registry).data_dir().join(CACHE_FILE),
        model: summary_model(&candidates, state.models.classifier_model()),
        config: state.agent_loop.config().clone(),
        commands: state.commands.clone(),
    };
    let hub = Arc::clone(&state.hub);
    tokio::spawn(async move {
        let provider = gateway.as_ref().map(|g| g as &dyn ModelProvider);
        summarize_all(&hub.channel_chat.summaries, provider, &job, requests).await;
    });
}

/// The model that writes summaries: the first of `candidates` (the
/// models the user can run) that is Claude Haiku, else the one that
/// titles chats.
pub(super) fn summary_model(candidates: &[String], classifier: String) -> String {
    candidates
        .iter()
        .find(|id| id.to_ascii_lowercase().contains(HAIKU))
        .cloned()
        .unwrap_or_else(|| titles::title_model(classifier))
}

/// Answer every one of `requests`: from `cache`, else with a call to
/// `provider` (none answers `None`), keeping what it writes.
pub(super) async fn summarize_all(
    cache: &SummaryCache,
    provider: Option<&dyn ModelProvider>,
    job: &SummaryJob,
    requests: Vec<MessageSummaryRequest>,
) {
    cache.ensure_loaded(&job.path).await;
    let mut wrote = false;
    for request in requests {
        let key = text_key(&request.text);
        if let Some(summary) = cache.get(&key) {
            send(&job.commands, request.key, Some(summary)).await;
            continue;
        }
        if !cache.start(&key) {
            // Another run is summarizing the same text and answers it.
            continue;
        }
        let summary = match provider {
            Some(provider) => summarize(provider, job, &request.text).await,
            None => None,
        };
        cache.finish(&key, summary.as_deref(), now_ms());
        wrote |= summary.is_some();
        send(&job.commands, request.key, summary).await;
    }
    if wrote {
        cache.save(&job.path).await;
    }
}

/// Tell the UI the summary of the message it filed under `key`.
async fn send(commands: &mpsc::Sender<UiCommand>, key: String, summary: Option<String>) {
    let view = MessageSummaryView { key, summary };
    // A closed channel means the UI is gone; nobody is left to show it.
    let _ = commands.send(UiCommand::SetMessageSummary(view)).await;
}

/// The summary the job's model writes of `text`, or `None` when the call
/// fails, says nothing usable or takes longer than [`SUMMARY_TIMEOUT`].
async fn summarize(provider: &dyn ModelProvider, job: &SummaryJob, text: &str) -> Option<String> {
    let model = job.model.as_str();
    let request = summary_request(model, &job.config, text)?;
    match tokio::time::timeout(SUMMARY_TIMEOUT, provider.complete(request)).await {
        Ok(Ok(response)) => {
            let summary = sanitize(&response.message.text_content(), MAX_SUMMARY_CHARS);
            debug!(%model, written = summary.is_some(), "Summarized a channel message");
            summary
        }
        Ok(Err(error)) => {
            warn!(%model, %error, "Could not summarize a channel message");
            None
        }
        Err(_) => {
            let timeout_secs = SUMMARY_TIMEOUT.as_secs();
            warn!(%model, timeout_secs, "The summary model did not answer in time");
            None
        }
    }
}

/// The request asking `model` for a summary of `text`: the title call's,
/// with a short answer.
fn summary_request(model: &str, config: &AgentLoopConfig, text: &str) -> Option<ModelRequest> {
    let user = message_summary_user_prompt(&truncate_with_ellipsis(text, MAX_SIDE_CHARS));
    let built = aura_agent::titles::request(model, MESSAGE_SUMMARY_SYSTEM_PROMPT, user, config)
        .and_then(|mut request| {
            request.max_tokens = MaxTokens::try_new(MAX_SUMMARY_TOKENS)?;
            Ok(request)
        });
    match built {
        Ok(request) => Some(request),
        Err(error) => {
            warn!(%model, %error, "Could not build the message summary request");
            None
        }
    }
}
