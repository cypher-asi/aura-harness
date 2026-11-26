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
//!
//! # Layout
//!
//! * `records`: the row types and [`RegistryDiff`].
//! * `error`: [`AgentStoreError`].
//! * `load`: reading every row from one snapshot.
//! * `write`: validating, encoding and staging a diff into one batch.

use crate::seal::SealCipher;
use aura_core_types::AgentId;
use rocksdb::{BoundColumnFamily, DBWithThreadMode, MultiThreaded, WriteOptions};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::borrow::Cow;
use std::sync::Arc;

mod error;
mod load;
mod records;
mod write;

pub use error::AgentStoreError;
pub use records::{AgentRecord, ProjectRecord, RegistryDiff, RegistryMeta, RegistrySnapshot};

type Db = DBWithThreadMode<MultiThreaded>;

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
}

#[cfg(test)]
mod tests;
