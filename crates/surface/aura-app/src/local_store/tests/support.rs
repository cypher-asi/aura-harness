//! Fixtures for the local store tests: a data directory with its key in
//! a file, and a store filled with posts, summaries, agents and prompts.

use crate::local_store::{open_with, KeySource, LocalStoreError};
use aura_core_types::{AgentId, Hash, RecordEntry, Transaction, TransactionType};
use aura_store_db::agents::derive_agent_id;
use aura_store_db::{
    AgentRecord, AgentStore, ChannelStore, NewChannel, NewPost, PostAuthor, ReadStore,
    RegistryDiff, RocksStore, WriteStore,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A string that must never be found on disk once the store is sealed.
pub(super) const SECRET: &str = "zebra-quartz-mango-7731";

/// The summary key the fixture writes.
const SUMMARY_KEY: &str = "ab12";

/// A data directory whose store key lives in a file outside it.
pub(super) struct Fixture {
    dir: TempDir,
    keys: KeySource,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let keys = KeySource::File(dir.path().join("keys").join("store.key"));
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        Self { dir, keys }
    }

    pub(super) fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    pub(super) fn db(&self) -> PathBuf {
        self.data().join("db")
    }

    pub(super) fn key_file(&self) -> PathBuf {
        self.dir.path().join("keys").join("store.key")
    }

    pub(super) fn open(&self) -> Result<RocksStore, LocalStoreError> {
        open_with(&self.data(), &self.db(), &self.keys)
    }

    /// The error opening fails with.
    pub(super) fn open_err(&self) -> LocalStoreError {
        self.open().err().expect("opening should fail")
    }

    pub(super) fn is_marked_sealed(&self) -> bool {
        self.data().join(".aura-sealed").exists()
    }

    /// An unencrypted store holding [`fill`]'s data, as older builds
    /// left it.
    pub(super) fn plain_store(&self) -> Written {
        let store = RocksStore::open(self.db(), false).unwrap();
        fill(&store)
    }
}

/// What [`fill`] wrote, to find it again.
pub(super) struct Written {
    channel_id: String,
    prompter: AgentId,
}

fn agent(zns_id: &str, name: &str, position: u64) -> AgentRecord {
    AgentRecord {
        agent_id: derive_agent_id(zns_id),
        zns_id: zns_id.to_string(),
        name: name.to_string(),
        created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        position,
        definition: None,
        parent: None,
        project_id: None,
        folder: None,
        archived: false,
        removed: false,
        title: None,
        code: None,
        subtitle: None,
        archived_at: None,
    }
}

fn prompt(agent: AgentId) -> Transaction {
    Transaction::new(
        Hash::from_content(b"prompt"),
        agent,
        1_000,
        TransactionType::UserPrompt,
        bytes::Bytes::from(format!("prompt {SECRET}")),
    )
}

/// A channel with a post, a post summary, two agents and a prompt in an
/// agent's record, each carrying [`SECRET`].
pub(super) fn fill(store: &RocksStore) -> Written {
    let channels = ChannelStore::for_store(store);
    let coordinator = agent("zns-coordinator", &format!("coordinator {SECRET}"), 0);
    let new = NewChannel {
        slug: "plans".to_string(),
        title: "Plans".to_string(),
        ..NewChannel::default()
    };
    let channel = channels
        .create(new, &coordinator, chrono::Utc::now())
        .unwrap();
    let post = NewPost {
        author: PostAuthor::User,
        text: format!("post {SECRET}"),
        mentions: Vec::new(),
        reply_to: None,
        at_ms: 1_000,
        planner: None,
    };
    channels.append_post(&channel.id, post).unwrap();
    channels
        .set_post_summaries(&[(SUMMARY_KEY.to_string(), format!("summary {SECRET}"))])
        .unwrap();
    let diff = RegistryDiff {
        agents_put: vec![agent("zns-helper", &format!("helper {SECRET}"), 1)],
        ..RegistryDiff::default()
    };
    AgentStore::for_store(store).apply(&diff).unwrap();

    let prompter = AgentId::generate();
    let entry = RecordEntry::builder(1, prompt(prompter)).build();
    store.append_entry_direct(prompter, 1, &entry).unwrap();
    Written {
        channel_id: channel.id,
        prompter,
    }
}

/// Everything [`fill`] wrote reads back through the typed stores.
pub(super) fn assert_readable(store: &RocksStore, written: &Written) {
    let channels = ChannelStore::for_store(store);
    let posts = channels.posts(&written.channel_id, None).unwrap();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].text, format!("post {SECRET}"));
    assert_eq!(
        channels.post_summary(SUMMARY_KEY).unwrap(),
        Some(format!("summary {SECRET}"))
    );
    let mut names: Vec<String> = AgentStore::for_store(store)
        .load()
        .unwrap()
        .agents
        .into_iter()
        .map(|a| a.name)
        .collect();
    names.sort();
    assert_eq!(
        names,
        [format!("coordinator {SECRET}"), format!("helper {SECRET}")]
    );
    let entry = store.get_record_entry(written.prompter, 1).unwrap();
    assert_eq!(
        entry.tx.payload.as_ref(),
        format!("prompt {SECRET}").as_bytes()
    );
}

/// Files under `dir` whose bytes contain `needle`.
pub(super) fn files_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            std::fs::read(e.path())
                .unwrap()
                .windows(needle.len())
                .any(|w| w == needle.as_bytes())
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// Every file under `dir` and its bytes.
pub(super) fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| (e.path().to_path_buf(), std::fs::read(e.path()).unwrap()))
        .collect()
}
