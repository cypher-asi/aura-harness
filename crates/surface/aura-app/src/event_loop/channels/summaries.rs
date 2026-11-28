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
//! through the coordinator's kernel, like a chat title, and kept in the
//! channel store (`h/{key}` rows, sealed like the posts) so it is never
//! asked for again, across restarts too. Earlier versions kept them in a
//! plaintext [`SUMMARY_FILE`] in the app's data directory; it is moved
//! into the store on first use and deleted.
//!
//! ## Invariants
//!
//! - A text is asked for at most once at a time (the in-flight set), and
//!   never again once its summary is kept. Summaries are keyed by the
//!   blake3 hash of the text, so the same message in any channel shares
//!   one.
//! - Nothing here blocks the event loop: the store and the old file are
//!   read and written on a blocking thread, and each model call runs on a
//!   task of its own with the title call's timeout.
//! - A failed or timed-out call sends nothing: the UI keeps showing the
//!   message cut at a word.
//! - A summary is never written in plaintext: without a channel store
//!   they are kept in memory only, and the old file is left as it was.
//! - The old file is moved in one batch and deleted only once the batch
//!   is committed; a failed move leaves it for the next run. An
//!   unreadable one is deleted, as it holds nothing to keep.

use crate::event_loop::titles;
use aura_agent::AgentLoopConfig;
use aura_core_types::AgentId;
use aura_model::ModelProvider;
use aura_store_db::ChannelStore;
use aura_ui::events::{is_typed_prompt, prompt_key, wants_summary, MessageData, MessageRole};
use aura_ui::UiCommand;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::{mpsc, OnceCell};
use tracing::{info, warn};

/// The plaintext file earlier versions kept the summaries in, in the
/// app's data directory; moved into the channel store and deleted.
pub(crate) const SUMMARY_FILE: &str = "channel-post-summaries.json";
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
    /// Where they are kept, sealed like the posts; `None` keeps them in
    /// memory only
    store: Option<ChannelStore>,
    /// The plaintext [`SUMMARY_FILE`] an earlier version kept them in,
    /// moved into `store` and deleted on first use
    legacy: Option<PathBuf>,
    /// Summaries read or written this run, by key
    kept: Mutex<HashMap<String, String>>,
    /// Set once the legacy file was dealt with
    migrated: OnceCell<()>,
    /// Keys of texts being summarized now
    in_flight: Mutex<HashSet<String>>,
}

/// One summary of the legacy file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Entry {
    key: String,
    summary: String,
}

/// The legacy file's contents.
#[derive(Debug, Default, Deserialize)]
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
    /// Summaries kept in `store`, sealed whenever its posts are; `None`
    /// keeps them in memory only. A [`SUMMARY_FILE`] left in
    /// `legacy_dir` by an earlier version is moved into `store` on first
    /// use, then deleted.
    pub(crate) fn new(store: Option<ChannelStore>, legacy_dir: Option<&Path>) -> Self {
        Self {
            store,
            legacy: legacy_dir.map(|dir| dir.join(SUMMARY_FILE)),
            kept: Mutex::new(HashMap::new()),
            migrated: OnceCell::new(),
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    /// The kept summary of the text keyed `key`.
    pub(crate) async fn get(&self, key: &str) -> Option<String> {
        self.migrate().await;
        if let Some(summary) = lock(&self.kept).get(key) {
            return Some(summary.clone());
        }
        let store = self.store.clone()?;
        let wanted = key.to_string();
        let found = tokio::task::spawn_blocking(move || store.post_summary(&wanted)).await;
        let summary = match found {
            Ok(Ok(summary)) => summary?,
            Ok(Err(error)) => {
                warn!(%error, "Could not read a channel post summary");
                return None;
            }
            Err(error) => {
                warn!(%error, "Reading a channel post summary stopped");
                return None;
            }
        };
        lock(&self.kept).insert(key.to_string(), summary.clone());
        Some(summary)
    }

    /// Keep `summary` of the text keyed `key`.
    pub(crate) async fn put(&self, key: String, summary: String) {
        self.migrate().await;
        lock(&self.kept).insert(key.clone(), summary.clone());
        let Some(store) = self.store.clone() else {
            return;
        };
        let saved =
            tokio::task::spawn_blocking(move || store.set_post_summaries(&[(key, summary)])).await;
        match saved {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "Could not keep a channel post summary"),
            Err(error) => warn!(%error, "Keeping a channel post summary stopped"),
        }
    }

    /// Mark the text keyed `key` as being summarized; `false` when it
    /// already is.
    fn start(&self, key: &str) -> bool {
        lock(&self.in_flight).insert(key.to_string())
    }

    fn done(&self, key: &str) {
        lock(&self.in_flight).remove(key);
    }

    /// Move the legacy file into the store, once.
    async fn migrate(&self) {
        self.migrated
            .get_or_init(|| async {
                let (Some(store), Some(path)) = (self.store.clone(), self.legacy.clone()) else {
                    return;
                };
                let moved = tokio::task::spawn_blocking(move || migrate_file(&store, &path)).await;
                if let Err(error) = moved {
                    warn!(%error, "Moving the channel post summaries stopped");
                }
            })
            .await;
    }
}

/// Lock `mutex`, recovering from poisoning: the sets and maps it guards
/// are caches, whole after every insert or remove.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Move the summaries of the plaintext file at `path` into `store` in one
/// batch, then delete the file and any half-written copy of it. A file
/// that can't be read as summaries is deleted: it holds nothing to keep.
/// When the batch fails, the file stays for the next run to try again.
fn migrate_file(store: &ChannelStore, path: &Path) {
    remove(&path.with_extension("json.tmp"));
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            warn!(path = %path.display(), %error, "Could not read the old channel post summaries");
            return;
        }
    };
    let entries = match serde_json::from_str::<SummaryFile>(&text) {
        Ok(file) => file.entries,
        Err(error) => {
            warn!(path = %path.display(), %error, "The old channel post summaries were unreadable; deleting them");
            remove(path);
            return;
        }
    };
    // Keys this version would never look up can't be kept.
    let pairs: Vec<(String, String)> = entries
        .into_iter()
        .filter(|entry| is_key(&entry.key))
        .map(|entry| (entry.key, entry.summary))
        .collect();
    match store.set_post_summaries(&pairs) {
        Ok(()) => {
            info!(
                moved = pairs.len(),
                "Moved the channel post summaries into the channel store"
            );
            remove(path);
        }
        Err(error) => {
            warn!(%error, "Could not move the old channel post summaries; trying again next run");
        }
    }
}

/// Whether `key` is one [`key_of`] makes: 64 lowercase hex digits.
fn is_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Delete the file at `path`, if there is one; a failure is logged.
fn remove(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(path = %path.display(), %error, "Could not delete the old channel post summaries");
        }
    }
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
