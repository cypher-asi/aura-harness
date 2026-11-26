//! Reading the registry: every row from one snapshot.

use super::{
    derive_agent_id, AgentRecord, AgentStore, AgentStoreError, Db, ProjectRecord, RegistryMeta,
    RegistrySnapshot, META_ACTIVE, META_LAST_NUMBERS, META_PREFIXES, META_VERSION,
};
use crate::cf;
use aura_core_types::AgentId;
use rocksdb::{IteratorMode, SnapshotWithThreadMode};
use serde::de::DeserializeOwned;

type Snapshot<'a> = SnapshotWithThreadMode<'a, Db>;

impl AgentStore {
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
}
