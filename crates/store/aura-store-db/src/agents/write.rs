//! Writing the registry: a diff is validated and encoded in full, then
//! staged into one `WriteBatch`.

use super::{
    derive_agent_id, AgentRecord, AgentStore, AgentStoreError, RegistryDiff, RegistryMeta,
    META_ACTIVE, META_LAST_NUMBERS, META_PREFIXES, META_VERSION,
};
use crate::cf;
use rocksdb::{WriteBatch, WriteOptions};

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

impl AgentStore {
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

#[doc(hidden)]
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
