//! The in-memory [`RegistryFile`] as rows of [`aura_store_db::AgentStore`],
//! and the diff that brings the stored rows up to date with it.
//!
//! # Mapping
//!
//! * List order is the order of the `Vec`s: an entry at index `i` is
//!   stored with `position = i * POSITION_STEP`, and loading sorts by
//!   position, so the order round-trips.
//! * In memory, parents and the active agent are ZNS ids; stored, they
//!   are [`AgentId`]s. A parent naming a registered agent becomes that
//!   agent's id. A parent naming no registered agent (a dangling link)
//!   is kept: it is stored as [`derive_agent_id`] of the ZNS id, and read
//!   back as the placeholder `agent-id:<hex>`, which maps back to the
//!   same id. So a dangling parent stays `Some(..)` (which
//!   [`super::RegisteredAgent::is_spawned_instance`] relies on) and the
//!   rows stay byte-stable across loads.
//! * An active agent that is no registered agent loads as `None`.

use super::{
    code, AgentRegistry, AgentRegistryError, IdCache, Project, RegisteredAgent, RegistryFile,
};
use aura_core_types::AgentId;
use aura_store_db::agents::{
    derive_agent_id, AgentRecord, ProjectRecord, RegistryDiff, RegistryMeta, RegistrySnapshot,
};
use std::collections::{HashMap, HashSet};

/// Gap between the positions of neighbouring entries, leaving room to
/// insert between them later without renumbering.
const POSITION_STEP: u64 = 1024;
/// How a parent that names no registered agent is held in memory.
const PLACEHOLDER_PREFIX: &str = "agent-id:";

/// The rows a [`RegistryFile`] is stored as.
#[derive(Debug, Clone)]
pub(super) struct Rows {
    pub(super) agents: Vec<AgentRecord>,
    pub(super) projects: Vec<ProjectRecord>,
    pub(super) meta: RegistryMeta,
}

fn position(index: usize) -> u64 {
    u64::try_from(index)
        .unwrap_or(u64::MAX)
        .saturating_mul(POSITION_STEP)
}

/// The id `link` (a parent or active agent, in memory) is stored as.
fn link_id(link: &str, by_zns: &HashMap<&str, AgentId>) -> AgentId {
    if let Some(id) = by_zns.get(link) {
        return *id;
    }
    link.strip_prefix(PLACEHOLDER_PREFIX)
        .and_then(|hex| AgentId::from_hex(hex).ok())
        .unwrap_or_else(|| derive_agent_id(link))
}

/// The ids of `file`'s agents by ZNS id, for [`agent_row`].
fn ids_by_zns(file: &RegistryFile) -> HashMap<&str, AgentId> {
    file.agents
        .iter()
        .map(|agent| (agent.zns_id.as_str(), agent.agent_id()))
        .collect()
}

/// `agent`'s row when it is at `index` in a registry whose agents are
/// `by_zns`.
fn agent_row(
    agent: &RegisteredAgent,
    index: usize,
    by_zns: &HashMap<&str, AgentId>,
) -> AgentRecord {
    AgentRecord {
        agent_id: agent.agent_id(),
        zns_id: agent.zns_id.clone(),
        name: agent.name.clone(),
        created_at: agent.created_at,
        position: position(index),
        definition: agent.definition.clone(),
        parent: agent.parent.as_deref().map(|p| link_id(p, by_zns)),
        project_id: agent.project_id.clone(),
        folder: agent.folder.clone(),
        archived: agent.archived,
        removed: agent.removed,
        title: agent.title.clone(),
        code: agent.code.clone(),
        subtitle: agent.subtitle.clone(),
        archived_at: agent.archived_at,
    }
}

/// The row `agent`, not yet in `file`, has once it is appended to it.
pub(super) fn appended_row(file: &RegistryFile, agent: &RegisteredAgent) -> AgentRecord {
    agent_row(agent, file.agents.len(), &ids_by_zns(file))
}

/// The rows `changed` (agents at their index in `file`, as they will be
/// once put back there) have in `file`. Parents are resolved against
/// `file`'s agents, so `changed` must keep the ZNS ids they have there.
pub(super) fn rows_at(
    file: &RegistryFile,
    changed: &[(usize, RegisteredAgent)],
) -> Vec<AgentRecord> {
    let by_zns = ids_by_zns(file);
    changed
        .iter()
        .map(|(index, agent)| agent_row(agent, *index, &by_zns))
        .collect()
}

/// `file` as rows, in its list order.
pub(super) fn to_rows(file: &RegistryFile) -> Rows {
    let by_zns = ids_by_zns(file);
    let agents = file
        .agents
        .iter()
        .enumerate()
        .map(|(index, agent)| agent_row(agent, index, &by_zns))
        .collect();
    let projects = file
        .projects
        .iter()
        .enumerate()
        .map(|(index, project)| ProjectRecord {
            id: project.id.clone(),
            folder: project.folder.clone(),
            created_at: project.created_at,
            position: position(index),
            removed: project.removed,
        })
        .collect();
    let meta = RegistryMeta {
        version: file.version,
        active: file.active.as_deref().map(|a| link_id(a, &by_zns)),
        prefixes: file.prefixes.to_map(),
        last_numbers: file.last_numbers.to_map(),
    };
    Rows {
        agents,
        projects,
        meta,
    }
}

/// The registry `snapshot` holds, in its position order.
pub(super) fn from_snapshot(snapshot: RegistrySnapshot) -> RegistryFile {
    let zns_of: HashMap<AgentId, String> = snapshot
        .agents
        .iter()
        .map(|agent| (agent.agent_id, agent.zns_id.clone()))
        .collect();
    let agents = snapshot
        .agents
        .into_iter()
        .map(|record| RegisteredAgent {
            parent: record.parent.map(|parent| {
                zns_of
                    .get(&parent)
                    .cloned()
                    .unwrap_or_else(|| format!("{PLACEHOLDER_PREFIX}{}", parent.to_hex()))
            }),
            zns_id: record.zns_id,
            name: record.name,
            created_at: record.created_at,
            definition: record.definition,
            project_id: record.project_id,
            folder: record.folder,
            archived: record.archived,
            removed: record.removed,
            title: record.title,
            code: record.code,
            subtitle: record.subtitle,
            archived_at: record.archived_at,
            legacy_member_of: None,
            id: IdCache::default(),
        })
        .collect();
    let projects = snapshot
        .projects
        .into_iter()
        .map(|record| Project {
            id: record.id,
            folder: record.folder,
            created_at: record.created_at,
            removed: record.removed,
        })
        .collect();
    let meta = snapshot.meta.unwrap_or_default();
    RegistryFile {
        version: meta.version,
        active: meta.active.and_then(|id| zns_of.get(&id).cloned()),
        projects,
        agents,
        prefixes: code::PrefixTable::from_map(meta.prefixes),
        last_numbers: code::LastNumbers::from_map(meta.last_numbers),
    }
}

/// The encoded rows last written to (or read from) the store, so a save
/// writes only what changed.
#[derive(Debug, Clone, Default)]
pub(super) struct Persisted {
    agents: HashMap<AgentId, Vec<u8>>,
    projects: HashMap<String, Vec<u8>>,
    meta: Option<Vec<u8>>,
}

/// Bytes `value` is compared by. Encoding these types cannot fail (no
/// maps with non-string keys); should it, the row counts as changed.
fn encoded<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

impl Persisted {
    /// What is stored once `rows` are.
    pub(super) fn of(rows: &Rows) -> Self {
        Self {
            agents: rows
                .agents
                .iter()
                .map(|agent| (agent.agent_id, encoded(agent)))
                .collect(),
            projects: rows
                .projects
                .iter()
                .map(|project| (project.id.clone(), encoded(project)))
                .collect(),
            meta: Some(encoded(&rows.meta)),
        }
    }

    /// Remember `rows`, written to the store by someone else (in a batch
    /// with rows of another store), as stored, so the next diff does not
    /// write them again.
    pub(super) fn mark_agents(&mut self, rows: &[AgentRecord]) {
        for row in rows {
            self.agents.insert(row.agent_id, encoded(row));
        }
    }

    /// The diff from what is stored to `rows`, and what is stored once it
    /// is applied. The caller replaces `self` with the latter only after
    /// the diff was applied, so a failed write is retried next time.
    pub(super) fn diff(&self, rows: Rows) -> (RegistryDiff, Self) {
        let next = Self::of(&rows);
        let agents_put = rows
            .agents
            .into_iter()
            .filter(|agent| self.agents.get(&agent.agent_id) != next.agents.get(&agent.agent_id))
            .collect();
        let live_agents: HashSet<&AgentId> = next.agents.keys().collect();
        let agents_del = self
            .agents
            .keys()
            .filter(|id| !live_agents.contains(id))
            .copied()
            .collect();
        let projects_put = rows
            .projects
            .into_iter()
            .filter(|project| self.projects.get(&project.id) != next.projects.get(&project.id))
            .collect();
        let projects_del = self
            .projects
            .keys()
            .filter(|id| !next.projects.contains_key(*id))
            .cloned()
            .collect();
        let meta = (self.meta != next.meta).then_some(rows.meta);
        let diff = RegistryDiff {
            agents_put,
            agents_del,
            projects_put,
            projects_del,
            meta,
        };
        (diff, next)
    }
}

impl AgentRegistry {
    /// Write the rows that changed since the last write, in one batch.
    /// What is remembered as stored moves on only once the batch landed,
    /// so a failed write is retried by the next one.
    pub(super) fn persist(&mut self) -> Result<(), AgentRegistryError> {
        self.persist_with(false)
    }

    /// [`Self::persist`], fsyncing the batch before returning whatever
    /// the store's `sync_writes` setting, for a write another durable step
    /// depends on.
    pub(super) fn persist_durable(&mut self) -> Result<(), AgentRegistryError> {
        self.persist_with(true)
    }

    fn persist_with(&mut self, durable: bool) -> Result<(), AgentRegistryError> {
        let (diff, next) = self.persisted.diff(to_rows(&self.file));
        if diff.is_empty() {
            return Ok(());
        }
        let written = if durable {
            self.store.apply_durable(&diff)
        } else {
            self.store.apply(&diff)
        };
        written.map_err(|source| AgentRegistryError::Save {
            data_dir: self.data_dir.clone(),
            agents: diff.agents_put.len() + diff.agents_del.len(),
            projects: diff.projects_put.len() + diff.projects_del.len(),
            source,
        })?;
        self.persisted = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(zns_id: &str, parent: Option<&str>) -> RegisteredAgent {
        RegisteredAgent {
            zns_id: zns_id.to_string(),
            parent: parent.map(str::to_string),
            ..super::super::AgentRegistry::new_agent("A".to_string())
        }
    }

    fn file(agents: Vec<RegisteredAgent>, active: Option<&str>) -> RegistryFile {
        RegistryFile {
            version: 3,
            active: active.map(str::to_string),
            agents,
            ..RegistryFile::default()
        }
    }

    fn snapshot(rows: &Rows) -> RegistrySnapshot {
        RegistrySnapshot {
            agents: rows.agents.clone(),
            projects: rows.projects.clone(),
            meta: Some(rows.meta.clone()),
        }
    }

    #[test]
    fn parents_are_stored_as_ids_and_read_back_as_zns_ids() {
        let file = file(
            vec![agent("0://a", None), agent("0://b", Some("0://a"))],
            Some("0://b"),
        );
        let rows = to_rows(&file);
        assert_eq!(rows.agents[1].parent, Some(derive_agent_id("0://a")));
        assert_eq!(rows.meta.active, Some(derive_agent_id("0://b")));
        assert_eq!(rows.agents[1].position, POSITION_STEP);
        let appended = agent("0://c", Some("0://b"));
        let row = appended_row(&file, &appended);
        assert_eq!(row.position, 2 * POSITION_STEP);
        assert_eq!(row.parent, Some(derive_agent_id("0://b")));
        let back = from_snapshot(snapshot(&rows));
        assert_eq!(back.agents[1].parent.as_deref(), Some("0://a"));
        assert_eq!(back.active.as_deref(), Some("0://b"));
    }

    #[test]
    fn a_dangling_parent_stays_set_and_round_trips() {
        let file = file(vec![agent("0://b", Some("0://gone"))], Some("0://gone"));
        let rows = to_rows(&file);
        let gone = derive_agent_id("0://gone");
        assert_eq!(rows.agents[0].parent, Some(gone));
        let back = from_snapshot(snapshot(&rows));
        let placeholder = format!("{PLACEHOLDER_PREFIX}{}", gone.to_hex());
        assert_eq!(back.agents[0].parent.as_deref(), Some(placeholder.as_str()));
        assert_eq!(back.active, None);
        assert_eq!(to_rows(&back).agents, rows.agents);
    }

    #[test]
    fn diff_writes_only_changed_rows_and_deletes_dropped_ones() {
        let mut file = file(vec![agent("0://a", None), agent("0://b", None)], None);
        let stored = Persisted::of(&to_rows(&file));
        let (diff, _) = stored.diff(to_rows(&file));
        assert!(diff.is_empty());

        file.agents[0].title = Some("t".to_string());
        file.agents.remove(1);
        let (diff, next) = stored.diff(to_rows(&file));
        assert_eq!(diff.agents_put.len(), 1);
        assert_eq!(diff.agents_del, vec![derive_agent_id("0://b")]);
        assert!(diff.meta.is_none());
        assert!(next.diff(to_rows(&file)).0.is_empty());
    }
}
