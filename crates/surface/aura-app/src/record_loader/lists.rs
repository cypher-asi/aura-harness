//! Plan paths, spawn archiving and agent/project list rows.

use super::*;

/// Path of the plan `agent_id` last wrote with `create_plan` or
/// `todo_write`, from its recent record.
#[must_use]
pub fn latest_plan_path(store: &dyn ReadStore, agent_id: AgentId) -> Option<String> {
    let entries = scan_tail(store, agent_id, HISTORY_SCAN_LIMIT)
        .inspect_err(|e| warn!(error = %e, "Failed to scan record for plans"))
        .ok()?;
    latest_plan_path_in(&entries)
}

/// [`latest_plan_path`] from `entries`, the newest of the record, oldest
/// first.
#[must_use]
pub fn latest_plan_path_in(entries: &[RecordEntry]) -> Option<String> {
    entries
        .iter()
        .rev()
        .filter(|entry| entry.tx.tx_type == TransactionType::ToolExecution)
        .filter_map(tool_execution)
        .filter(|execution| {
            !execution.is_error
                && execution.decision == ToolGateVerdict::Approved
                && matches!(
                    execution.tool.as_str(),
                    CREATE_PLAN_TOOL_NAME | TODO_WRITE_TOOL_NAME
                )
        })
        .find_map(|execution| {
            execution
                .result
                .as_deref()
                .and_then(plan_path_from_output)
                .map(str::to_string)
        })
}

/// Archive spawns from an earlier run of the app that never got to record
/// anything: the app closed or was killed while a `task` call was still
/// being dispatched, or while a writer waited for the workspace. Nothing
/// was recorded for them, so there is no prompt or turn to resume; left
/// alone they sit in the Agents panel with a chat that stays empty.
///
/// Only agents a `task` call spawned (`parent` is set) are candidates: a
/// user-created project or agent legitimately has an empty record before
/// its first message. Run it before reading [`AgentRegistry::active`] at
/// startup, which skips archived agents, so the app never opens on one it
/// just archived. Returns the agents archived.
pub fn archive_unstarted_spawns(
    registry: &mut AgentRegistry,
    store: &dyn ReadStore,
) -> Vec<AgentId> {
    let candidates: Vec<AgentId> = registry
        .agents()
        .iter()
        .filter(|agent| !agent.archived && agent.parent.is_some())
        .map(crate::agent_registry::RegisteredAgent::agent_id)
        .filter(|&agent_id| matches!(store.get_head_seq(agent_id), Ok(0)))
        .collect();
    let mut archived = Vec::with_capacity(candidates.len());
    for agent_id in candidates {
        // Best-effort: say why before archiving, but archive either way.
        if let Err(e) = registry.set_title(agent_id, "Did not start before the app closed") {
            warn!(%agent_id, error = %e, "Could not note an unstarted spawn");
        }
        match registry.archive(agent_id) {
            Ok(()) => {
                tracing::info!(%agent_id, "Archived a spawn an earlier run never started");
                archived.push(agent_id);
            }
            Err(e) => warn!(%agent_id, error = %e, "Could not archive an unstarted spawn"),
        }
    }
    archived
}

/// Rows of the agents column, the Archive and the projects list, with the
/// store not read yet: built under the registry lock, which
/// [`ListRows::read_activity`] does not need.
pub(crate) struct ListRows {
    pub(crate) projects: Vec<ProjectSummary>,
    pub(crate) listed: Vec<(AgentId, AgentSummary)>,
    pub(crate) archived: Vec<(AgentId, AgentSummary)>,
    /// Rows of channel members, copied (or, for hidden ones, taken) out of
    /// `listed`, and copied out of `archived`, by
    /// [`ListRows::split_members`] for the channels list
    pub(crate) members: Vec<(AgentId, AgentSummary)>,
    /// The agent the rows mark active
    pub(crate) active: AgentId,
}

impl ListRows {
    /// The rows of every list, in one walk of each. `running` holds the
    /// agents currently working on a task for their parent. `active`, the
    /// open agent, is marked active in whichever list it is in, since an
    /// archived agent can be opened without leaving the Archive.
    pub(crate) fn build(
        registry: &AgentRegistry,
        running: &HashSet<AgentId>,
        active: AgentId,
    ) -> Self {
        let mut names = ProjectNames::default();
        let listed = rows(registry, registry.tree(), active, running, &mut names);
        let archived = rows(
            registry,
            registry.archived_tree(),
            active,
            running,
            &mut names,
        );
        let count = |rows: &[(AgentId, AgentSummary)], project: &str| {
            rows.iter()
                .filter(|(_, row)| row.project_id.as_deref() == Some(project))
                .count()
        };
        let projects = registry
            .listed_projects()
            .into_iter()
            .map(|project| {
                project_summary(
                    project,
                    count(&listed, &project.id),
                    count(&archived, &project.id),
                )
            })
            .collect();
        Self {
            projects,
            listed,
            archived,
            members: Vec::new(),
            active,
        }
    }

    /// Copy the rows of the agents in `members` (channel members) into
    /// [`ListRows::members`], for the channels list, and move those also in
    /// `hidden` (members of channels without a project) out of the agents
    /// column, taking them off their project's listed count. Members of a
    /// channel with a project stay in the agents column under it. Rows of
    /// archived members are copied too, staying in the Archive: a channel
    /// still lists its coordinator when that is archived. A project's
    /// archived count is left as it is.
    ///
    /// Every project stays on the projects list, even one whose every
    /// listed row was hidden: any listed project is one a channel can be
    /// given, so clearing a channel's project (which puts its members,
    /// hidden, in the launch project) never takes a project off the list.
    pub(crate) fn split_members(&mut self, members: &HashSet<AgentId>, hidden: &HashSet<AgentId>) {
        if members.is_empty() && hidden.is_empty() {
            return;
        }
        let mut removed: HashMap<String, usize> = HashMap::new();
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(self.listed.len());
        for (agent_id, row) in std::mem::take(&mut self.listed) {
            if hidden.contains(&agent_id) {
                if let Some(project) = &row.project_id {
                    *removed.entry(project.clone()).or_default() += 1;
                }
                taken.push((agent_id, row));
            } else {
                if members.contains(&agent_id) {
                    taken.push((agent_id, row.clone()));
                }
                kept.push((agent_id, row));
            }
        }
        self.listed = kept;
        taken.extend(
            (self.archived.iter())
                .filter(|(agent_id, _)| members.contains(agent_id))
                .cloned(),
        );
        self.members = taken;
        for project in &mut self.projects {
            if let Some(&gone) = removed.get(&project.id) {
                project.listed_agents = project.listed_agents.saturating_sub(gone);
            }
        }
    }

    /// Fill in each row's record count and last activity from `store`,
    /// reusing what `cache` saw for agents with no new entries.
    pub(crate) fn read_activity(&mut self, store: &dyn ReadStore, cache: &mut LastActive) {
        for (agent_id, row) in self
            .listed
            .iter_mut()
            .chain(self.archived.iter_mut())
            .chain(self.members.iter_mut())
        {
            set_activity(row, cache.of(store, *agent_id));
        }
    }

    /// The commands that send the lists, in the order the UI takes them:
    /// the projects first, so one with every agent archived is known when
    /// the rows arrive.
    pub(crate) fn into_commands(self) -> [UiCommand; 3] {
        let strip = |rows: Vec<(AgentId, AgentSummary)>| -> Vec<AgentSummary> {
            rows.into_iter().map(|(_, row)| row).collect()
        };
        [
            UiCommand::SetProjects(self.projects),
            UiCommand::SetAgents(strip(self.listed)),
            UiCommand::SetArchivedAgents(strip(self.archived)),
        ]
    }
}

pub(super) fn set_activity(
    row: &mut AgentSummary,
    (record_count, last_active_ms): (u64, Option<u64>),
) {
    row.record_count = record_count;
    row.last_active_ms = last_active_ms;
    row.last_active = last_active_ms.map_or_else(|| "-".to_string(), format_local_time);
}

/// Agents whose reads a [`ByHead`] keeps.
pub(super) const READS_KEPT: usize = 8;

/// What was read from the records of the agents shown last, each kept
/// until its record grows: records are only appended to, so the same head
/// reads the same. Switching back to an agent then decodes nothing.
#[derive(Debug)]
pub struct ByHead<T>(pub(super) VecDeque<(AgentId, u64, T)>);

impl<T> Default for ByHead<T> {
    fn default() -> Self {
        Self(VecDeque::new())
    }
}

impl<T: Clone> ByHead<T> {
    /// What `read` gives for `agent_id`'s record as it is in `store`.
    ///
    /// # Errors
    /// Returns what `read` fails with; failures are not kept.
    pub fn get_or_read(
        &mut self,
        store: &dyn ReadStore,
        agent_id: AgentId,
        read: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let Ok(head) = store.get_head_seq(agent_id) else {
            return read();
        };
        if let Some(value) = self.kept(agent_id, head) {
            return Ok(value);
        }
        let value = read()?;
        self.keep(agent_id, head, value.clone());
        Ok(value)
    }

    /// What was read for `agent_id` when its record's head was `head`, if
    /// it is kept. A read at another head is dropped: it is out of date.
    pub fn kept(&mut self, agent_id: AgentId, head: u64) -> Option<T> {
        let at = self.0.iter().position(|(id, _, _)| *id == agent_id)?;
        let kept = self.0.remove(at).filter(|(_, seen, _)| *seen == head)?;
        let value = kept.2.clone();
        self.0.push_front(kept);
        Some(value)
    }

    /// Keep `value`, read from `agent_id`'s record at `head`.
    pub fn keep(&mut self, agent_id: AgentId, head: u64, value: T) {
        self.0.retain(|(id, _, _)| *id != agent_id);
        self.0.push_front((agent_id, head, value));
        self.0.truncate(READS_KEPT);
    }
}

impl<T: Clone + Send + 'static> ByHead<T> {
    /// [`Self::get_or_read`] for the event loop: a read that is not kept
    /// runs on a blocking thread, so the runtime's worker goes on with the
    /// agents' turns while the record is scanned. `head` is read first, so
    /// a record that grows meanwhile is read again next time.
    ///
    /// # Errors
    /// Returns what `read` fails with; failures are not kept.
    pub async fn get_or_read_off_loop(
        cache: &Mutex<Self>,
        store: &Arc<dyn ReadStore>,
        agent_id: AgentId,
        read: fn(&dyn ReadStore, AgentId) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let lock = || cache.lock().unwrap_or_else(PoisonError::into_inner);
        let head = store.get_head_seq(agent_id).ok();
        if let Some(value) = head.and_then(|head| lock().kept(agent_id, head)) {
            return Ok(value);
        }
        let reader = Arc::clone(store);
        let value = tokio::task::spawn_blocking(move || read(reader.as_ref(), agent_id))
            .await
            .map_err(|e| StoreError::Backend(Box::new(e)))??;
        if let Some(head) = head {
            lock().keep(agent_id, head, value.clone());
        }
        Ok(value)
    }
}

/// When each agent was last active, by the head of its record: the latest
/// entry, which is decoded whole, is only read again once the head moves.
#[derive(Debug, Default)]
pub struct LastActive(pub(super) HashMap<AgentId, (u64, Option<u64>)>);

impl LastActive {
    /// `agent_id`'s record count and the time of its latest entry.
    pub fn of(&mut self, store: &dyn ReadStore, agent_id: AgentId) -> (u64, Option<u64>) {
        let head = store.get_head_seq(agent_id).unwrap_or(0);
        if head == 0 {
            self.0.remove(&agent_id);
            return (0, None);
        }
        if let Some(&(seen, ts)) = self.0.get(&agent_id) {
            if seen == head {
                return (head, ts);
            }
        }
        let ts = store
            .get_record_entry(agent_id, head)
            .ok()
            .map(|entry| entry.tx.ts_ms);
        self.0.insert(agent_id, (head, ts));
        (head, ts)
    }
}

/// Build the agents-column rows for every registered agent in the order of
/// [`AgentRegistry::tree`]. `running` holds the agents currently working
/// on a task for their parent.
#[must_use]
pub fn agent_summaries(
    registry: &AgentRegistry,
    store: &dyn ReadStore,
    active: AgentId,
    running: &HashSet<AgentId>,
) -> Vec<AgentSummary> {
    let mut names = ProjectNames::default();
    let rows = rows(registry, registry.tree(), active, running, &mut names);
    with_activity(rows, store)
}

/// Rows for the Archive. Each archived agent's `project` names the project
/// it was in, so the row can say where it came from. The open agent's row
/// is marked active.
#[must_use]
pub fn archived_summaries(registry: &AgentRegistry, store: &dyn ReadStore) -> Vec<AgentSummary> {
    let none = HashSet::new();
    let active = registry.current().agent_id();
    let mut names = ProjectNames::default();
    let rows = rows(
        registry,
        registry.archived_tree(),
        active,
        &none,
        &mut names,
    );
    with_activity(rows, store)
}

pub(super) fn with_activity(
    rows: Vec<(AgentId, AgentSummary)>,
    store: &dyn ReadStore,
) -> Vec<AgentSummary> {
    let mut cache = LastActive::default();
    rows.into_iter()
        .map(|(agent_id, mut row)| {
            set_activity(&mut row, cache.of(store, agent_id));
            row
        })
        .collect()
}

/// Rows of the listed projects, in list order.
#[must_use]
pub fn project_summaries(registry: &AgentRegistry) -> Vec<ProjectSummary> {
    let counts = registry.project_counts_all();
    registry
        .listed_projects()
        .into_iter()
        .map(|project| {
            let (listed, archived) = counts.get(&project.id).copied().unwrap_or_default();
            project_summary(project, listed, archived)
        })
        .collect()
}

pub(super) fn project_summary(
    project: &crate::agent_registry::Project,
    listed_agents: usize,
    archived_agents: usize,
) -> ProjectSummary {
    ProjectSummary {
        id: project.id.clone(),
        name: project.name(),
        folder: project.folder.display().to_string(),
        folder_exists: project.folder.is_dir(),
        created: project
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        listed_agents,
        archived_agents,
    }
}

/// Names of projects by id, each worked out once per list.
#[derive(Default)]
pub(super) struct ProjectNames(HashMap<String, Option<String>>);

impl ProjectNames {
    fn of(&mut self, registry: &AgentRegistry, project_id: Option<&str>) -> Option<String> {
        let id = project_id?;
        if let Some(name) = self.0.get(id) {
            return name.clone();
        }
        let name = registry
            .project(id)
            .map(crate::agent_registry::Project::name);
        self.0.insert(id.to_string(), name.clone());
        name
    }
}

/// Rows for `tree`, a walk of [`AgentRegistry::tree`] or
/// [`AgentRegistry::archived_tree`], without the store's fields.
///
/// A row below the top has as parent the nearest row above it one level
/// up, so whether a `/compare` holds it carries down the walk instead of
/// being looked up for each agent; rows at the top look it up.
pub(super) fn rows(
    registry: &AgentRegistry,
    tree: Vec<(&crate::agent_registry::RegisteredAgent, usize)>,
    active: AgentId,
    running: &HashSet<AgentId>,
    names: &mut ProjectNames,
) -> Vec<(AgentId, AgentSummary)> {
    // Per depth: whether the agents below the row there are under a
    // comparison.
    let mut below_compare: Vec<bool> = Vec::new();
    tree.into_iter()
        .map(|(agent, depth)| {
            let agent_id = agent.agent_id();
            let above = match depth.checked_sub(1) {
                Some(up) => below_compare.get(up).copied().unwrap_or(false),
                None => registry
                    .ancestry(agent_id)
                    .into_iter()
                    .skip(1)
                    .any(|id| registry.find(id).is_some_and(|a| a.is_compare())),
            };
            below_compare.truncate(depth);
            below_compare.push(above || agent.is_compare());
            let row = AgentSummary {
                id: agent_id.to_hex(),
                name: agent.display_name().into_owned(),
                zns_id: agent.zns_id.clone(),
                is_active: agent_id == active,
                record_count: 0,
                last_active: "-".to_string(),
                last_active_ms: None,
                archived_at_ms: agent
                    .archived_at
                    .and_then(|at| u64::try_from(at.timestamp_millis()).ok()),
                definition: agent.definition.clone(),
                depth,
                running: running.contains(&agent_id),
                bench_run: above && !agent.is_compare_arm(),
                project_id: agent.project_id.clone(),
                project: names.of(registry, agent.project_id.as_deref()),
                // The registry does not know modes; the hub fills this in.
                plan_mode: false,
                code: agent.code.clone(),
                subtitle: agent.subtitle.clone(),
                title: agent.title.clone(),
                // The registry does not know how work went; the hub does.
                error: None,
                starting: false,
                waiting: false,
                reconnecting: false,
            };
            (agent_id, row)
        })
        .collect()
}
