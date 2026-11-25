//! Agents and projects of the terminal registry in the agent-state
//! database.
//!
//! The registry used to live in `agents.json`; this store keeps the same
//! data as one row per agent and per project, so a change rewrites only
//! the rows it touches, and other stores sharing the database can commit
//! agent rows in their own batches (see [`AgentStore::stage`]).
//!
//! # Key formats
//!
//! * [`crate::cf::AGENTS`]: the 32 raw bytes of the agent id. Value: a
//!   JSON [`AgentRecord`].
//! * [`crate::cf::PROJECTS`]: the project id as utf-8. Value: a JSON
//!   [`ProjectRecord`].
//! * [`crate::cf::REGISTRY_META`]: the keys `version`, `active`,
//!   `prefixes` and `last_numbers`, each holding the JSON of the
//!   [`RegistryMeta`] field of that name. `active` holds an [`AgentId`]
//!   and is absent when no agent is active.
//!
//! Every value is sealed in sealed mode; keys are plaintext in both modes.
//!
//! # Invariants
//!
//! * An agent row's key is [`derive_agent_id`] of its `zns_id` (blake3 of
//!   the ZNS id, as [`aura_core_types::Identity::new`] derives it), and
//!   its `agent_id` field equals the key. [`AgentStore::stage`] refuses
//!   records that break this.
//! * Every agent a stored row refers to (`parent`, `active`) is named by
//!   its [`AgentId`], never by its ZNS id.
//! * `position` defines list order: [`AgentStore::load`] returns agents
//!   and projects sorted by it, ties broken by key.
//!
//! # Failure modes
//!
//! * An agent row whose key is not 32 bytes, does not equal
//!   [`derive_agent_id`] of its `zns_id`, or differs from its `agent_id`
//!   field is skipped on load with a `warn!` naming the key, never the
//!   payload. So is a project row whose key differs from its `id`.
//! * A diff is committed in one `WriteBatch`: all of it lands or none of
//!   it does. [`AgentStore::load`] reads from one snapshot, so it never
//!   sees half a batch.
//! * A value that fails to unseal (wrong key, tampering) or to decode
//!   fails the whole load: sealed values fail closed instead of
//!   dropping rows.

use crate::cf;
use crate::seal::{SealCipher, SealError};
use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use rocksdb::{
    BoundColumnFamily, DBWithThreadMode, IteratorMode, MultiThreaded, SnapshotWithThreadMode,
    WriteBatch, WriteOptions,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

type Db = DBWithThreadMode<MultiThreaded>;
type Snapshot<'a> = SnapshotWithThreadMode<'a, Db>;

const META_VERSION: &[u8] = b"version";
const META_ACTIVE: &[u8] = b"active";
const META_PREFIXES: &[u8] = b"prefixes";
const META_LAST_NUMBERS: &[u8] = b"last_numbers";

/// The id of the agent with `zns_id`: blake3 of the ZNS id, the same
/// derivation as [`aura_core_types::Identity::new`].
#[must_use]
pub fn derive_agent_id(zns_id: &str) -> AgentId {
    AgentId::new(aura_core_types::hash::hash_bytes(zns_id.as_bytes()))
}

/// Errors produced by [`AgentStore`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentStoreError {
    /// A record's `agent_id` is not [`derive_agent_id`] of its `zns_id`.
    #[error("agent {agent_id}: id is not derived from its zns id (expected {expected})")]
    IdMismatch {
        /// Full hex of the record's `agent_id`.
        agent_id: String,
        /// Full hex of the id derived from its `zns_id`.
        expected: String,
    },
    /// A project id that can't be a key.
    #[error("invalid project id {0:?}: must be non-empty")]
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
    /// A failure injected by [`AgentStore::apply_with_fault`].
    #[cfg(any(test, feature = "test-support"))]
    #[error("injected fault: {0:?}")]
    InjectedFault(crate::FaultAt),
}

/// One agent of the registry, the value of an [`crate::cf::AGENTS`] row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    /// [`derive_agent_id`] of `zns_id`; the row key.
    pub agent_id: AgentId,
    /// ZNS id the agent's identity is derived from. Never changes.
    pub zns_id: String,
    /// The agent's name, its identity in records.
    pub name: String,
    /// When the agent was registered.
    pub created_at: DateTime<Utc>,
    /// Where the agent is listed: lower comes first.
    pub position: u64,
    /// Permanent agent this is an instance of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// The agent that spawned this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AgentId>,
    /// Id of the project the agent is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Folder the agent works in instead of its project's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    /// Moved to the Archive.
    #[serde(default)]
    pub archived: bool,
    /// Taken off the list with its project but kept.
    #[serde(default)]
    pub removed: bool,
    /// Short title of what the agent's chat is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Short code name shown beside the agent, such as `RE36`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Short name of the subtask a spawned instance was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// When the agent was moved to the Archive, if it is archived and that
    /// was recorded; the Archive lists the latest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
}

/// One project of the registry, the value of a [`crate::cf::PROJECTS`]
/// row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRecord {
    /// Project id; the row key.
    pub id: String,
    /// Canonical folder of the project.
    pub folder: PathBuf,
    /// When the project was created.
    pub created_at: DateTime<Utc>,
    /// Where the project is listed: lower comes first.
    pub position: u64,
    /// Taken off the list but kept.
    #[serde(default)]
    pub removed: bool,
}

/// Registry-wide settings, the [`crate::cf::REGISTRY_META`] rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryMeta {
    /// Registry format version.
    #[serde(default)]
    pub version: u32,
    /// The agent that was active when the UI last switched.
    #[serde(default)]
    pub active: Option<AgentId>,
    /// The code prefix each source key (definition name, or `agent`) has
    /// reserved, e.g. `rust-engineer` -> `RE`.
    #[serde(default)]
    pub prefixes: BTreeMap<String, String>,
    /// The highest code number each prefix has ever been given.
    #[serde(default)]
    pub last_numbers: BTreeMap<String, u64>,
}

/// Everything [`AgentStore::load`] read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    /// Agents, sorted by `position`, ties by key.
    pub agents: Vec<AgentRecord>,
    /// Projects, sorted by `position`, ties by key.
    pub projects: Vec<ProjectRecord>,
    /// The settings; `None` when no meta row was ever written.
    pub meta: Option<RegistryMeta>,
}

/// Rows to write or delete in one batch.
///
/// Puts are staged before deletes, so a key in both ends up deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryDiff {
    /// Agent rows to write.
    pub agents_put: Vec<AgentRecord>,
    /// Agent rows to delete.
    pub agents_del: Vec<AgentId>,
    /// Project rows to write.
    pub projects_put: Vec<ProjectRecord>,
    /// Project rows to delete, by id.
    pub projects_del: Vec<String>,
    /// Settings to write in full; `None` leaves them as they are.
    pub meta: Option<RegistryMeta>,
}

impl RegistryDiff {
    /// Whether applying the diff would change nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents_put.is_empty()
            && self.agents_del.is_empty()
            && self.projects_put.is_empty()
            && self.projects_del.is_empty()
            && self.meta.is_none()
    }
}

/// Which column family a staged write goes to.
#[derive(Debug, Clone, Copy)]
enum Family {
    Agents,
    Projects,
    Meta,
}

/// One encoded write, ready for a batch. `value: None` deletes.
struct Op {
    family: Family,
    key: Vec<u8>,
    value: Option<Vec<u8>>,
}

/// Agents and projects of the terminal registry in the shared
/// agent-state `RocksDB`.
#[derive(Clone)]
pub struct AgentStore {
    db: Arc<Db>,
    /// `Some` in sealed mode.
    cipher: Option<Arc<SealCipher>>,
    sync_writes: bool,
}

impl std::fmt::Debug for AgentStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentStore")
            .field("sealed", &self.cipher.is_some())
            .field("sync_writes", &self.sync_writes)
            .finish_non_exhaustive()
    }
}

impl AgentStore {
    /// Plaintext store on a shared DB handle.
    #[must_use]
    pub const fn new(db: Arc<Db>) -> Self {
        Self::with_cipher(db, None)
    }

    /// Store with optional sealed values.
    #[must_use]
    pub const fn with_cipher(db: Arc<Db>, cipher: Option<Arc<SealCipher>>) -> Self {
        Self {
            db,
            cipher,
            sync_writes: false,
        }
    }

    /// Store sharing `store`'s database, sealing mode and write
    /// durability.
    #[must_use]
    pub fn for_store(store: &crate::RocksStore) -> Self {
        Self {
            db: Arc::clone(store.db_handle()),
            cipher: store.cipher().cloned(),
            sync_writes: store.sync_writes(),
        }
    }

    fn cf(&self, name: &'static str) -> Result<Arc<BoundColumnFamily<'_>>, AgentStoreError> {
        self.db
            .cf_handle(name)
            .ok_or(AgentStoreError::MissingColumnFamily(name))
    }

    fn write_opts(&self) -> WriteOptions {
        let mut opts = WriteOptions::default();
        opts.set_sync(self.sync_writes);
        opts
    }

    /// JSON-encode `value` and seal it in sealed mode.
    fn encode<T: Serialize>(&self, value: &T, what: &str) -> Result<Vec<u8>, AgentStoreError> {
        let plain = serde_json::to_vec(value).map_err(|source| AgentStoreError::Serde {
            context: format!("encoding {what}"),
            source,
        })?;
        match &self.cipher {
            Some(cipher) => cipher.seal(&plain).map_err(|source| AgentStoreError::Seal {
                context: format!("sealing {what}"),
                source,
            }),
            None => Ok(plain),
        }
    }

    /// Unseal `bytes` in sealed mode and JSON-decode them.
    fn decode<T: DeserializeOwned>(&self, bytes: &[u8], what: &str) -> Result<T, AgentStoreError> {
        let plain = match &self.cipher {
            Some(cipher) => {
                Cow::Owned(cipher.open(bytes).map_err(|source| AgentStoreError::Seal {
                    context: format!("opening {what}"),
                    source,
                })?)
            }
            None => Cow::Borrowed(bytes),
        };
        serde_json::from_slice(&plain).map_err(|source| AgentStoreError::Serde {
            context: format!("decoding {what}"),
            source,
        })
    }

    /// Every row, agents and projects sorted by `position` (ties by key),
    /// read from one snapshot.
    ///
    /// # Errors
    /// Surfaces store, unsealing and decoding failures. Rows that break
    /// the key invariants are skipped, not errors (see the module docs).
    pub fn load(&self) -> Result<RegistrySnapshot, AgentStoreError> {
        let snapshot = self.db.snapshot();
        Ok(RegistrySnapshot {
            agents: self.load_agents(&snapshot)?,
            projects: self.load_projects(&snapshot)?,
            meta: self.load_meta(&snapshot)?,
        })
    }

    fn load_agents(&self, snapshot: &Snapshot<'_>) -> Result<Vec<AgentRecord>, AgentStoreError> {
        let cf = self.cf(cf::AGENTS)?;
        let mut agents = Vec::new();
        for item in snapshot.iterator_cf(&cf, IteratorMode::Start) {
            let (key, value) = item.map_err(|source| AgentStoreError::Backend {
                context: "reading agents".to_string(),
                source,
            })?;
            let Ok(key) = <[u8; 32]>::try_from(key.as_ref()) else {
                tracing::warn!(
                    key_len = key.len(),
                    "skipping agent row with a malformed key"
                );
                continue;
            };
            let key = AgentId::new(key);
            let record: AgentRecord = self.decode(&value, &format!("agent {}", key.to_hex()))?;
            if record.agent_id != key || derive_agent_id(&record.zns_id) != key {
                tracing::warn!(
                    agent_id = %key.to_hex(),
                    "skipping agent row whose key is not derived from its zns id"
                );
                continue;
            }
            agents.push(record);
        }
        // Stable sort: rows came in key order, so ties stay in key order.
        agents.sort_by_key(|agent| agent.position);
        Ok(agents)
    }

    fn load_projects(
        &self,
        snapshot: &Snapshot<'_>,
    ) -> Result<Vec<ProjectRecord>, AgentStoreError> {
        let cf = self.cf(cf::PROJECTS)?;
        let mut projects = Vec::new();
        for item in snapshot.iterator_cf(&cf, IteratorMode::Start) {
            let (key, value) = item.map_err(|source| AgentStoreError::Backend {
                context: "reading projects".to_string(),
                source,
            })?;
            let key = String::from_utf8_lossy(&key);
            let record: ProjectRecord = self.decode(&value, &format!("project {key}"))?;
            if record.id != key {
                tracing::warn!(project_id = %key, "skipping project row whose key is not its id");
                continue;
            }
            projects.push(record);
        }
        projects.sort_by_key(|project| project.position);
        Ok(projects)
    }

    fn load_meta(&self, snapshot: &Snapshot<'_>) -> Result<Option<RegistryMeta>, AgentStoreError> {
        let cf = self.cf(cf::REGISTRY_META)?;
        let get = |key: &[u8]| {
            snapshot
                .get_cf(&cf, key)
                .map_err(|source| AgentStoreError::Backend {
                    context: format!("reading registry meta {}", String::from_utf8_lossy(key)),
                    source,
                })
        };
        let version = get(META_VERSION)?;
        let active = get(META_ACTIVE)?;
        let prefixes = get(META_PREFIXES)?;
        let last_numbers = get(META_LAST_NUMBERS)?;
        if version.is_none() && active.is_none() && prefixes.is_none() && last_numbers.is_none() {
            return Ok(None);
        }
        Ok(Some(RegistryMeta {
            version: self.decode_or_default(version, "registry meta version")?,
            active: self.decode_or_default(active, "registry meta active")?,
            prefixes: self.decode_or_default(prefixes, "registry meta prefixes")?,
            last_numbers: self.decode_or_default(last_numbers, "registry meta last_numbers")?,
        }))
    }

    fn decode_or_default<T: DeserializeOwned + Default>(
        &self,
        bytes: Option<Vec<u8>>,
        what: &str,
    ) -> Result<T, AgentStoreError> {
        bytes.map_or_else(|| Ok(T::default()), |bytes| self.decode(&bytes, what))
    }

    /// Whether no agent row is stored.
    ///
    /// # Errors
    /// Surfaces store failures.
    pub fn is_empty(&self) -> Result<bool, AgentStoreError> {
        let cf = self.cf(cf::AGENTS)?;
        match self.db.iterator_cf(&cf, IteratorMode::Start).next() {
            None => Ok(true),
            Some(Ok(_)) => Ok(false),
            Some(Err(source)) => Err(AgentStoreError::Backend {
                context: "reading agents".to_string(),
                source,
            }),
        }
    }

    /// Write `diff` in one batch: all of it or none of it.
    ///
    /// # Errors
    /// Rejects records that break the key invariants; surfaces sealing,
    /// encoding and store failures. Nothing is written on error.
    pub fn apply(&self, diff: &RegistryDiff) -> Result<(), AgentStoreError> {
        self.write(diff, &self.write_opts())
    }

    /// [`Self::apply`], but the batch is fsynced before this returns,
    /// whatever the store's `sync_writes` setting.
    ///
    /// For writes another durable step depends on, such as renaming the
    /// file a batch was imported from: without the sync a power loss can
    /// keep the rename and lose the batch.
    ///
    /// # Errors
    /// As [`Self::apply`].
    pub fn apply_durable(&self, diff: &RegistryDiff) -> Result<(), AgentStoreError> {
        let mut opts = WriteOptions::default();
        opts.set_sync(true);
        self.write(diff, &opts)
    }

    fn write(&self, diff: &RegistryDiff, opts: &WriteOptions) -> Result<(), AgentStoreError> {
        let mut batch = WriteBatch::default();
        self.stage(diff, &mut batch)?;
        if batch.is_empty() {
            return Ok(());
        }
        self.db
            .write_opt(batch, opts)
            .map_err(|source| AgentStoreError::Backend {
                context: "writing registry batch".to_string(),
                source,
            })
    }

    /// Add the writes of `diff` to `batch`, for a caller committing them
    /// together with rows of another store.
    ///
    /// Every row is validated and encoded before the first is added, so
    /// on error `batch` is left as it was.
    ///
    /// # Errors
    /// Rejects records that break the key invariants; surfaces sealing,
    /// encoding and missing-column-family failures.
    pub fn stage(
        &self,
        diff: &RegistryDiff,
        batch: &mut WriteBatch,
    ) -> Result<(), AgentStoreError> {
        let ops = self.diff_ops(diff)?;
        self.push_ops(ops, batch)
    }

    fn diff_ops(&self, diff: &RegistryDiff) -> Result<Vec<Op>, AgentStoreError> {
        let mut ops = self.agent_put_ops(&diff.agents_put)?;
        ops.extend(diff.agents_del.iter().map(|id| Op {
            family: Family::Agents,
            key: id.as_bytes().to_vec(),
            value: None,
        }));
        for project in &diff.projects_put {
            if project.id.is_empty() {
                return Err(AgentStoreError::InvalidProjectId(project.id.clone()));
            }
            ops.push(Op {
                family: Family::Projects,
                key: project.id.as_bytes().to_vec(),
                value: Some(self.encode(project, &format!("project {}", project.id))?),
            });
        }
        ops.extend(diff.projects_del.iter().map(|id| Op {
            family: Family::Projects,
            key: id.as_bytes().to_vec(),
            value: None,
        }));
        if let Some(meta) = &diff.meta {
            self.meta_ops(meta, &mut ops)?;
        }
        Ok(ops)
    }

    fn agent_put_ops(&self, agents: &[AgentRecord]) -> Result<Vec<Op>, AgentStoreError> {
        agents
            .iter()
            .map(|agent| {
                let expected = derive_agent_id(&agent.zns_id);
                if agent.agent_id != expected {
                    return Err(AgentStoreError::IdMismatch {
                        agent_id: agent.agent_id.to_hex(),
                        expected: expected.to_hex(),
                    });
                }
                Ok(Op {
                    family: Family::Agents,
                    key: agent.agent_id.as_bytes().to_vec(),
                    value: Some(self.encode(agent, &format!("agent {}", expected.to_hex()))?),
                })
            })
            .collect()
    }

    fn meta_ops(&self, meta: &RegistryMeta, ops: &mut Vec<Op>) -> Result<(), AgentStoreError> {
        let put = |key: &[u8], value: Vec<u8>| Op {
            family: Family::Meta,
            key: key.to_vec(),
            value: Some(value),
        };
        ops.push(put(
            META_VERSION,
            self.encode(&meta.version, "registry meta version")?,
        ));
        ops.push(match &meta.active {
            Some(active) => put(META_ACTIVE, self.encode(active, "registry meta active")?),
            None => Op {
                family: Family::Meta,
                key: META_ACTIVE.to_vec(),
                value: None,
            },
        });
        ops.push(put(
            META_PREFIXES,
            self.encode(&meta.prefixes, "registry meta prefixes")?,
        ));
        ops.push(put(
            META_LAST_NUMBERS,
            self.encode(&meta.last_numbers, "registry meta last_numbers")?,
        ));
        Ok(())
    }

    fn push_ops(&self, ops: Vec<Op>, batch: &mut WriteBatch) -> Result<(), AgentStoreError> {
        // Resolve every handle before touching the batch, so a missing
        // family can't leave it half staged.
        let agents = self.cf(cf::AGENTS)?;
        let projects = self.cf(cf::PROJECTS)?;
        let meta = self.cf(cf::REGISTRY_META)?;
        for op in ops {
            let cf = match op.family {
                Family::Agents => &agents,
                Family::Projects => &projects,
                Family::Meta => &meta,
            };
            match op.value {
                Some(value) => batch.put_cf(cf, op.key, value),
                None => batch.delete_cf(cf, op.key),
            }
        }
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AgentStore {
    /// Fault-injected variant of [`Self::apply`]; see
    /// [`crate::FaultAt`]. `BeforeBatchWrite` writes nothing,
    /// `AfterBatchWrite` commits the whole diff, and `InsideBatch` commits
    /// only the agent puts, the partial state a non-atomic writer leaves.
    ///
    /// # Errors
    /// Always fails: with [`AgentStoreError::InjectedFault`] once the
    /// writes of the fault mode are done, or earlier with the error that
    /// stopped them.
    pub fn apply_with_fault(
        &self,
        diff: &RegistryDiff,
        fault: crate::FaultAt,
    ) -> Result<(), AgentStoreError> {
        let mut full = WriteBatch::default();
        self.stage(diff, &mut full)?;
        let mut partial = WriteBatch::default();
        self.push_ops(self.agent_put_ops(&diff.agents_put)?, &mut partial)?;
        match crate::rocks_store::write_batch_with_fault(
            &self.db,
            &self.write_opts(),
            full,
            partial,
            fault,
        ) {
            crate::rocks_store::FaultWriteError::Injected(fault) => {
                Err(AgentStoreError::InjectedFault(fault))
            }
            crate::rocks_store::FaultWriteError::Write(source) => Err(AgentStoreError::Backend {
                context: "writing registry batch".to_string(),
                source,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
