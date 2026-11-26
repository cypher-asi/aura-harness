//! Fixtures shared by the channel store tests.

use crate::agents::{derive_agent_id, AgentRecord, AgentStore};
use crate::cf;
use crate::channels::keys::LINK_PREFIX;
use crate::channels::{
    ChannelRecord, ChannelStore, ChannelUpdate, MemberRecord, MemberRole, NewChannel, NewPost,
    PostAuthor,
};
use crate::seal::SealCipher;
use crate::RocksStore;
use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use rocksdb::IteratorMode;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

const SEAL_KEY: [u8; 32] = [5u8; 32];

pub(super) fn open(sealed: bool, path: &Path) -> RocksStore {
    if sealed {
        let cipher = Arc::new(SealCipher::new(&SEAL_KEY));
        RocksStore::open_sealed(path, false, Some(cipher)).unwrap()
    } else {
        RocksStore::open(path, false).unwrap()
    }
}

/// A plaintext and a sealed store, each in its own temp dir, labelled for
/// assertion messages.
pub(super) fn stores() -> Vec<(&'static str, RocksStore, TempDir)> {
    let plain_dir = TempDir::new().unwrap();
    let plain = open(false, plain_dir.path());
    let sealed_dir = TempDir::new().unwrap();
    let sealed = open(true, sealed_dir.path());
    vec![
        ("plaintext", plain, plain_dir),
        ("sealed", sealed, sealed_dir),
    ]
}

pub(super) fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
}

pub(super) fn agent(zns_id: &str, position: u64) -> AgentRecord {
    AgentRecord {
        agent_id: derive_agent_id(zns_id),
        zns_id: zns_id.to_string(),
        name: format!("agent {zns_id}"),
        created_at: ts(i64::try_from(position).unwrap()),
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

pub(super) fn new_channel(slug: &str) -> NewChannel {
    NewChannel {
        slug: slug.to_string(),
        title: format!("Channel {slug}"),
        description: format!("about {slug}"),
        project_id: None,
    }
}

pub(super) fn new_channel_in(slug: &str, project_id: &str) -> NewChannel {
    NewChannel {
        project_id: Some(project_id.to_string()),
        ..new_channel(slug)
    }
}

/// An update that only sets (`Some`) or clears (`None`) the project.
pub(super) fn set_project(project_id: Option<&str>) -> ChannelUpdate {
    ChannelUpdate {
        project_id: Some(project_id.map(str::to_string)),
        ..ChannelUpdate::default()
    }
}

pub(super) fn raw_put(store: &RocksStore, family: &str, key: &[u8], value: &[u8]) {
    let db = store.db_handle();
    db.put_cf(&db.cf_handle(family).unwrap(), key, value)
        .unwrap();
}

pub(super) fn raw_delete(store: &RocksStore, family: &str, key: &[u8]) {
    let db = store.db_handle();
    db.delete_cf(&db.cf_handle(family).unwrap(), key).unwrap();
}

/// Keys of every `p/` row left in the channels family.
pub(super) fn link_keys(store: &RocksStore) -> Vec<Vec<u8>> {
    raw_values(store, cf::CHANNELS)
        .into_iter()
        .map(|(key, _)| key)
        .filter(|key| key.starts_with(LINK_PREFIX.as_bytes()))
        .collect()
}

pub(super) fn member(
    agent_id: AgentId,
    role: MemberRole,
    parent: Option<AgentId>,
    at: i64,
) -> MemberRecord {
    MemberRecord {
        agent_id,
        role,
        parent,
        joined_at: ts(at),
    }
}

pub(super) fn post(text: &str, at_ms: u64) -> NewPost {
    NewPost {
        author: PostAuthor::User,
        text: text.to_string(),
        mentions: Vec::new(),
        reply_to: None,
        at_ms,
    }
}

pub(super) fn agent_ids(store: &RocksStore) -> Vec<AgentId> {
    AgentStore::for_store(store)
        .load()
        .unwrap()
        .agents
        .into_iter()
        .map(|a| a.agent_id)
        .collect()
}

pub(super) fn raw_values(store: &RocksStore, family: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    let db = store.db_handle();
    let cf = db.cf_handle(family).unwrap();
    db.iterator_cf(&cf, IteratorMode::Start)
        .map(|item| {
            let (k, v) = item.unwrap();
            (k.to_vec(), v.to_vec())
        })
        .collect()
}

pub(super) fn is_empty_cf(store: &RocksStore, family: &str) -> bool {
    raw_values(store, family).is_empty()
}

/// Asserts that neither channel family holds a row.
pub(super) fn assert_no_channel_rows(store: &RocksStore, mode: &str) {
    assert!(is_empty_cf(store, cf::CHANNELS), "{mode}");
    assert!(is_empty_cf(store, cf::CHANNEL_MEMBERS), "{mode}");
}

/// Creates a channel with slug `slug` coordinated by a fresh agent
/// `zns_id` at `position`, at `ts(1)`.
pub(super) fn create_channel(
    channels: &ChannelStore,
    slug: &str,
    zns_id: &str,
    position: u64,
) -> ChannelRecord {
    channels
        .create(new_channel(slug), &agent(zns_id, position), ts(1))
        .unwrap()
}
