//! One-line summaries of the long messages the user sent in a channel,
//! for its HISTORY tab.
//!
//! The UI asks for the messages over
//! [`aura_ui::events::MESSAGE_SUMMARY_THRESHOLD`] it has no summary of
//! ([`UiEvent::SummarizeMessages`](aura_ui::UiEvent)). Each is looked up
//! in the channel store first, keyed by a blake3 hash of its text, so a
//! message is summarized once across restarts; the rest go to a small
//! model (the cheapest Haiku, see `titles::summary_model`) through the
//! coordinator's kernel, one at a time, each stored once written.
//!
//! # Invariants
//!
//! * The event loop never waits on the model or the store: the work runs
//!   on a task of its own, and store calls on `spawn_blocking`.
//! * Every model call has `TITLE_TIMEOUT`; a failure or a timeout leaves
//!   the message without a summary, so the UI keeps the message cut at a
//!   word. Nothing is stored for it, so a later ask tries again.
//! * Summaries are the user's words: they are sealed like posts, and
//!   never logged.

use super::{blocking, ChannelStore, LoopState};
use crate::event_loop::titles;
use aura_agent::AgentLoopConfig;
use aura_model::ModelProvider;
use aura_ui::events::MessageSummary;
use aura_ui::UiCommand;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Most messages one ask summarizes; the UI asks again for the rest.
const MAX_TEXTS: usize = 200;

/// The model call a summary needs, when one can be made.
pub(super) struct Summarizer<'a> {
    pub(super) provider: &'a dyn ModelProvider,
    pub(super) config: &'a AgentLoopConfig,
    pub(super) model: &'a str,
}

/// `UiEvent::SummarizeMessages`: send the UI a summary of each of
/// `texts`, long messages the user sent in channel `channel_id`, from the
/// cache or the model, without holding up the loop.
pub(crate) async fn handle_summarize(
    state: &mut LoopState<'_>,
    channel_id: &str,
    mut texts: Vec<String>,
) {
    let Some(store) = state.hub.channels.clone() else {
        return;
    };
    texts.truncate(MAX_TEXTS);
    let kernel = match coordinator_of(&store, channel_id).await {
        Some(coordinator) => state.kernel_for(coordinator),
        None => return,
    };
    let config = state.hub.base_config();
    let model = titles::summary_model(
        &state.models.gate_candidates(),
        state.models.classifier_model(),
    );
    let commands = state.commands.clone();
    let channel_id = channel_id.to_string();
    tokio::spawn(async move {
        let gateway = kernel.map(aura_agent::KernelModelGateway::new);
        let summarizer = gateway.as_ref().map(|gateway| Summarizer {
            provider: gateway,
            config: &config,
            model: &model,
        });
        summarize_texts(&store, &channel_id, texts, summarizer, &commands).await;
    });
}

/// The coordinator of channel `channel_id`; `None` when it can't be read
/// (logged) or there is no such channel.
async fn coordinator_of(
    store: &ChannelStore,
    channel_id: &str,
) -> Option<aura_core_types::AgentId> {
    let id = channel_id.to_string();
    match blocking(store, move |store| store.get(&id)).await {
        Ok(channel) => channel.map(|channel| channel.coordinator),
        Err(error) => {
            warn!(%channel_id, error = %format!("{error:#}"), "Could not read the channel to summarize its messages");
            None
        }
    }
}

/// Send `commands` a summary of each of `texts` in channel `channel_id`:
/// the cached ones at once, then each the `summarizer` writes as it comes,
/// stored first. Without a `summarizer` (a coordinator whose kernel is on
/// its cloud machine) only cached ones are sent.
pub(super) async fn summarize_texts(
    store: &ChannelStore,
    channel_id: &str,
    texts: Vec<String>,
    summarizer: Option<Summarizer<'_>>,
    commands: &mpsc::Sender<UiCommand>,
) {
    let (cached, missing) = split_cached(store, channel_id, texts).await;
    if !cached.is_empty() && !send(commands, cached).await {
        return;
    }
    let Some(summarizer) = summarizer else {
        debug!(%channel_id, missing = missing.len(), "No kernel to summarize messages with");
        return;
    };
    for text in missing {
        let Some(summary) = titles::summary_for(
            summarizer.provider,
            summarizer.config,
            summarizer.model,
            &text,
        )
        .await
        else {
            continue;
        };
        save(store, channel_id, &text, &summary).await;
        if !send(commands, vec![MessageSummary { text, summary }]).await {
            return;
        }
    }
}

/// The cache key of the message `text`: its blake3 hash in hex.
fn summary_key(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// `texts` split into the summaries the store has and the texts it has
/// none for. A text whose lookup fails (logged) counts as missing.
async fn split_cached(
    store: &ChannelStore,
    channel_id: &str,
    texts: Vec<String>,
) -> (Vec<MessageSummary>, Vec<String>) {
    let id = channel_id.to_string();
    let looked_up = blocking(store, move |store| {
        Ok(texts
            .into_iter()
            .map(|text| {
                let found = store.summary(&id, &summary_key(&text));
                (text, found)
            })
            .collect::<Vec<_>>())
    })
    .await;
    let looked_up = match looked_up {
        Ok(looked_up) => looked_up,
        Err(error) => {
            warn!(%channel_id, error = %format!("{error:#}"), "Could not read the message summaries");
            return (Vec::new(), Vec::new());
        }
    };
    let mut cached = Vec::new();
    let mut missing = Vec::new();
    for (text, found) in looked_up {
        match found {
            Ok(Some(summary)) => cached.push(MessageSummary { text, summary }),
            Ok(None) => missing.push(text),
            Err(error) => {
                warn!(%channel_id, %error, "Could not read a message summary");
                missing.push(text);
            }
        }
    }
    (cached, missing)
}

/// Store `summary` for `text`; a failure is only logged, and the message
/// is summarized again next run.
async fn save(store: &ChannelStore, channel_id: &str, text: &str, summary: &str) {
    let (id, key, summary) = (
        channel_id.to_string(),
        summary_key(text),
        summary.to_string(),
    );
    if let Err(error) = blocking(store, move |store| store.set_summary(&id, &key, &summary)).await {
        warn!(%channel_id, error = %format!("{error:#}"), "Could not store a message summary");
    }
}

/// Send `summaries` to the UI; `false` once it is gone.
async fn send(commands: &mpsc::Sender<UiCommand>, summaries: Vec<MessageSummary>) -> bool {
    commands
        .send(UiCommand::MessageSummaries(summaries))
        .await
        .is_ok()
}

#[cfg(test)]
#[path = "summaries_tests.rs"]
mod tests;
