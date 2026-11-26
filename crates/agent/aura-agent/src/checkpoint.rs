//! Session checkpoints.
//!
//! The record rebuilds only an agent's prompts and replies; tool calls
//! can't be replayed, so a restart used to lose the model's working
//! context and its todo list. After each turn the agent's surface (the
//! terminal, or the node on a cloud machine) writes what the model saw to
//! `<data_dir>/sessions/<agent>/<started_ms>.json`, with a `current` file
//! naming the session in use. The file is a cache: the record stays the
//! source of truth, entries newer than the checkpoint are replayed on top
//! of it (see [`crate::history::restore`]), and a checkpoint older than
//! the last session start is ignored.

use aura_core_types::AgentId;
use aura_exec_tools::todo::TodoItem;
use aura_model::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::warn;

/// Format version of checkpoint files.
pub const CHECKPOINT_VERSION: u32 = 1;

/// Name of the file holding the session in use.
const CURRENT: &str = "current";

/// Characters of the first prompt kept as a session's title.
const TITLE_CHARS: usize = 80;

/// Model calls and tokens of a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionUsage {
    pub calls: u32,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// Whether any call had a price
    pub priced: bool,
    /// Calls with no known price, which `cost_usd` leaves out. 0 in
    /// checkpoints written before it was kept.
    #[serde(default)]
    pub unpriced_calls: u32,
    /// Prompt of the latest call: what was in the context window
    #[serde(default)]
    pub context_tokens: u64,
    /// Models the calls went to, first use first
    #[serde(default)]
    pub models: Vec<String>,
}

impl SessionUsage {
    /// Add `other` to these totals.
    pub fn add(&mut self, other: &Self) {
        self.calls = self.calls.saturating_add(other.calls);
        self.input_tokens += other.input_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.output_tokens += other.output_tokens;
        self.cost_usd += other.cost_usd;
        self.priced |= other.priced;
        self.unpriced_calls = self.unpriced_calls.saturating_add(other.unpriced_calls);
        if other.calls > 0 {
            self.context_tokens = other.context_tokens;
        }
        for model in &other.models {
            if !self.models.contains(model) {
                self.models.push(model.clone());
            }
        }
    }
}

/// What an agent's model saw at the end of its latest turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCheckpoint {
    pub version: u32,
    /// When the session started; names its file
    pub started_ms: u64,
    pub updated_ms: u64,
    /// Head of the agent's record when this was written
    pub record_seq: u64,
    pub messages: Vec<Message>,
    pub todos: Vec<TodoItem>,
    pub usage: SessionUsage,
}

impl SessionCheckpoint {
    /// A checkpoint of the given session state.
    #[must_use]
    pub fn new(
        started_ms: u64,
        record_seq: u64,
        messages: Vec<Message>,
        todos: Vec<TodoItem>,
        usage: SessionUsage,
    ) -> Self {
        Self {
            version: CHECKPOINT_VERSION,
            started_ms,
            updated_ms: now_ms(),
            record_seq,
            messages,
            todos,
            usage,
        }
    }

    /// The first thing the user asked, cut short.
    #[must_use]
    pub fn title(&self) -> String {
        let first = self
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .map(Message::text_content)
            .unwrap_or_default();
        let line = first.lines().next().unwrap_or("").trim();
        let mut title: String = line.chars().take(TITLE_CHARS).collect();
        if line.chars().count() > TITLE_CHARS {
            title.push('…');
        }
        title
    }
}

/// One saved session, for the `/resume` list.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionListing {
    pub started_ms: u64,
    pub updated_ms: u64,
    pub messages: usize,
    pub title: String,
}

/// Where an agent surface keeps session checkpoints.
#[derive(Debug, Clone)]
pub struct Checkpoints {
    root: PathBuf,
}

impl Checkpoints {
    /// Checkpoints under `<data_dir>/sessions`.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("sessions"),
        }
    }

    fn agent_dir(&self, agent_id: AgentId) -> PathBuf {
        self.root.join(agent_id.to_hex())
    }

    fn file(&self, agent_id: AgentId, started_ms: u64) -> PathBuf {
        self.agent_dir(agent_id).join(format!("{started_ms}.json"))
    }

    /// Write `checkpoint` and make it the agent's current session. The file
    /// is replaced whole, so a crash leaves the previous checkpoint.
    ///
    /// # Errors
    /// Returns the I/O or serialization error.
    pub async fn save(
        &self,
        agent_id: AgentId,
        checkpoint: &SessionCheckpoint,
    ) -> std::io::Result<()> {
        let mut stored = checkpoint.clone();
        aura_context_compaction::compact_for_storage(&mut stored.messages);
        let bytes = serde_json::to_vec(&stored)?;
        let dir = self.agent_dir(agent_id);
        tokio::fs::create_dir_all(&dir).await?;
        let path = self.file(agent_id, checkpoint.started_ms);
        replace(&path, &bytes).await?;
        replace(
            &dir.join(CURRENT),
            checkpoint.started_ms.to_string().as_bytes(),
        )
        .await
    }

    /// The checkpoint of the session started at `started_ms`.
    #[must_use]
    pub fn load(&self, agent_id: AgentId, started_ms: u64) -> Option<SessionCheckpoint> {
        let path = self.file(agent_id, started_ms);
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<SessionCheckpoint>(&bytes) {
            Ok(checkpoint) if checkpoint.version == CHECKPOINT_VERSION => Some(checkpoint),
            Ok(checkpoint) => {
                warn!(path = %path.display(), version = checkpoint.version, "Unknown checkpoint version");
                None
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Unreadable session checkpoint");
                None
            }
        }
    }

    /// The checkpoint of the agent's current session.
    #[must_use]
    pub fn load_current(&self, agent_id: AgentId) -> Option<SessionCheckpoint> {
        let current = std::fs::read_to_string(self.agent_dir(agent_id).join(CURRENT)).ok()?;
        self.load(agent_id, current.trim().parse().ok()?)
    }

    /// The agent's saved sessions other than `except`, newest first.
    #[must_use]
    pub fn list(&self, agent_id: AgentId, except: Option<u64>) -> Vec<SessionListing> {
        let Ok(entries) = std::fs::read_dir(self.agent_dir(agent_id)) else {
            return Vec::new();
        };
        let mut sessions: Vec<SessionListing> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let started_ms = name.to_str()?.strip_suffix(".json")?.parse::<u64>().ok()?;
                (Some(started_ms) != except).then_some(started_ms)
            })
            .filter_map(|started_ms| self.load(agent_id, started_ms))
            .filter(|checkpoint| !checkpoint.messages.is_empty())
            .map(|checkpoint| SessionListing {
                started_ms: checkpoint.started_ms,
                updated_ms: checkpoint.updated_ms,
                messages: checkpoint.messages.len(),
                title: checkpoint.title(),
            })
            .collect();
        sessions.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        sessions
    }
}

/// Write `bytes` to `path` through a temporary file, so readers never see
/// half a file.
async fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

/// Milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Whether `messages` end in a state the model can continue from: no tool
/// call without its result.
#[must_use]
pub fn is_resumable(messages: &[Message]) -> bool {
    let Some(last) = messages.last() else {
        return true;
    };
    !last
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_exec_tools::todo::TodoStatus;

    fn checkpoint(started_ms: u64, prompt: &str) -> SessionCheckpoint {
        SessionCheckpoint::new(
            started_ms,
            7,
            vec![Message::user(prompt), Message::assistant("done")],
            vec![TodoItem {
                id: "1".to_string(),
                content: "ship it".to_string(),
                status: TodoStatus::InProgress,
                assignee: None,
            }],
            SessionUsage {
                calls: 3,
                ..SessionUsage::default()
            },
        )
    }

    #[tokio::test]
    async fn save_then_load_current() {
        let dir = tempfile::tempdir().unwrap();
        let checkpoints = Checkpoints::new(dir.path());
        let agent = AgentId::generate();

        assert!(checkpoints.load_current(agent).is_none());
        checkpoints
            .save(agent, &checkpoint(100, "first"))
            .await
            .unwrap();
        checkpoints
            .save(agent, &checkpoint(200, "second"))
            .await
            .unwrap();

        let current = checkpoints.load_current(agent).unwrap();
        assert_eq!(current.started_ms, 200);
        assert_eq!(current.record_seq, 7);
        assert_eq!(current.todos.len(), 1);
        assert_eq!(current.usage.calls, 3);

        let others = checkpoints.list(agent, Some(200));
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].title, "first");
        assert_eq!(others[0].messages, 2);
    }

    #[tokio::test]
    async fn unreadable_checkpoint_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let checkpoints = Checkpoints::new(dir.path());
        let agent = AgentId::generate();
        let agent_dir = dir.path().join("sessions").join(agent.to_hex());
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("5.json"), b"{not json").unwrap();
        std::fs::write(agent_dir.join(CURRENT), b"5").unwrap();

        assert!(checkpoints.load_current(agent).is_none());
        assert!(checkpoints.list(agent, None).is_empty());
    }

    #[test]
    fn usage_counts_unpriced_calls_and_loads_without_them() {
        let mut usage = SessionUsage::default();
        usage.add(&SessionUsage {
            calls: 1,
            cost_usd: 0.1,
            priced: true,
            ..SessionUsage::default()
        });
        usage.add(&SessionUsage {
            calls: 1,
            unpriced_calls: 1,
            ..SessionUsage::default()
        });
        assert!(usage.priced);
        assert_eq!(usage.unpriced_calls, 1, "a part priced total says so");

        let old = r#"{"calls":2,"input_tokens":1,"cache_read_tokens":0,
            "cache_write_tokens":0,"output_tokens":1,"cost_usd":0.1,"priced":true}"#;
        let loaded: SessionUsage = serde_json::from_str(old).unwrap();
        assert_eq!(loaded.unpriced_calls, 0);
        assert!(loaded.priced);
    }

    #[test]
    fn title_is_the_first_prompt_line() {
        let long = format!("{}\nsecond line", "a".repeat(100));
        let title = checkpoint(1, &long).title();
        assert_eq!(title.chars().count(), TITLE_CHARS + 1);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn a_dangling_tool_call_is_not_resumable() {
        let call = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({}),
            }],
        );
        assert!(!is_resumable(&[Message::user("hi"), call]));
        assert!(is_resumable(&[Message::user("hi")]));
    }
}
