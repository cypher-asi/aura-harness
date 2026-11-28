//! One-line summaries of the long messages the user posts in a channel,
//! for the channel's HISTORY tab.
//!
//! The messages are the user's posts, and the messages the user typed to
//! the coordinator, which stay in its conversation and are no posts
//! ([`prompts_of`]); a post goes by its `seq`, a typed message by its
//! [`prompt_key`].
//!
//! A post longer than [`aura_ui::events::POST_SUMMARY_MIN_CHARS`] gets one summary, written
//! once by a cheap model (Claude Haiku when the user has a route to it)
//! through the coordinator's kernel, like a chat title, and kept in a
//! file so it is never asked for again, across restarts too
//! ([`SUMMARY_FILE`] in the app's data directory).
//!
//! ## Invariants
//!
//! - A text is asked for at most once at a time (the in-flight set), and
//!   never again once its summary is kept. Summaries are keyed by the
//!   blake3 hash of the text, so the same message in any channel shares
//!   one.
//! - Nothing here blocks the event loop: the file is read and written on a
//!   blocking thread, and each model call runs on a task of its own with
//!   the title call's timeout.
//! - A failed or timed-out call sends nothing: the UI keeps showing the
//!   message cut at a word.
//! - The file is replaced atomically and holds at most
//!   [`MAX_SUMMARIES`], the oldest dropped first. An unreadable file
//!   counts as empty and is overwritten by the next summary.

use crate::event_loop::titles;
use aura_agent::AgentLoopConfig;
use aura_core_types::AgentId;
use aura_model::ModelProvider;
use aura_ui::events::{is_typed_prompt, prompt_key, wants_summary, MessageData, MessageRole};
use aura_ui::UiCommand;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc;
use tracing::warn;

/// The file the summaries are kept in, in the app's data directory.
pub(crate) const SUMMARY_FILE: &str = "channel-post-summaries.json";
/// Most summaries kept; the oldest go first.
pub(crate) const MAX_SUMMARIES: usize = 2_000;
/// The model family a summary is asked of when the user can reach it.
const SUMMARY_MODEL_HINT: &str = "haiku";

/// How a summary is asked for: the coordinator's kernel's gateway, its
/// turn config (sign-in) and the model.
#[derive(Clone)]
pub(crate) struct Asker {
    /// Where the call goes
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    pub config: AgentLoopConfig,
    pub model: String,
}

/// How `coordinator`'s summaries are asked for: through its kernel, with
/// the turn config's sign-in, of Claude Haiku when the user can reach it,
/// else the cheap model chat titles use. `None` without an open kernel
/// (a coordinator on a cloud machine): then only kept summaries show.
pub(crate) fn asker(
    state: &crate::event_loop::LoopState<'_>,
    coordinator: AgentId,
) -> Option<Asker> {
    let kernel = state.kernel_for(coordinator)?;
    let candidates: Vec<String> = state
        .models
        .gate_candidates()
        .into_iter()
        .map(|candidate| candidate.id)
        .collect();
    let fallback = titles::title_model(state.models.classifier_model());
    Some(Asker {
        provider: Arc::new(aura_agent::KernelModelGateway::new(kernel)),
        config: state.agent_loop.config().clone(),
        model: summary_model(&candidates, fallback),
    })
}

/// Summarize the user's posts among `posts` of channel `channel_id` in the
/// background ([`summarize_posts`]), asking through `coordinator`.
pub(crate) fn spawn_for(
    state: &crate::event_loop::LoopState<'_>,
    coordinator: AgentId,
    channel_id: &str,
    posts: Vec<(u64, String)>,
) {
    if posts.iter().all(|(_, text)| !wants_summary(text)) {
        return;
    }
    tokio::spawn(summarize_posts(
        Arc::clone(&state.hub.post_summaries),
        state.commands.clone(),
        channel_id.to_string(),
        posts,
        asker(state, coordinator),
    ));
}

/// The messages the user typed in a coordinator's conversation `history`,
/// as [`spawn_for`] takes them: each under its [`prompt_key`], with its
/// text. Shell commands, agent steps and mail other agents sent are not
/// the user's.
pub(crate) fn prompts_of(history: &[MessageData]) -> Vec<(u64, String)> {
    history
        .iter()
        .filter(|m| m.role == MessageRole::User && m.command.is_none() && m.activity.is_none())
        .filter(|m| is_typed_prompt(&m.content))
        .map(|m| (prompt_key(&m.content), m.content.clone()))
        .collect()
}

/// The kept summaries and the texts being summarized.
#[derive(Debug)]
pub(crate) struct PostSummaries {
    /// The file they are kept in; `None` keeps them in memory only
    path: Option<PathBuf>,
    /// Kept summaries, oldest first; `None` until the file was read
    kept: Mutex<Option<Vec<Entry>>>,
    /// Keys of texts being summarized now
    in_flight: Mutex<HashSet<String>>,
}

/// One kept summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    key: String,
    summary: String,
}

/// The file's contents.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SummaryFile {
    #[serde(default)]
    entries: Vec<Entry>,
}

/// The key a summary of `text` is kept under.
pub(crate) fn key_of(text: &str) -> String {
    blake3::hash(text.trim().as_bytes()).to_hex().to_string()
}

/// The model a summary is asked of: the first Claude Haiku among
/// `candidates` (the user's routes, cheapest first), else `fallback`.
pub(crate) fn summary_model(candidates: &[String], fallback: String) -> String {
    candidates
        .iter()
        .find(|id| id.to_ascii_lowercase().contains(SUMMARY_MODEL_HINT))
        .cloned()
        .unwrap_or(fallback)
}

impl PostSummaries {
    /// Summaries kept in `dir`'s [`SUMMARY_FILE`]; `None` keeps them in
    /// memory only.
    pub(crate) fn new(dir: Option<&Path>) -> Self {
        Self {
            path: dir.map(|dir| dir.join(SUMMARY_FILE)),
            kept: Mutex::new(None),
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    /// The kept summary of the text keyed `key`, reading the file first
    /// when it wasn't yet.
    pub(crate) async fn get(&self, key: &str) -> Option<String> {
        self.load().await;
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.as_ref()?
            .iter()
            .rev()
            .find(|entry| entry.key == key)
            .map(|entry| entry.summary.clone())
    }

    /// Keep `summary` of the text keyed `key`, and save the file.
    pub(crate) async fn put(&self, key: String, summary: String) {
        self.load().await;
        let entries = {
            let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            let entries = kept.get_or_insert_with(Vec::new);
            entries.retain(|entry| entry.key != key);
            entries.push(Entry { key, summary });
            let over = entries.len().saturating_sub(MAX_SUMMARIES);
            entries.drain(..over);
            entries.clone()
        };
        let Some(path) = self.path.clone() else {
            return;
        };
        let saved = tokio::task::spawn_blocking(move || save(&path, entries)).await;
        match saved {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "Could not save the channel post summaries"),
            Err(error) => warn!(%error, "Saving the channel post summaries stopped"),
        }
    }

    /// Mark the text keyed `key` as being summarized; `false` when it
    /// already is.
    fn start(&self, key: &str) -> bool {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_string())
    }

    fn done(&self, key: &str) {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }

    /// Read the file once.
    async fn load(&self) {
        let loaded = self
            .kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if loaded {
            return;
        }
        let entries = match self.path.clone() {
            Some(path) => tokio::task::spawn_blocking(move || read(&path))
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.is_none() {
            *kept = Some(entries);
        }
    }
}

/// The summaries in `path`; none when it is missing or unreadable.
fn read(path: &Path) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<SummaryFile>(&text) {
        Ok(file) => file.entries,
        Err(error) => {
            warn!(path = %path.display(), %error, "The channel post summaries were unreadable; starting anew");
            Vec::new()
        }
    }
}

/// Replace `path` with `entries`, atomically.
fn save(path: &Path, entries: Vec<Entry>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec(&SummaryFile { entries }).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// Send channel `channel_id`'s HISTORY tab the summaries of the user's
/// `posts` (`seq` and text) over the threshold ([`wants_summary`]): those kept
/// at once, the others as `asker` writes them, each once. Without an
/// asker, only the kept ones.
pub(crate) async fn summarize_posts(
    summaries: Arc<PostSummaries>,
    commands: mpsc::Sender<UiCommand>,
    channel_id: String,
    posts: Vec<(u64, String)>,
    asker: Option<Asker>,
) {
    let mut known = Vec::new();
    let mut missing = Vec::new();
    for (seq, text) in posts.into_iter().filter(|(_, text)| wants_summary(text)) {
        let key = key_of(&text);
        match summaries.get(&key).await {
            Some(summary) => known.push((seq, summary)),
            None => missing.push((seq, key, text)),
        }
    }
    if !known.is_empty() {
        send(&commands, &channel_id, known).await;
    }
    let Some(asker) = asker else {
        return;
    };
    for (seq, key, text) in missing {
        if !summaries.start(&key) {
            continue;
        }
        let task = Summarize {
            summaries: Arc::clone(&summaries),
            commands: commands.clone(),
            channel_id: channel_id.clone(),
            asker: asker.clone(),
        };
        tokio::spawn(task.run(seq, key, text));
    }
}

/// One summary being written.
struct Summarize {
    summaries: Arc<PostSummaries>,
    commands: mpsc::Sender<UiCommand>,
    channel_id: String,
    asker: Asker,
}

impl Summarize {
    async fn run(self, seq: u64, key: String, text: String) {
        let Asker {
            provider,
            config,
            model,
        } = &self.asker;
        let summary = titles::summarize_post(provider.as_ref(), config, model, &text).await;
        if let Some(summary) = summary {
            self.summaries.put(key.clone(), summary.clone()).await;
            send(&self.commands, &self.channel_id, vec![(seq, summary)]).await;
        }
        self.summaries.done(&key);
    }
}

/// Send `summaries` of channel `channel_id` to the UI, for the whole app:
/// a channel's HISTORY tab is not one agent's.
async fn send(commands: &mpsc::Sender<UiCommand>, channel_id: &str, summaries: Vec<(u64, String)>) {
    let command = UiCommand::SetPostSummaries {
        channel_id: channel_id.to_string(),
        summaries,
    };
    // A closed channel means the UI is gone; nobody is left to show it.
    let _ = commands.send(command).await;
}

#[cfg(test)]
mod tests;
