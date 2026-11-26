//! Channels: named rooms where a coordinator agent, the agents it
//! brings in and the user talk, stored in the agent-state database.
//!
//! # Key formats
//!
//! * [`crate::cf::CHANNELS`]:
//!   * `c/{channel_id}`: a JSON [`ChannelRecord`]. Its optional
//!     `project_id` is the one project the channel's agents work in.
//!   * `s/{slug}`: the JSON string of the id of the channel owning the
//!     slug (unique slug index).
//!   * `p/{channel_id}/{project_id}`: legacy, a JSON [`ProjectLink`] from
//!     when a channel could link many projects. Only
//!     [`ChannelStore::migrate_links`] (and the legacy
//!     [`ChannelStore::link_project`], [`ChannelStore::unlink_project`]
//!     and [`ChannelStore::projects`], kept until their callers move)
//!     touch these rows; the migration folds them into the `c/` record
//!     and deletes them.
//!   * `l/{channel_id}`: when the channel's coordinator last read its
//!     activity, JSON Unix milliseconds.
//! * [`crate::cf::CHANNEL_MEMBERS`]:
//!   * `m/{channel_id}/` followed by the 32 raw bytes of the agent id: a
//!     JSON [`MemberRecord`].
//!   * `a/` followed by the 32 raw bytes of the agent id: the JSON string
//!     of the id of the channel the agent is in (reverse index).
//! * [`crate::cf::CHANNEL_POSTS`], keyed
//!   `{channel_id}/{at_ms:020}/{seq:010}`: a JSON [`ChannelPost`]. Keys
//!   sort by time within a channel, ties by sequence number.
//!
//! A channel id is a UUID v4 string; ids containing `/` are rejected so
//! they can't bleed into a neighbouring key prefix. Every value is
//! sealed in sealed mode; keys are plaintext in both modes.
//!
//! # Invariants
//!
//! * The `s/` index and the `c/` rows are in step: every slug names
//!   exactly one channel, written in the same batch as its `c/` row.
//!   Archived channels keep their slug.
//! * The `m/` and `a/` rows are in step: a member row and its reverse
//!   index row are written and deleted in the same batch.
//! * An agent is in at most one channel. Check-then-write operations
//!   (slug uniqueness, membership) run under one write lock shared by
//!   every clone of a [`ChannelStore`]; callers that stage membership
//!   themselves ([`ChannelStore::stage_member`]) must check
//!   [`ChannelStore::channel_of`] under their own serialization.
//! * A channel and its coordinator (agent row, member row, reverse index)
//!   are committed together in one batch by [`ChannelStore::create`],
//!   the channel's project included.
//! * A channel has at most one project, held on its `c/` record. A
//!   stored project id is never empty or all whitespace. `c/` rows
//!   written before the field existed decode with no project.
//! * After [`ChannelStore::migrate_links`] a channel has no `p/` rows
//!   (unless its migration failed, or a legacy
//!   [`ChannelStore::link_project`] call adds one); each channel's
//!   migration is one batch, so its project is set and its `p/` rows
//!   deleted together.
//! * Post sequence numbers are unique across the database and grow with
//!   each append: the counter is seeded once from the highest sequence
//!   stored and appends are serialized, so sequence order is commit
//!   order.
//!
//! # Failure modes
//!
//! * Every multi-row change is one `WriteBatch`: all of it lands or none
//!   of it does.
//! * A value that fails to unseal (wrong key, tampering) or to decode
//!   fails the read: sealed values fail closed instead of dropping rows.
//! * A row whose key disagrees with its value (a `c/` row whose id is not
//!   its key, a member row whose agent id is not its key) is skipped with
//!   a `warn!` naming the key, never the payload. So is a slug index
//!   entry pointing at a missing channel.
//! * Sequence numbers above `9_999_999_999` still get unique keys, but
//!   two such posts with the same `at_ms` no longer sort by sequence.
//! * [`ChannelStore::stage_update`] takes no lock: an `update` or
//!   `archive` committed between staging and the caller's write is
//!   overwritten by the staged record. Callers serialize their channel
//!   changes, as with [`ChannelStore::stage_member`].
//! * [`ChannelStore::migrate_links`] logs a channel whose links cannot be
//!   migrated and carries on with the others; that channel keeps its rows
//!   and a rerun tries it again. Link rows with a malformed key are
//!   skipped (logged) and left in place.

use crate::agents::{AgentRecord, AgentStore, AgentStoreError, RegistryDiff};
use crate::cf;
use crate::seal::{SealCipher, SealError};
use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use rocksdb::{
    BoundColumnFamily, DBWithThreadMode, Direction, IteratorMode, MultiThreaded, WriteBatch,
    WriteOptions,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

type Db = DBWithThreadMode<MultiThreaded>;
/// One raw `(key, value)` row read by a prefix scan.
type Row = (Box<[u8]>, Box<[u8]>);

const CHANNEL_PREFIX: &[u8] = b"c/";
const SLUG_PREFIX: &[u8] = b"s/";
const SEEN_PREFIX: &[u8] = b"l/";
const LINK_PREFIX: &str = "p/";
const MEMBER_PREFIX: &str = "m/";
const REVERSE_PREFIX: &[u8] = b"a/";
const MAX_SLUG_LEN: usize = 40;

/// Whether `slug` is 1 to 40 characters of `a-z`, `0-9` and `-`.
#[must_use]
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Errors produced by [`ChannelStore`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ChannelError {
    /// A slug that is not 1 to 40 characters of `a-z`, `0-9` and `-`.
    #[error("invalid channel slug {0:?}: must be 1-40 characters of a-z, 0-9 and -")]
    InvalidSlug(String),
    /// Another channel already has the slug.
    #[error("channel slug {0:?} is taken")]
    SlugTaken(String),
    /// No channel has the id.
    #[error("unknown channel {0}")]
    UnknownChannel(String),
    /// The agent is already a member of a channel.
    #[error("agent {agent} is already a member of channel {channel}")]
    AlreadyMember {
        /// Full hex of the agent id.
        agent: String,
        /// Id of the channel the agent is in.
        channel: String,
    },
    /// A channel id that can't be a key: empty or containing `/`.
    #[error("invalid channel id {0:?}: must be non-empty and contain no '/'")]
    InvalidId(String),
    /// A project id that is empty or all whitespace.
    #[error("invalid project id {0:?}: must not be empty or blank")]
    InvalidProjectId(String),
    /// The database was opened without a column family this store needs.
    #[error("{0} column family not found")]
    MissingColumnFamily(&'static str),
    /// A `RocksDB` read or write failed.
    #[error("{context}: {source}")]
    Backend {
        /// What was being read or written.
        context: String,
        /// The `RocksDB` failure.
        #[source]
        source: rocksdb::Error,
    },
    /// Sealing or unsealing a value failed.
    #[error("{context}: {source}")]
    Seal {
        /// Which value.
        context: String,
        /// The sealing failure.
        #[source]
        source: SealError,
    },
    /// A value failed to encode or decode as JSON.
    #[error("{context}: {source}")]
    Serde {
        /// Which value.
        context: String,
        /// The JSON failure.
        #[source]
        source: serde_json::Error,
    },
    /// Staging the coordinator's agent row failed.
    #[error("staging coordinator agent row: {0}")]
    Agent(#[from] AgentStoreError),
    /// A failure injected by [`ChannelStore::create_with_fault`].
    #[cfg(any(test, feature = "test-support"))]
    #[error("injected fault: {0:?}")]
    InjectedFault(crate::FaultAt),
}

/// One channel, the value of a `c/{channel_id}` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRecord {
    /// UUID v4 string; the row key.
    pub id: String,
    /// Unique short name, see [`valid_slug`].
    pub slug: String,
    /// Display title.
    pub title: String,
    /// What the channel is for.
    pub description: String,
    /// The agent that runs the channel.
    pub coordinator: AgentId,
    /// When the channel was created.
    pub created_at: DateTime<Utc>,
    /// When the title, description, project or archived flag last
    /// changed through [`ChannelStore::update`] or
    /// [`ChannelStore::archive`].
    pub updated_at: DateTime<Utc>,
    /// Moved to the Archive; still holds its slug.
    #[serde(default)]
    pub archived: bool,
    /// The project the channel's agents work in; `None` for none. Absent
    /// in rows written before channels had a project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// What [`ChannelStore::create`] needs to make a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewChannel {
    /// Unique short name, see [`valid_slug`].
    pub slug: String,
    /// Display title.
    pub title: String,
    /// What the channel is for.
    pub description: String,
    /// The project the channel's agents work in, if any; must not be
    /// empty or blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// Fields [`ChannelStore::update`] changes; `None` leaves one as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelUpdate {
    /// New title.
    pub title: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New project: `Some(None)` clears it, `Some(Some(id))` sets it (the
    /// id must not be empty or blank). In JSON an absent field is `None`
    /// and `null` is `Some(None)`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub project_id: Option<Option<String>>,
}

/// Deserialize a field that is present (possibly `null`) as `Some`, so
/// together with `#[serde(default)]` an absent field stays `None` and
/// `null` becomes `Some(None)`.
fn present_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A project linked to a channel, the value of a legacy
/// `p/{channel_id}/{project_id}` row; see [`ChannelStore::migrate_links`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLink {
    /// The linked project.
    pub project_id: String,
    /// When it was linked.
    pub linked_at: DateTime<Utc>,
}

/// What an agent does in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    /// Runs the channel.
    Coordinator,
    /// Brought in as a full member.
    Agent,
    /// Spawned by another member for a subtask.
    Subagent,
}

/// One member of a channel, the value of an `m/` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRecord {
    /// The member; the last 32 bytes of the row key.
    pub agent_id: AgentId,
    /// What it does in the channel.
    pub role: MemberRole,
    /// The member that brought it in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AgentId>,
    /// When it joined.
    pub joined_at: DateTime<Utc>,
}

/// Who wrote a post.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostAuthor {
    /// The human user.
    User,
    /// An agent.
    Agent(AgentId),
}

/// One post in a channel, the value of a [`crate::cf::CHANNEL_POSTS`]
/// row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPost {
    /// The channel posted to.
    pub channel_id: String,
    /// Unique, growing sequence number assigned on append.
    pub seq: u64,
    /// Who wrote it.
    pub author: PostAuthor,
    /// The text.
    pub text: String,
    /// Agents mentioned.
    #[serde(default)]
    pub mentions: Vec<AgentId>,
    /// `seq` of the post replied to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
    /// When it was posted, milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// What [`ChannelStore::append_post`] needs to make a post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPost {
    /// Who wrote it.
    pub author: PostAuthor,
    /// The text.
    pub text: String,
    /// Agents mentioned.
    pub mentions: Vec<AgentId>,
    /// `seq` of the post replied to.
    pub reply_to: Option<u64>,
    /// When it was posted, milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// Channels, their members, projects and posts in the shared
/// agent-state `RocksDB`.
///
/// Every method is synchronous and blocking; async callers run them on
/// `spawn_blocking`.
#[derive(Clone)]
pub struct ChannelStore {
    db: Arc<Db>,
    /// `Some` in sealed mode.
    cipher: Option<Arc<SealCipher>>,
    sync_writes: bool,
    /// Stages the coordinator's agent row into the create batch.
    agents: AgentStore,
    /// Serializes check-then-write operations across clones.
    write_lock: Arc<Mutex<()>>,
    /// The last post sequence number assigned; `None` until seeded from
    /// the stored posts. Held across the post write so sequence order is
    /// commit order.
    last_post_seq: Arc<Mutex<Option<u64>>>,
}

impl std::fmt::Debug for ChannelStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelStore")
            .field("sealed", &self.cipher.is_some())
            .field("sync_writes", &self.sync_writes)
            .finish_non_exhaustive()
    }
}

/// Lock a mutex, recovering from poisoning: the guarded data is either
/// `()` or a counter only advanced after a successful write, so a panic
/// while holding it can't leave it inconsistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn validate_id(id: &str) -> Result<(), ChannelError> {
    if id.is_empty() || id.contains('/') {
        return Err(ChannelError::InvalidId(id.to_string()));
    }
    Ok(())
}

fn validate_project_id(project_id: &str) -> Result<(), ChannelError> {
    if project_id.trim().is_empty() {
        return Err(ChannelError::InvalidProjectId(project_id.to_string()));
    }
    Ok(())
}

/// Check the project id an update would set, if any.
fn validate_update(update: &ChannelUpdate) -> Result<(), ChannelError> {
    match &update.project_id {
        Some(Some(project_id)) => validate_project_id(project_id),
        Some(None) | None => Ok(()),
    }
}

/// Apply the fields `update` sets to `record`.
fn apply_update(record: &mut ChannelRecord, update: ChannelUpdate) {
    if let Some(title) = update.title {
        record.title = title;
    }
    if let Some(description) = update.description {
        record.description = description;
    }
    if let Some(project_id) = update.project_id {
        record.project_id = project_id;
    }
}

fn channel_key(id: &str) -> Vec<u8> {
    [CHANNEL_PREFIX, id.as_bytes()].concat()
}

fn slug_key(slug: &str) -> Vec<u8> {
    [SLUG_PREFIX, slug.as_bytes()].concat()
}

fn seen_key(channel_id: &str) -> Vec<u8> {
    [SEEN_PREFIX, channel_id.as_bytes()].concat()
}

fn link_prefix(channel_id: &str) -> Vec<u8> {
    format!("{LINK_PREFIX}{channel_id}/").into_bytes()
}

fn link_key(channel_id: &str, project_id: &str) -> Vec<u8> {
    [link_prefix(channel_id), project_id.as_bytes().to_vec()].concat()
}

/// The channel id of a `p/{channel_id}/{project_id}` key, if the key has
/// that shape and the id is valid.
fn link_key_channel(key: &[u8]) -> Option<&str> {
    let rest = key.strip_prefix(LINK_PREFIX.as_bytes())?;
    let end = rest.iter().position(|&b| b == b'/')?;
    let channel_id = std::str::from_utf8(rest.get(..end)?).ok()?;
    validate_id(channel_id).ok()?;
    Some(channel_id)
}

fn member_prefix(channel_id: &str) -> Vec<u8> {
    format!("{MEMBER_PREFIX}{channel_id}/").into_bytes()
}

fn member_key(channel_id: &str, agent: &AgentId) -> Vec<u8> {
    [member_prefix(channel_id).as_slice(), agent.as_bytes()].concat()
}

fn reverse_key(agent: &AgentId) -> Vec<u8> {
    [REVERSE_PREFIX, agent.as_bytes()].concat()
}

fn post_prefix(channel_id: &str) -> Vec<u8> {
    format!("{channel_id}/").into_bytes()
}

fn post_key(channel_id: &str, at_ms: u64, seq: u64) -> Vec<u8> {
    format!("{channel_id}/{at_ms:020}/{seq:010}").into_bytes()
}

/// The sequence number at the end of a post key, if it parses.
fn post_key_seq(key: &[u8]) -> Option<u64> {
    let tail = key.rsplit(|&b| b == b'/').next()?;
    std::str::from_utf8(tail).ok()?.parse().ok()
}

impl ChannelStore {
    /// Store sharing `store`'s database, sealing mode and write
    /// durability.
    #[must_use]
    pub fn for_store(store: &crate::RocksStore) -> Self {
        let db = Arc::clone(store.db_handle());
        let cipher = store.cipher().cloned();
        Self {
            agents: AgentStore::with_cipher(Arc::clone(&db), cipher.clone()),
            db,
            cipher,
            sync_writes: store.sync_writes(),
            write_lock: Arc::new(Mutex::new(())),
            last_post_seq: Arc::new(Mutex::new(None)),
        }
    }

    fn cf(&self, name: &'static str) -> Result<Arc<BoundColumnFamily<'_>>, ChannelError> {
        self.db
            .cf_handle(name)
            .ok_or(ChannelError::MissingColumnFamily(name))
    }

    fn write_opts(&self) -> WriteOptions {
        let mut opts = WriteOptions::default();
        opts.set_sync(self.sync_writes);
        opts
    }

    /// JSON-encode `value` and seal it in sealed mode.
    fn encode<T: Serialize>(&self, value: &T, what: &str) -> Result<Vec<u8>, ChannelError> {
        let plain = serde_json::to_vec(value).map_err(|source| ChannelError::Serde {
            context: format!("encoding {what}"),
            source,
        })?;
        match &self.cipher {
            Some(cipher) => cipher.seal(&plain).map_err(|source| ChannelError::Seal {
                context: format!("sealing {what}"),
                source,
            }),
            None => Ok(plain),
        }
    }

    /// Unseal `bytes` in sealed mode and JSON-decode them.
    fn decode<T: DeserializeOwned>(&self, bytes: &[u8], what: &str) -> Result<T, ChannelError> {
        let plain = match &self.cipher {
            Some(cipher) => {
                Cow::Owned(cipher.open(bytes).map_err(|source| ChannelError::Seal {
                    context: format!("opening {what}"),
                    source,
                })?)
            }
            None => Cow::Borrowed(bytes),
        };
        serde_json::from_slice(&plain).map_err(|source| ChannelError::Serde {
            context: format!("decoding {what}"),
            source,
        })
    }

    fn read<T: DeserializeOwned>(
        &self,
        family: &'static str,
        key: &[u8],
        what: &str,
    ) -> Result<Option<T>, ChannelError> {
        let cf = self.cf(family)?;
        let bytes = self
            .db
            .get_cf(&cf, key)
            .map_err(|source| ChannelError::Backend {
                context: format!("reading {what}"),
                source,
            })?;
        bytes.map(|bytes| self.decode(&bytes, what)).transpose()
    }

    /// Every `(key, value)` row of `family` whose key starts with
    /// `prefix`, from `start` (which must itself start with `prefix`).
    fn scan(
        &self,
        family: &'static str,
        prefix: &[u8],
        start: &[u8],
        what: &str,
    ) -> Result<Vec<Row>, ChannelError> {
        let cf = self.cf(family)?;
        let mut rows = Vec::new();
        for item in self
            .db
            .iterator_cf(&cf, IteratorMode::From(start, Direction::Forward))
        {
            let (key, value) = item.map_err(|source| ChannelError::Backend {
                context: format!("reading {what}"),
                source,
            })?;
            if !key.starts_with(prefix) {
                break;
            }
            rows.push((key, value));
        }
        Ok(rows)
    }

    fn commit(&self, batch: WriteBatch, what: &str) -> Result<(), ChannelError> {
        self.db
            .write_opt(batch, &self.write_opts())
            .map_err(|source| ChannelError::Backend {
                context: format!("writing {what}"),
                source,
            })
    }

    fn require_channel(&self, id: &str) -> Result<ChannelRecord, ChannelError> {
        self.get(id)?
            .ok_or_else(|| ChannelError::UnknownChannel(id.to_string()))
    }

    /// Create a channel run by `coordinator`, committing the channel (and
    /// its project, if any), its slug, the coordinator's agent row and
    /// its membership in one batch.
    ///
    /// # Errors
    /// [`ChannelError::InvalidSlug`], [`ChannelError::SlugTaken`],
    /// [`ChannelError::InvalidProjectId`] for an empty or blank project
    /// id, [`ChannelError::AlreadyMember`] when the coordinator is in a
    /// channel, [`ChannelError::Agent`] when its record breaks the agent
    /// key invariants; store, sealing and encoding failures. Nothing is
    /// written on error.
    pub fn create(
        &self,
        new: NewChannel,
        coordinator: &AgentRecord,
        now: DateTime<Utc>,
    ) -> Result<ChannelRecord, ChannelError> {
        let _guard = lock(&self.write_lock);
        let (record, batch, _) = self.prepare_create(new, coordinator, now)?;
        self.commit(batch, &format!("channel {}", record.id))?;
        tracing::info!(
            channel_id = %record.id,
            coordinator = %record.coordinator.to_hex(),
            "channel created"
        );
        Ok(record)
    }

    /// Validate, then build the full create batch and the partial one
    /// (just the agent row) a non-atomic writer would have committed.
    fn prepare_create(
        &self,
        new: NewChannel,
        coordinator: &AgentRecord,
        now: DateTime<Utc>,
    ) -> Result<(ChannelRecord, WriteBatch, WriteBatch), ChannelError> {
        if !valid_slug(&new.slug) {
            return Err(ChannelError::InvalidSlug(new.slug));
        }
        if let Some(project_id) = &new.project_id {
            validate_project_id(project_id)?;
        }
        let slugs = self.cf(cf::CHANNELS)?;
        let taken = self
            .db
            .get_cf(&slugs, slug_key(&new.slug))
            .map_err(|source| ChannelError::Backend {
                context: format!("reading slug {}", new.slug),
                source,
            })?;
        if taken.is_some() {
            return Err(ChannelError::SlugTaken(new.slug));
        }
        let agent = coordinator.agent_id;
        if let Some(channel) = self.channel_of(&agent)? {
            return Err(ChannelError::AlreadyMember {
                agent: agent.to_hex(),
                channel,
            });
        }
        let record = ChannelRecord {
            id: uuid::Uuid::new_v4().to_string(),
            slug: new.slug,
            title: new.title,
            description: new.description,
            coordinator: agent,
            created_at: now,
            updated_at: now,
            archived: false,
            project_id: new.project_id,
        };
        let member = MemberRecord {
            agent_id: agent,
            role: MemberRole::Coordinator,
            parent: None,
            joined_at: now,
        };
        let diff = RegistryDiff {
            agents_put: vec![coordinator.clone()],
            ..RegistryDiff::default()
        };
        let what = format!("channel {}", record.id);
        let channel_value = self.encode(&record, &what)?;
        let slug_value = self.encode(&record.id, &format!("slug {}", record.slug))?;

        let mut partial = WriteBatch::default();
        self.agents.stage(&diff, &mut partial)?;
        let mut full = WriteBatch::default();
        self.agents.stage(&diff, &mut full)?;
        full.put_cf(&slugs, channel_key(&record.id), channel_value);
        full.put_cf(&slugs, slug_key(&record.slug), slug_value);
        self.stage_member(&record.id, &member, &mut full)?;
        Ok((record, full, partial))
    }

    /// The channel with `id`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn get(&self, id: &str) -> Result<Option<ChannelRecord>, ChannelError> {
        validate_id(id)?;
        self.read(cf::CHANNELS, &channel_key(id), &format!("channel {id}"))
    }

    /// The channel with `slug`, if any.
    ///
    /// # Errors
    /// Store, unsealing and decoding failures.
    pub fn by_slug(&self, slug: &str) -> Result<Option<ChannelRecord>, ChannelError> {
        if !valid_slug(slug) {
            return Ok(None);
        }
        let Some(id) =
            self.read::<String>(cf::CHANNELS, &slug_key(slug), &format!("slug {slug}"))?
        else {
            return Ok(None);
        };
        let record = self.get(&id)?;
        if record.is_none() {
            tracing::warn!(slug, channel_id = %id, "slug index points at a missing channel");
        }
        Ok(record)
    }

    /// Every channel, archived ones included, sorted by `created_at`,
    /// ties by slug.
    ///
    /// # Errors
    /// Store, unsealing and decoding failures.
    pub fn list(&self) -> Result<Vec<ChannelRecord>, ChannelError> {
        let rows = self.scan(cf::CHANNELS, CHANNEL_PREFIX, CHANNEL_PREFIX, "channels")?;
        let mut channels = Vec::with_capacity(rows.len());
        for (key, value) in rows {
            let id = String::from_utf8_lossy(key.get(CHANNEL_PREFIX.len()..).unwrap_or_default());
            let record: ChannelRecord = self.decode(&value, &format!("channel {id}"))?;
            if record.id != id {
                tracing::warn!(channel_id = %id, "skipping channel row whose key is not its id");
                continue;
            }
            channels.push(record);
        }
        channels.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.slug.cmp(&b.slug))
        });
        Ok(channels)
    }

    /// Change the title, description and/or project of channel `id`,
    /// stamping `updated_at` with `now`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidProjectId`] for an empty or blank project
    /// id, [`ChannelError::InvalidId`], [`ChannelError::UnknownChannel`];
    /// store, sealing and encoding failures. Nothing is written on error.
    pub fn update(
        &self,
        id: &str,
        update: ChannelUpdate,
        now: DateTime<Utc>,
    ) -> Result<ChannelRecord, ChannelError> {
        validate_update(&update)?;
        self.modify(id, now, |record| apply_update(record, update))
    }

    /// Apply `update` to channel `channel_id` as [`Self::update`] does,
    /// but put the changed `c/` row into `batch` instead of writing it,
    /// for a caller committing it together with rows of another store
    /// (e.g. the coordinator's agent row), then [`Self::write`]. Returns
    /// the record as it will be once `batch` is written.
    ///
    /// Takes no lock, like [`Self::stage_member`]: the record is read now
    /// and written whenever the caller commits, so an [`Self::update`] or
    /// [`Self::archive`] committed in between is overwritten. The caller
    /// must serialize its channel changes (aura-app runs them one at a
    /// time from its event loop). Holding the store's write lock here
    /// would not help, since it can't be held until the caller's write.
    /// On error `batch` is left as it was.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub fn stage_update(
        &self,
        channel_id: &str,
        update: ChannelUpdate,
        now: DateTime<Utc>,
        batch: &mut WriteBatch,
    ) -> Result<ChannelRecord, ChannelError> {
        validate_update(&update)?;
        self.stage_change(
            channel_id,
            now,
            |record| apply_update(record, update),
            batch,
        )
    }

    /// Move channel `id` to the Archive. It keeps its slug and members.
    ///
    /// # Errors
    /// [`ChannelError::UnknownChannel`]; store, sealing and encoding
    /// failures.
    pub fn archive(&self, id: &str, now: DateTime<Utc>) -> Result<ChannelRecord, ChannelError> {
        self.modify(id, now, |record| record.archived = true)
    }

    /// Read channel `id`, `change` it and commit it, under the write lock.
    fn modify(
        &self,
        id: &str,
        now: DateTime<Utc>,
        change: impl FnOnce(&mut ChannelRecord),
    ) -> Result<ChannelRecord, ChannelError> {
        let _guard = lock(&self.write_lock);
        let mut batch = WriteBatch::default();
        let record = self.stage_change(id, now, change, &mut batch)?;
        self.commit(batch, &format!("channel {id}"))?;
        Ok(record)
    }

    /// Read channel `id`, `change` it, stamp `updated_at` and put it into
    /// `batch`. Takes no lock; `batch` is untouched on error.
    fn stage_change(
        &self,
        id: &str,
        now: DateTime<Utc>,
        change: impl FnOnce(&mut ChannelRecord),
        batch: &mut WriteBatch,
    ) -> Result<ChannelRecord, ChannelError> {
        let mut record = self.require_channel(id)?;
        change(&mut record);
        record.updated_at = now;
        let value = self.encode(&record, &format!("channel {id}"))?;
        let channels = self.cf(cf::CHANNELS)?;
        batch.put_cf(&channels, channel_key(id), value);
        Ok(record)
    }

    /// Fold the legacy `p/` link rows into the channels' `project_id`,
    /// returning how many channels changed.
    ///
    /// For each channel with link rows, in one batch: when it has no
    /// project, it takes the link with the earliest `linked_at` (ties:
    /// the lowest project id) among those whose project `is_known`
    /// accepts (blank ids are never taken); when none is known it keeps
    /// no project. Then all of its link rows are deleted, including when
    /// it already had a project, which it keeps. `updated_at` is left
    /// alone: this is not a user change. Every such channel counts as
    /// changed, so a second call returns 0. Links of a channel that no
    /// longer exists are deleted too (logged, not counted); rows with a
    /// malformed key are skipped (logged) and left in place.
    ///
    /// A channel whose rows cannot be migrated (a link that fails to
    /// unseal or decode, a failed write) is logged with `warn!` and left
    /// as it was, rows included, and the others are still migrated; it
    /// is not counted, and a rerun tries it again.
    ///
    /// Runs under the write lock, so it does not interleave with
    /// [`Self::update`], [`Self::archive`] or [`Self::link_project`].
    ///
    /// # Errors
    /// Store, unsealing and decoding failures of the scan of the link
    /// rows; nothing is migrated then.
    pub fn migrate_links(&self, is_known: impl Fn(&str) -> bool) -> Result<usize, ChannelError> {
        let _guard = lock(&self.write_lock);
        let prefix = LINK_PREFIX.as_bytes();
        let rows = self.scan(cf::CHANNELS, prefix, prefix, "project links")?;
        let mut by_channel: BTreeMap<String, Vec<Row>> = BTreeMap::new();
        for (key, value) in rows {
            let Some(channel_id) = link_key_channel(&key) else {
                tracing::warn!(
                    key_len = key.len(),
                    "skipping project link row with a malformed key"
                );
                continue;
            };
            by_channel
                .entry(channel_id.to_string())
                .or_default()
                .push((key, value));
        }
        let mut changed = 0;
        for (channel_id, rows) in by_channel {
            match self.migrate_channel_links(&channel_id, &rows, &is_known) {
                Ok(true) => changed += 1,
                Ok(false) => {}
                Err(error) => tracing::warn!(
                    channel_id = %channel_id,
                    %error,
                    "could not migrate the project links of a channel; left for the next run"
                ),
            }
        }
        Ok(changed)
    }

    /// Migrate the link `rows` of channel `channel_id` in one batch;
    /// whether the channel exists (and so changed).
    fn migrate_channel_links(
        &self,
        channel_id: &str,
        rows: &[Row],
        is_known: &impl Fn(&str) -> bool,
    ) -> Result<bool, ChannelError> {
        let what = format!("project links of channel {channel_id}");
        let links = rows
            .iter()
            .map(|(_, value)| self.decode::<ProjectLink>(value, &what))
            .collect::<Result<Vec<_>, _>>()?;
        let channels = self.cf(cf::CHANNELS)?;
        let mut batch = WriteBatch::default();
        for (key, _) in rows {
            batch.delete_cf(&channels, key);
        }
        let Some(mut record) = self.get(channel_id)? else {
            self.commit(batch, &what)?;
            tracing::warn!(
                channel_id,
                dropped = links.len(),
                "deleted project links of a missing channel"
            );
            return Ok(false);
        };
        let mut dropped = links.len();
        if record.project_id.is_none() {
            let earliest = links
                .iter()
                .filter(|link| {
                    validate_project_id(&link.project_id).is_ok() && is_known(&link.project_id)
                })
                .min_by(|a, b| {
                    a.linked_at
                        .cmp(&b.linked_at)
                        .then_with(|| a.project_id.cmp(&b.project_id))
                });
            if let Some(link) = earliest {
                record.project_id = Some(link.project_id.clone());
                dropped -= 1;
                let value = self.encode(&record, &format!("channel {channel_id}"))?;
                batch.put_cf(&channels, channel_key(channel_id), value);
            }
        }
        self.commit(batch, &what)?;
        tracing::info!(
            channel_id,
            kept_project = record.project_id.as_deref().unwrap_or(""),
            dropped,
            "migrated legacy project links"
        );
        Ok(true)
    }

    /// Legacy: link `project_id` to channel `channel_id`; relinking
    /// refreshes `linked_at`. A channel's project now lives on its record
    /// (see [`Self::update`]); kept until its callers move, and its rows
    /// are folded in by [`Self::migrate_links`].
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`], [`ChannelError::InvalidProjectId`],
    /// [`ChannelError::UnknownChannel`]; store, sealing and encoding
    /// failures.
    pub fn link_project(
        &self,
        channel_id: &str,
        project_id: &str,
        now: DateTime<Utc>,
    ) -> Result<ProjectLink, ChannelError> {
        validate_project_id(project_id)?;
        let _guard = lock(&self.write_lock);
        self.require_channel(channel_id)?;
        let link = ProjectLink {
            project_id: project_id.to_string(),
            linked_at: now,
        };
        let what = format!("project link {channel_id}/{project_id}");
        let value = self.encode(&link, &what)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &self.cf(cf::CHANNELS)?,
            link_key(channel_id, project_id),
            value,
        );
        self.commit(batch, &what)?;
        Ok(link)
    }

    /// Legacy: unlink `project_id` from channel `channel_id`. Unlinking a
    /// project that isn't linked is a no-op. Kept until its callers move
    /// to the record's `project_id`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`], [`ChannelError::InvalidProjectId`];
    /// store failures.
    pub fn unlink_project(&self, channel_id: &str, project_id: &str) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        validate_project_id(project_id)?;
        let mut batch = WriteBatch::default();
        batch.delete_cf(&self.cf(cf::CHANNELS)?, link_key(channel_id, project_id));
        self.commit(batch, &format!("project link {channel_id}/{project_id}"))
    }

    /// When the coordinator of channel `channel_id` last read the
    /// channel's activity, in Unix milliseconds; `None` when it never did.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store failures.
    pub fn seen(&self, channel_id: &str) -> Result<Option<u64>, ChannelError> {
        validate_id(channel_id)?;
        self.read(
            cf::CHANNELS,
            &seen_key(channel_id),
            &format!("last read of channel {channel_id}"),
        )
    }

    /// Record that the coordinator of channel `channel_id` read its
    /// activity up to `at_ms`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; sealing, encoding and store failures.
    pub fn set_seen(&self, channel_id: &str, at_ms: u64) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        let what = format!("last read of channel {channel_id}");
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &self.cf(cf::CHANNELS)?,
            seen_key(channel_id),
            self.encode(&at_ms, &what)?,
        );
        self.commit(batch, &what)
    }

    /// Legacy: projects linked to channel `channel_id` by `p/` rows, in
    /// key order (by project id). Empty once [`Self::migrate_links`] ran;
    /// the channel's project is [`ChannelRecord::project_id`]. Kept until
    /// its callers move.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn projects(&self, channel_id: &str) -> Result<Vec<ProjectLink>, ChannelError> {
        validate_id(channel_id)?;
        let prefix = link_prefix(channel_id);
        let what = format!("project links of channel {channel_id}");
        self.scan(cf::CHANNELS, &prefix, &prefix, &what)?
            .into_iter()
            .map(|(_, value)| self.decode(&value, &what))
            .collect()
    }

    /// Add `member` to channel `channel_id` in one batch (member row and
    /// reverse index). Adding an agent already in this channel is a
    /// no-op that keeps the existing record.
    ///
    /// # Errors
    /// [`ChannelError::UnknownChannel`], [`ChannelError::AlreadyMember`]
    /// when the agent is in another channel; store, sealing and encoding
    /// failures.
    pub fn add_member(&self, channel_id: &str, member: &MemberRecord) -> Result<(), ChannelError> {
        let _guard = lock(&self.write_lock);
        self.require_channel(channel_id)?;
        match self.channel_of(&member.agent_id)? {
            Some(channel) if channel == channel_id => return Ok(()),
            Some(channel) => {
                return Err(ChannelError::AlreadyMember {
                    agent: member.agent_id.to_hex(),
                    channel,
                })
            }
            None => {}
        }
        let mut batch = WriteBatch::default();
        self.stage_member(channel_id, member, &mut batch)?;
        self.commit(
            batch,
            &format!(
                "member {} of channel {channel_id}",
                member.agent_id.to_hex()
            ),
        )
    }

    /// Add the member row and reverse index of `member` to `batch`, for a
    /// caller committing them together with rows of another store (e.g.
    /// a new agent's row), then [`Self::write`].
    ///
    /// Performs no checks: the caller must first confirm, under its own
    /// serialization, that the channel exists and that
    /// [`Self::channel_of`] the agent is `None`, or the one-channel-per-
    /// agent invariant can break. On error `batch` is left as it was.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; sealing, encoding and
    /// missing-column-family failures.
    pub fn stage_member(
        &self,
        channel_id: &str,
        member: &MemberRecord,
        batch: &mut WriteBatch,
    ) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        let agent = member.agent_id;
        let what = format!("member {} of channel {channel_id}", agent.to_hex());
        let member_value = self.encode(member, &what)?;
        let reverse_value = self.encode(&channel_id, &format!("channel of {}", agent.to_hex()))?;
        let members = self.cf(cf::CHANNEL_MEMBERS)?;
        batch.put_cf(&members, member_key(channel_id, &agent), member_value);
        batch.put_cf(&members, reverse_key(&agent), reverse_value);
        Ok(())
    }

    /// Commit a batch built with [`Self::stage_member`] (and other
    /// stores' `stage` methods) in one write.
    ///
    /// # Errors
    /// Store failures; nothing is written on error.
    pub fn write(&self, batch: WriteBatch) -> Result<(), ChannelError> {
        self.commit(batch, "channel batch")
    }

    /// Remove `agent` from channel `channel_id`. The reverse index is
    /// only deleted when it points at this channel; removing an agent
    /// that isn't a member is a no-op.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn remove_member(&self, channel_id: &str, agent: &AgentId) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        let _guard = lock(&self.write_lock);
        let members = self.cf(cf::CHANNEL_MEMBERS)?;
        let mut batch = WriteBatch::default();
        batch.delete_cf(&members, member_key(channel_id, agent));
        if self.channel_of(agent)?.as_deref() == Some(channel_id) {
            batch.delete_cf(&members, reverse_key(agent));
        }
        self.commit(
            batch,
            &format!("removing member {} of channel {channel_id}", agent.to_hex()),
        )
    }

    /// Members of channel `channel_id`: the coordinator first, then by
    /// `joined_at`, ties by agent id.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn members(&self, channel_id: &str) -> Result<Vec<MemberRecord>, ChannelError> {
        validate_id(channel_id)?;
        let prefix = member_prefix(channel_id);
        let what = format!("members of channel {channel_id}");
        let rows = self.scan(cf::CHANNEL_MEMBERS, &prefix, &prefix, &what)?;
        let mut members = Vec::with_capacity(rows.len());
        for (key, value) in rows {
            let Some(agent) = key
                .get(prefix.len()..)
                .and_then(|tail| <[u8; 32]>::try_from(tail).ok())
                .map(AgentId::new)
            else {
                tracing::warn!(
                    channel_id,
                    key_len = key.len(),
                    "skipping member row with a malformed key"
                );
                continue;
            };
            let record: MemberRecord = self.decode(&value, &what)?;
            if record.agent_id != agent {
                tracing::warn!(
                    channel_id,
                    agent_id = %agent.to_hex(),
                    "skipping member row whose key is not its agent id"
                );
                continue;
            }
            members.push(record);
        }
        // Stable sort: rows came in agent id order, so ties stay in it.
        members.sort_by_key(|member| (member.role != MemberRole::Coordinator, member.joined_at));
        Ok(members)
    }

    /// The id of the channel `agent` is in, if any.
    ///
    /// # Errors
    /// Store, unsealing and decoding failures.
    pub fn channel_of(&self, agent: &AgentId) -> Result<Option<String>, ChannelError> {
        self.read(
            cf::CHANNEL_MEMBERS,
            &reverse_key(agent),
            &format!("channel of {}", agent.to_hex()),
        )
    }

    /// Every channel member, as `(channel id, agent)`, from the reverse
    /// index. Rows with a malformed key are skipped (logged).
    ///
    /// # Errors
    /// Store, unsealing and decoding failures.
    pub fn all_members(&self) -> Result<Vec<(String, AgentId)>, ChannelError> {
        let rows = self.scan(
            cf::CHANNEL_MEMBERS,
            REVERSE_PREFIX,
            REVERSE_PREFIX,
            "channel members",
        )?;
        let mut members = Vec::with_capacity(rows.len());
        for (key, value) in rows {
            let Some(agent) = key
                .get(REVERSE_PREFIX.len()..)
                .and_then(|tail| <[u8; 32]>::try_from(tail).ok())
                .map(AgentId::new)
            else {
                tracing::warn!(
                    key_len = key.len(),
                    "skipping channel reverse-index row with a malformed key"
                );
                continue;
            };
            let channel_id: String = self.decode(&value, "channel of a member")?;
            members.push((channel_id, agent));
        }
        Ok(members)
    }

    /// Append a post to channel `channel_id`, assigning it the next
    /// sequence number.
    ///
    /// # Errors
    /// [`ChannelError::UnknownChannel`]; store, sealing and encoding
    /// failures. No sequence number is used up on error.
    pub fn append_post(&self, channel_id: &str, new: NewPost) -> Result<ChannelPost, ChannelError> {
        self.require_channel(channel_id)?;
        let mut last = lock(&self.last_post_seq);
        let seq = match *last {
            Some(seq) => seq,
            None => self.max_post_seq()?,
        }
        .saturating_add(1);
        let post = ChannelPost {
            channel_id: channel_id.to_string(),
            seq,
            author: new.author,
            text: new.text,
            mentions: new.mentions,
            reply_to: new.reply_to,
            at_ms: new.at_ms,
        };
        let what = format!("post {seq} of channel {channel_id}");
        let value = self.encode(&post, &what)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &self.cf(cf::CHANNEL_POSTS)?,
            post_key(channel_id, post.at_ms, seq),
            value,
        );
        self.commit(batch, &what)?;
        *last = Some(seq);
        Ok(post)
    }

    /// The highest sequence number in any post key, 0 when there are
    /// none. Reads keys only, so nothing is unsealed.
    fn max_post_seq(&self) -> Result<u64, ChannelError> {
        let cf = self.cf(cf::CHANNEL_POSTS)?;
        let mut max = 0;
        for item in self.db.iterator_cf(&cf, IteratorMode::Start) {
            let (key, _) = item.map_err(|source| ChannelError::Backend {
                context: "reading channel posts".to_string(),
                source,
            })?;
            match post_key_seq(&key) {
                Some(seq) => max = max.max(seq),
                None => tracing::warn!(
                    key_len = key.len(),
                    "skipping post row with a malformed key"
                ),
            }
        }
        Ok(max)
    }

    /// Posts of channel `channel_id` in time order (ties by sequence
    /// number); with `since_ms`, only those with `at_ms > since_ms`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn posts(
        &self,
        channel_id: &str,
        since_ms: Option<u64>,
    ) -> Result<Vec<ChannelPost>, ChannelError> {
        validate_id(channel_id)?;
        let prefix = post_prefix(channel_id);
        let start = match since_ms {
            None => prefix.clone(),
            Some(since) => match since.checked_add(1) {
                Some(from) => format!("{channel_id}/{from:020}/").into_bytes(),
                None => return Ok(Vec::new()),
            },
        };
        let what = format!("posts of channel {channel_id}");
        self.scan(cf::CHANNEL_POSTS, &prefix, &start, &what)?
            .into_iter()
            .map(|(_, value)| self.decode(&value, &what))
            .collect()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ChannelStore {
    /// Fault-injected variant of [`Self::create`]; see
    /// [`crate::FaultAt`]. `BeforeBatchWrite` writes nothing,
    /// `AfterBatchWrite` commits the whole channel, and `InsideBatch`
    /// commits only the coordinator's agent row, the partial state a
    /// non-atomic writer leaves.
    ///
    /// # Errors
    /// Always fails: with [`ChannelError::InjectedFault`] once the writes
    /// of the fault mode are done, or earlier with the error that stopped
    /// them.
    pub fn create_with_fault(
        &self,
        new: NewChannel,
        coordinator: &AgentRecord,
        now: DateTime<Utc>,
        fault: crate::FaultAt,
    ) -> Result<ChannelRecord, ChannelError> {
        let _guard = lock(&self.write_lock);
        let (_, full, partial) = self.prepare_create(new, coordinator, now)?;
        match crate::rocks_store::write_batch_with_fault(
            &self.db,
            &self.write_opts(),
            full,
            partial,
            fault,
        ) {
            crate::rocks_store::FaultWriteError::Injected(fault) => {
                Err(ChannelError::InjectedFault(fault))
            }
            crate::rocks_store::FaultWriteError::Write(source) => Err(ChannelError::Backend {
                context: "writing channel batch".to_string(),
                source,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
