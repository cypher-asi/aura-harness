//! Persistent list of the projects and agents the terminal UI manages.
//!
//! Stored in the `agents`, `projects` and `registry_meta` column families
//! of the agent-state database at `<data_dir>/db` (see
//! [`aura_store_db::agents`]), one row per agent and per project, so a
//! change writes only the rows it touched, in one batch. List order is
//! each row's `position`; parent links and the active agent are stored as
//! [`AgentId`]s (in memory they are ZNS ids; see `persist`).
//!
//! `<data_dir>/agents.json`, where the registry used to live, is read
//! only once: to import it when the store holds no registry yet. The
//! legacy single-agent `agent_identity.txt` is folded in as the first
//! entry so existing records stay attached to the agent that wrote them.
//!
//! A project is one folder: no two projects share one, and a project is
//! called by its folder's name. Every agent belongs to a project. The
//! order of `projects` is the order the projects are listed in, and the
//! order of a project's top-level agents in `agents` is theirs.
//! Archiving or removing never moves an entry, so whatever comes back
//! comes back in its old place.

use anyhow::Context;
use aura_core_types::{AgentId, Identity};
use aura_store_db::{AgentStore, RocksStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

const REGISTRY_FILE: &str = "agents.json";
/// What [`REGISTRY_FILE`] is renamed to once imported into the store.
const MIGRATED_REGISTRY_FILE: &str = "agents.json.migrated";
/// Directory of the agent-state database under the data directory.
const DB_DIR: &str = "db";
const LEGACY_IDENTITY_FILE: &str = "agent_identity.txt";
/// Where the desktop app saved which projects each tab held, by the ids
/// of the agents that headed them before projects had ids of their own.
const DESKTOP_TABS_FILE: &str = "desktop_tabs";
/// Registries written before projects were their own entries are
/// version 0. From version 3 a project with every agent archived stays
/// listed.
const REGISTRY_VERSION: u32 = 3;
const DEFAULT_AGENT_NAME: &str = "Terminal Agent";
/// Name of the agent created with a new project.
const DEFAULT_PROJECT_AGENT: &str = "Agent";
/// ZNS id of the agent `aura -p` runs as. Fixed, so every headless run in
/// a data directory finds the same agent and its saved sessions.
const HEADLESS_ZNS_ID: &str = "0://terminal/headless";
const HEADLESS_AGENT_NAME: &str = "Headless";
/// ZNS id prefix of the agents that hold a `/compare` and its runs.
const COMPARE_ZNS_PREFIX: &str = "0://terminal/compare/";
/// ZNS id prefix of the agents that group one arm's runs of a `/compare`.
const COMPARE_ARM_ZNS_PREFIX: &str = "0://terminal/compare-arm/";

/// A folder the agents of the terminal UI work in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    /// Canonical path; no other project has it.
    pub folder: PathBuf,
    pub created_at: DateTime<Utc>,
    /// Taken off the list. Kept while agents of it sit in the Archive, so
    /// restoring one of them puts the same project back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
}

impl Project {
    /// What the UI calls the project: its folder's name.
    #[must_use]
    pub fn name(&self) -> String {
        self.folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.folder.display().to_string())
    }
}

/// How [`AgentRegistry::open_project`] found a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// It was no project yet; now it is, with a new agent.
    Created,
    /// It is a listed project already; nothing was added.
    Existing,
    /// It was a project off the list; it is back, with a new agent.
    Revived,
}

/// [`RegisteredAgent::agent_id`], worked out once. Deriving an id hashes
/// the ZNS id, and the agent lists look ids up for every agent on every
/// change, so recomputing them made those lists quadratic in cost.
///
/// A clone starts empty, so it never carries an id its `zns_id` might
/// not match; equality ignores it.
#[derive(Default)]
struct IdCache(std::sync::OnceLock<AgentId>);

impl Clone for IdCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for IdCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for IdCache {}

impl std::fmt::Debug for IdCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdCache")
    }
}

/// One agent known to the terminal UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredAgent {
    /// Never changed once the agent is registered: its id is derived from
    /// it and cached.
    pub zns_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// Permanent agent this is an instance of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// ZNS id of the agent that spawned this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Id of the project the agent is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Folder this agent works in instead of its project's, for it and the
    /// agents it spawns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    /// Moved to the Archive: hidden from the projects list, together with
    /// the agents it spawned, until restored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// Taken off the list with its project, but kept because agents it
    /// spawned sit in the Archive. Restoring one of them brings it back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    /// Short title of what the agent's chat is about, written by the model
    /// after its first turn or set explicitly. Shown instead of `name`
    /// (after the code for a spawned instance, in place of its subtitle;
    /// see [`Self::display_name`]), which stays the agent's identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Short code name shown beside the agent in the UI, `<PREFIX><N>`
    /// such as `RE36` for `rust-engineer #36`. Unique in the registry and
    /// never changed once assigned: the registry gives one to every agent
    /// it registers, and to agents of older registries when it loads them
    /// (older `RE-36` codes become `RE36`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Short name of the subtask a spawned permanent-agent instance was
    /// given ("Fix parser escapes"), written by the title model from the
    /// task prompt. Shown after the code (`RE36: Fix parser escapes`)
    /// unless the agent has a title; `name` stays the agent's identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Version 0: ZNS id of the agent heading the project this one was
    /// added to.
    #[serde(default, rename = "member_of", skip_serializing)]
    legacy_member_of: Option<String>,
    #[serde(skip)]
    id: IdCache,
}

impl RegisteredAgent {
    #[must_use]
    pub fn identity(&self) -> Identity {
        Identity::new(&self.zns_id, &self.name)
    }

    /// The id [`Self::identity`] has, without building the identity.
    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        *self
            .id
            .0
            .get_or_init(|| AgentId::new(*blake3::hash(self.zns_id.as_bytes()).as_bytes()))
    }

    /// Whether this agent holds a `/compare` (its record and runs).
    #[must_use]
    pub fn is_compare(&self) -> bool {
        self.zns_id.starts_with(COMPARE_ZNS_PREFIX)
    }

    /// Whether this agent groups the runs of one arm of a `/compare`.
    #[must_use]
    pub fn is_compare_arm(&self) -> bool {
        self.zns_id.starts_with(COMPARE_ARM_ZNS_PREFIX)
    }

    /// What the UI calls the agent. A spawned instance, or any agent with
    /// a subtitle, is labelled by its code and what it is doing:
    /// `<code>: <title>` once it has a title (an explicit title says more
    /// than the subtitle written from its task, so it wins), else
    /// `<code>: <subtitle>`, without the code when it has none. Other
    /// agents show their title once they have one. Without either, the
    /// name. The name stays the agent's identity (see [`Self::identity`]).
    #[must_use]
    pub fn display_name(&self) -> Cow<'_, str> {
        let label = self.title.as_deref().or(self.subtitle.as_deref());
        let coded = self.is_spawned_instance() || self.subtitle.is_some();
        match (label, self.code.as_deref()) {
            (Some(label), Some(code)) if coded => Cow::Owned(format!("{code}: {label}")),
            (Some(label), _) => Cow::Borrowed(label),
            (None, _) => Cow::Borrowed(&self.name),
        }
    }

    /// Whether another agent spawned this instance of a permanent agent.
    fn is_spawned_instance(&self) -> bool {
        self.definition.is_some() && self.parent.is_some()
    }

    /// Whether the agent should be named after its subtask: it is an
    /// instance of a permanent agent that another agent spawned, and has
    /// neither a subtitle nor a title yet. A title set first (by hand, or
    /// when an unstarted spawn is archived) is not overridden by a
    /// subtitle the title model writes later.
    #[must_use]
    pub fn needs_subtitle(&self) -> bool {
        self.is_spawned_instance() && self.subtitle.is_none() && self.title.is_none()
    }

    /// Whether the agent should get a title: it has none yet and still has
    /// the placeholder name it was created with. Names the user picked and
    /// permanent-agent instances are kept.
    #[must_use]
    pub fn needs_title(&self) -> bool {
        self.title.is_none() && self.definition.is_none() && is_placeholder_name(&self.name)
    }
}

/// Names the registry gives agents nobody named: [`DEFAULT_AGENT_NAME`],
/// [`DEFAULT_PROJECT_AGENT`] and `Agent N`.
fn is_placeholder_name(name: &str) -> bool {
    if name == DEFAULT_AGENT_NAME || name == DEFAULT_PROJECT_AGENT {
        return true;
    }
    name.strip_prefix(DEFAULT_PROJECT_AGENT)
        .and_then(|rest| rest.strip_prefix(' '))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Where a `/compare` run named `<arm>-<n>` goes among its comparison's
/// runs: by run number, then arm, with other names after them.
fn run_order(name: &str) -> (bool, Option<(u32, &str)>) {
    let order = name
        .rsplit_once('-')
        .filter(|(arm, _)| !arm.is_empty() && arm.bytes().all(|b| b.is_ascii_lowercase()))
        .and_then(|(arm, n)| Some((n.parse().ok()?, arm)));
    (order.is_none(), order)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    version: u32,
    /// ZNS id of the agent that was active when the TUI last switched.
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    projects: Vec<Project>,
    #[serde(default)]
    agents: Vec<RegisteredAgent>,
    /// The code prefix each source key (definition name, or `agent`) has
    /// reserved, so definitions without instances have one too.
    #[serde(default, skip_serializing_if = "code::PrefixTable::is_empty")]
    prefixes: code::PrefixTable,
    /// The highest code number each prefix has ever been given, so the
    /// codes and instance names of dropped agents are not reused.
    #[serde(default, skip_serializing_if = "code::LastNumbers::is_empty")]
    last_numbers: code::LastNumbers,
}

/// Agent registry backed by the agent-state database.
pub struct AgentRegistry {
    store: AgentStore,
    /// The rows `store` holds, as last written or read.
    persisted: persist::Persisted,
    /// Data directory the registry belongs to, for log context.
    data_dir: PathBuf,
    /// Folder of the project agents go in when nothing else says which.
    launch_dir: PathBuf,
    file: RegistryFile,
    /// The database handle [`Self::load_or_init_in`] opened, held so the
    /// process-wide cache hands the same one to the next registry on it.
    _keep_open: Option<Arc<RocksStore>>,
}

impl std::fmt::Debug for AgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRegistry")
            .field("store", &self.store)
            .field("data_dir", &self.data_dir)
            .field("launch_dir", &self.launch_dir)
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

/// Database handles [`AgentRegistry::load_or_init_in`] opened, by path,
/// so a second registry on a data directory shares the live handle
/// instead of failing on the database lock.
static OPEN: Mutex<Option<HashMap<PathBuf, Weak<RocksStore>>>> = Mutex::new(None);

/// The live handle on the database at `path`, opened when there is none.
fn shared_store(path: &Path) -> anyhow::Result<Arc<RocksStore>> {
    let mut open = OPEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let open = open.get_or_insert_with(HashMap::new);
    open.retain(|_, store| store.strong_count() > 0);
    if let Some(store) = open.get(path).and_then(Weak::upgrade) {
        return Ok(store);
    }
    let store = Arc::new(
        RocksStore::open(path, false)
            .with_context(|| format!("opening agent registry database {}", path.display()))?,
    );
    open.insert(path.to_path_buf(), Arc::downgrade(&store));
    Ok(store)
}

/// `folder` made canonical when it exists, else as given.
fn canonical(folder: &Path) -> PathBuf {
    folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf())
}

/// Put the entries `order` names into the slots `slots` of `items` that
/// hold them, in `order`'s order. Entries it leaves out keep their
/// relative order after the ones it names; other slots are untouched.
fn reorder_slots<T: Clone>(
    items: &mut [T],
    slots: &[usize],
    order: &[String],
    key: impl Fn(&T) -> String,
) -> bool {
    let held: Vec<String> = slots.iter().map(|&slot| key(&items[slot])).collect();
    let mut wanted: Vec<String> = Vec::with_capacity(held.len());
    for id in order {
        if held.contains(id) && !wanted.contains(id) {
            wanted.push(id.clone());
        }
    }
    for id in &held {
        if !wanted.contains(id) {
            wanted.push(id.clone());
        }
    }
    if wanted == held {
        return false;
    }
    let taken: Vec<T> = slots.iter().map(|&slot| items[slot].clone()).collect();
    for (&slot, id) in slots.iter().zip(&wanted) {
        if let Some(item) = taken.iter().find(|item| key(item) == *id) {
            items[slot] = item.clone();
        }
    }
    true
}

impl AgentRegistry {
    /// [`Self::load_or_init_in`] with the process's working directory as
    /// the launch directory.
    ///
    /// # Errors
    /// As [`Self::load_or_init_in`].
    pub fn load_or_init(data_dir: &Path) -> anyhow::Result<Self> {
        let launch_dir = std::env::current_dir().context("resolving working directory")?;
        Self::load_or_init_in(data_dir, &launch_dir)
    }

    /// [`Self::open`] on the database at `<data_dir>/db`, opened here (or
    /// shared with another registry on it in this process). For tests and
    /// benches; the app opens the database itself and calls
    /// [`Self::open`].
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened, or as
    /// [`Self::open`].
    #[doc(hidden)]
    pub fn load_or_init_in(data_dir: &Path, launch_dir: &Path) -> anyhow::Result<Self> {
        let rocks = shared_store(&data_dir.join(DB_DIR))?;
        let mut registry = Self::open(AgentStore::for_store(&rocks), data_dir, launch_dir)?;
        registry._keep_open = Some(rocks);
        Ok(registry)
    }

    /// Load the registry from `store`, importing `<data_dir>/agents.json`
    /// once when the store holds no registry yet, seeding it from the
    /// legacy identity file (or a fresh identity) when there is neither,
    /// and bringing an older registry up to date. Agents nothing places
    /// go in the project for `launch_dir`.
    ///
    /// # Errors
    /// Returns an error if the store, registry or identity file cannot be
    /// read, parsed, or written.
    pub fn open(store: AgentStore, data_dir: &Path, launch_dir: &Path) -> anyhow::Result<Self> {
        let imported = store
            .is_empty()
            .with_context(|| format!("reading agent registry in {}", data_dir.display()))?;
        let (file, persisted) = if imported {
            (read_registry_file(data_dir)?, persist::Persisted::default())
        } else {
            let snapshot = store
                .load()
                .with_context(|| format!("loading agent registry in {}", data_dir.display()))?;
            let file = persist::from_snapshot(snapshot);
            let persisted = persist::Persisted::of(&persist::to_rows(&file));
            (file, persisted)
        };

        let mut registry = Self {
            store,
            persisted,
            data_dir: data_dir.to_path_buf(),
            launch_dir: canonical(launch_dir),
            file,
            _keep_open: None,
        };
        let mut changed = false;
        if registry.file.version < 2 {
            registry.migrate(data_dir);
        }
        if registry.file.version < 3 {
            registry.hide_empty_projects();
        }
        if registry.file.version < REGISTRY_VERSION {
            registry.file.version = REGISTRY_VERSION;
            changed = true;
        }
        if registry.file.agents.is_empty() {
            let zns_id = legacy_or_new_zns_id(data_dir)?;
            let project_id = registry.launch_project();
            registry.file.agents.push(RegisteredAgent {
                zns_id,
                project_id: Some(project_id),
                ..Self::new_agent(DEFAULT_AGENT_NAME.to_string())
            });
            changed = true;
        }
        let migrated = code::migrate(&mut registry.file.agents, &mut registry.file.prefixes);
        let reserved = registry
            .file
            .prefixes
            .fill_from_codes(&registry.file.agents);
        let seeded = registry
            .file
            .last_numbers
            .raise_to_codes(&registry.file.agents);
        let coded = registry.assign_missing_codes();
        if changed || imported {
            registry.persist()?;
            if imported {
                retire_registry_file(data_dir);
            }
        } else if migrated || coded || reserved || seeded {
            // Codes alone are no reason to refuse to start, say on a
            // read-only data directory: the next load assigns the same
            // ones again, as backfilling is deterministic.
            if let Err(error) = registry.persist() {
                tracing::warn!(
                    data_dir = %registry.data_dir.display(),
                    error = %format!("{error:#}"),
                    "could not save the agents' new codes"
                );
            }
        }
        Ok(registry)
    }

    /// Give every agent without a code one, in registry order, so the same
    /// registry always gets the same codes (see [`code::backfill`]).
    /// Returns whether any agent got one.
    fn assign_missing_codes(&mut self) -> bool {
        let mut changed = false;
        for index in 0..self.file.agents.len() {
            if self.file.agents[index].code.is_some() {
                continue;
            }
            let assigned = code::backfill(
                &self.file.agents[index],
                &self.file.agents,
                &mut self.file.prefixes,
                &mut self.file.last_numbers,
            );
            self.file.agents[index].code = Some(assigned);
            changed = true;
        }
        changed
    }

    /// The code prefix (the abbreviation, such as `RE` for
    /// `rust-engineer`) of each of `keys`, in the same order. A key is a
    /// definition name, or `agent` for agents without one.
    ///
    /// Keys without a prefix yet reserve one now, in the order given, so
    /// the same registry and keys always give the same prefixes. A
    /// reserved prefix is the one the key's instances' codes get, is never
    /// changed, and no other key has it. The registry is saved only when
    /// something was reserved.
    ///
    /// # Errors
    /// Returns an error if a new reservation cannot be saved. The
    /// reservations stay in memory regardless.
    pub fn reserve_prefixes(&mut self, keys: &[&str]) -> anyhow::Result<Vec<String>> {
        let before = self.file.prefixes.len();
        let prefixes = keys
            .iter()
            .map(|key| self.file.prefixes.prefix_for(key, &self.file.agents))
            .collect();
        if self.file.prefixes.len() != before {
            self.persist()?;
        }
        Ok(prefixes)
    }

    /// Turn a version 0 registry, where the agent heading a project held
    /// its folder and the agents added to it pointed at it, into projects
    /// of their own. A project takes the hex id of the agent that headed
    /// it, so what saved that id still finds it. Projects in one folder
    /// become one. Projects are ordered as the desktop app's tabs had
    /// them, then as the registry listed them.
    fn migrate(&mut self, data_dir: &Path) {
        let agents = &mut self.file.agents;
        // Registries written before `member_of` existed linked agents added
        // to a project to its root through `parent`. Only those agents have
        // a parent and no definition: spawned agents are always instances.
        for agent in agents.iter_mut() {
            if agent.definition.is_none() && agent.legacy_member_of.is_none() {
                if let Some(root) = agent.parent.take() {
                    agent.legacy_member_of = Some(root);
                }
            }
        }
        let up = |agents: &[RegisteredAgent], index: usize| {
            let agent = &agents[index];
            let above = agent
                .parent
                .as_deref()
                .or(agent.legacy_member_of.as_deref())?;
            agents.iter().position(|a| a.zns_id == above)
        };
        let root_of = |agents: &[RegisteredAgent], mut index: usize| {
            for _ in 0..agents.len() {
                match up(agents, index) {
                    Some(above) => index = above,
                    None => break,
                }
            }
            index
        };
        let mut roots: Vec<usize> = (0..agents.len())
            .filter(|&i| up(agents, i).is_none())
            .collect();
        let tabs = fs::read_to_string(data_dir.join(DESKTOP_TABS_FILE)).unwrap_or_default();
        let saved: Vec<&str> = tabs.split_whitespace().collect();
        roots.sort_by_key(|&i| {
            let hex = agents[i].agent_id().to_hex();
            saved
                .iter()
                .position(|id| *id == hex)
                .unwrap_or(saved.len())
        });

        let mut projects: Vec<Project> = self.file.projects.drain(..).collect();
        let mut project_of_root: Vec<(usize, String)> = Vec::with_capacity(roots.len());
        for &root in &roots {
            let agent = &agents[root];
            let folder = agent
                .folder
                .as_deref()
                .map_or_else(|| self.launch_dir.clone(), canonical);
            let id = match projects.iter_mut().find(|p| p.folder == folder) {
                Some(project) => {
                    project.removed &= agent.removed;
                    project.id.clone()
                }
                None => {
                    let id = agent.agent_id().to_hex();
                    projects.push(Project {
                        id: id.clone(),
                        folder,
                        created_at: agent.created_at,
                        removed: agent.removed,
                    });
                    id
                }
            };
            project_of_root.push((root, id));
        }
        let assigned: Vec<Option<String>> = (0..agents.len())
            .map(|i| {
                let root = root_of(agents, i);
                project_of_root
                    .iter()
                    .find(|(r, _)| *r == root)
                    .map(|(_, id)| id.clone())
            })
            .collect();
        for (index, project_id) in assigned.into_iter().enumerate() {
            let agent = &mut agents[index];
            if agent.project_id.is_none() {
                agent.project_id = project_id;
            }
            if roots.contains(&index) {
                // The folder is the project's now.
                agent.folder = None;
            }
            agent.legacy_member_of = None;
        }
        self.file.projects = projects;
    }

    /// Before version 3 a project showed only while it had an agent
    /// listed. Take those with none off the list, as removed, so the list
    /// looks as it did; restoring one of their agents brings them back.
    fn hide_empty_projects(&mut self) {
        let shown: Vec<String> = self
            .tree()
            .iter()
            .filter_map(|(agent, _)| agent.project_id.clone())
            .collect();
        for project in &mut self.file.projects {
            if !shown.contains(&project.id) {
                project.removed = true;
            }
        }
    }

    #[must_use]
    pub fn agents(&self) -> &[RegisteredAgent] {
        &self.file.agents
    }

    /// Every project, listed or not, in list order.
    #[must_use]
    pub fn projects(&self) -> &[Project] {
        &self.file.projects
    }

    #[must_use]
    pub fn find(&self, agent_id: AgentId) -> Option<&RegisteredAgent> {
        self.file.agents.iter().find(|a| a.agent_id() == agent_id)
    }

    /// The project with id `id`.
    #[must_use]
    pub fn project(&self, id: &str) -> Option<&Project> {
        self.file.projects.iter().find(|p| p.id == id)
    }

    /// The project `agent_id` is in.
    #[must_use]
    pub fn project_of(&self, agent_id: AgentId) -> Option<&Project> {
        self.project(self.find(agent_id)?.project_id.as_deref()?)
    }

    /// The agent to open on startup: the last active one, else the first
    /// listed one. Archived agents are not opened while another is listed.
    #[must_use]
    pub fn active(&self) -> &RegisteredAgent {
        let stored = self
            .file
            .active
            .as_deref()
            .and_then(|zns| self.file.agents.iter().find(|a| a.zns_id == zns));
        stored
            .filter(|agent| self.is_listed(agent))
            .or_else(|| self.tree().first().map(|(agent, _)| *agent))
            .or(stored)
            .unwrap_or(&self.file.agents[0])
    }

    /// Remember `agent_id` as the active agent.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn set_active(&mut self, agent_id: AgentId) -> anyhow::Result<()> {
        if let Some(agent) = self.find(agent_id) {
            if self.file.active.as_deref() == Some(agent.zns_id.as_str()) {
                return Ok(());
            }
            self.file.active = Some(agent.zns_id.clone());
            self.persist()?;
        }
        Ok(())
    }

    /// Id of the project for the launch directory, made (or put back on
    /// the list) when needed. Not saved.
    fn launch_project(&mut self) -> String {
        let folder = self.launch_dir.clone();
        self.project_for(&folder)
    }

    /// Id of the project for the canonical folder `folder`, made (or put
    /// back on the list) when needed. Not saved.
    fn project_for(&mut self, folder: &Path) -> String {
        if let Some(project) = self.file.projects.iter_mut().find(|p| p.folder == folder) {
            project.removed = false;
            return project.id.clone();
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.file.projects.push(Project {
            id: id.clone(),
            folder: folder.to_path_buf(),
            created_at: Utc::now(),
            removed: false,
        });
        id
    }

    /// Id of the project the agent with ZNS id `zns_id` is in.
    fn project_id_of_zns(&self, zns_id: &str) -> Option<String> {
        self.file
            .agents
            .iter()
            .find(|a| a.zns_id == zns_id)
            .and_then(|a| a.project_id.clone())
    }

    /// Register a new agent in the launch directory's project. Blank names
    /// fall back to `Agent N`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create(&mut self, name: Option<String>) -> anyhow::Result<RegisteredAgent> {
        let name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Agent {}", self.file.agents.len() + 1));
        let project_id = self.launch_project();
        self.push(RegisteredAgent {
            project_id: Some(project_id),
            ..Self::new_agent(name)
        })
    }

    /// Open `folder` as a project. A folder is one project: when it is
    /// listed already, that project's first listed agent is returned and
    /// nothing changes. Otherwise the project is made, or put back on the
    /// list with its archived agents left in the Archive, and gets a new
    /// agent, which is returned.
    ///
    /// # Errors
    /// Returns an error if `folder` is not a directory or the registry
    /// cannot be written.
    pub fn open_project(&mut self, folder: &Path) -> anyhow::Result<(RegisteredAgent, Opened)> {
        let folder = folder
            .canonicalize()
            .with_context(|| format!("opening {}", folder.display()))?;
        if !folder.is_dir() {
            anyhow::bail!("{} is not a folder", folder.display());
        }
        let known = self
            .file
            .projects
            .iter()
            .find(|p| p.folder == folder)
            .map(|p| p.id.clone());
        let Some(id) = known else {
            let id = self.project_for(&folder);
            let agent = self.push(RegisteredAgent {
                project_id: Some(id),
                ..Self::new_agent(DEFAULT_PROJECT_AGENT.to_string())
            })?;
            return Ok((agent, Opened::Created));
        };
        let listed = self
            .tree()
            .into_iter()
            .find(|(agent, _)| agent.project_id.as_deref() == Some(id.as_str()))
            .map(|(agent, _)| agent.clone());
        if let Some(agent) = listed {
            return Ok((agent, Opened::Existing));
        }
        let agent = self.create_in_project(&id)?;
        Ok((agent, Opened::Revived))
    }

    /// Register a new agent in project `project_id`, named `Agent N` after
    /// how many agents the project already has.
    ///
    /// # Errors
    /// Returns an error if the project is unknown or the registry cannot be
    /// written.
    pub fn create_in_project(&mut self, project_id: &str) -> anyhow::Result<RegisteredAgent> {
        let Some(project) = self.file.projects.iter_mut().find(|p| p.id == project_id) else {
            anyhow::bail!("no such project");
        };
        project.removed = false;
        let count = self
            .file
            .agents
            .iter()
            .filter(|a| a.project_id.as_deref() == Some(project_id))
            .count();
        let name = if count == 0 {
            DEFAULT_PROJECT_AGENT.to_string()
        } else {
            format!("{DEFAULT_PROJECT_AGENT} {}", count + 1)
        };
        self.push(RegisteredAgent {
            project_id: Some(project_id.to_string()),
            ..Self::new_agent(name)
        })
    }

    /// Folder `agent_id` works in: its own, else the nearest one of the
    /// agents that spawned it, else its project's.
    #[must_use]
    pub fn project_folder(&self, agent_id: AgentId) -> Option<PathBuf> {
        let agent = self.find(agent_id)?;
        std::iter::once(agent)
            .chain(self.spawners(agent))
            .find_map(|a| a.folder.clone())
            .or_else(|| self.project_of(agent_id).map(|p| p.folder.clone()))
    }

    fn parent_index(&self, index: usize) -> Option<usize> {
        let parent = self.file.agents[index].parent.as_deref()?;
        self.file
            .agents
            .iter()
            .position(|agent| agent.zns_id == parent)
    }

    /// Every agent below `agent_id`: those it spawned, theirs, and so on.
    #[must_use]
    pub fn descendants(&self, agent_id: AgentId) -> Vec<AgentId> {
        let Some(agent) = self.find(agent_id) else {
            return Vec::new();
        };
        let mut found: Vec<&str> = vec![agent.zns_id.as_str()];
        let mut out = Vec::new();
        let mut i = 0;
        while i < found.len() {
            let parent = found[i];
            for child in self
                .file
                .agents
                .iter()
                .filter(|a| a.parent.as_deref() == Some(parent))
            {
                if !found.contains(&child.zns_id.as_str()) {
                    found.push(&child.zns_id);
                    out.push(child.agent_id());
                }
            }
            i += 1;
        }
        out
    }

    /// `agent_id` and the agents above it, nearest first.
    #[must_use]
    pub fn ancestry(&self, agent_id: AgentId) -> Vec<AgentId> {
        let Some(mut index) = self.position(agent_id) else {
            return Vec::new();
        };
        let mut out = vec![agent_id];
        for _ in 0..self.file.agents.len() {
            let Some(parent) = self.parent_index(index) else {
                break;
            };
            index = parent;
            out.push(self.file.agents[index].agent_id());
        }
        out
    }

    /// The agent at the top of `agent_id`'s tree, which nothing registered
    /// spawned; `agent_id` itself when it is at the top or unknown.
    #[must_use]
    pub fn root(&self, agent_id: AgentId) -> AgentId {
        self.ancestry(agent_id).last().copied().unwrap_or(agent_id)
    }

    /// Whether `agent_id` works under a `/compare`: one of its runs, or a
    /// subtask of one. The agents grouping an arm's runs do no work.
    #[must_use]
    pub fn under_compare(&self, agent_id: AgentId) -> bool {
        !self
            .find(agent_id)
            .is_some_and(RegisteredAgent::is_compare_arm)
            && self
                .ancestry(agent_id)
                .into_iter()
                .skip(1)
                .any(|id| self.find(id).is_some_and(RegisteredAgent::is_compare))
    }

    /// Register an agent named `name` under the agent with ZNS id
    /// `parent`, in its project, working in `folder` when given.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_child(
        &mut self,
        name: &str,
        parent: &str,
        folder: Option<PathBuf>,
    ) -> anyhow::Result<RegisteredAgent> {
        self.push(RegisteredAgent {
            parent: Some(parent.to_string()),
            project_id: self.project_id_of_zns(parent),
            folder,
            ..Self::new_agent(name.to_string())
        })
    }

    /// Register the agent that holds a new `/compare` asked for by the
    /// agent with ZNS id `parent`, named `Compare #<n>`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_compare(&mut self, parent: &str) -> anyhow::Result<RegisteredAgent> {
        let n = self.compares().len() + 1;
        self.push(RegisteredAgent {
            zns_id: format!("{COMPARE_ZNS_PREFIX}{}", uuid::Uuid::new_v4()),
            parent: Some(parent.to_string()),
            project_id: self.project_id_of_zns(parent),
            ..Self::new_agent(format!("Compare #{n}"))
        })
    }

    /// Register the agent named `name` that groups one arm's runs of the
    /// comparison with ZNS id `compare`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_compare_arm(
        &mut self,
        name: &str,
        compare: &str,
    ) -> anyhow::Result<RegisteredAgent> {
        self.push(RegisteredAgent {
            zns_id: format!("{COMPARE_ARM_ZNS_PREFIX}{}", uuid::Uuid::new_v4()),
            parent: Some(compare.to_string()),
            project_id: self.project_id_of_zns(compare),
            ..Self::new_agent(name.to_string())
        })
    }

    /// Have `agent_id` work in `folder`.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown or the registry cannot be
    /// written.
    pub fn set_folder(&mut self, agent_id: AgentId, folder: PathBuf) -> anyhow::Result<()> {
        let Some(index) = self.position(agent_id) else {
            anyhow::bail!("no such agent");
        };
        self.file.agents[index].folder = Some(folder);
        self.persist()
    }

    /// Drop `agent_id`, registered for work that never started, from the
    /// registry altogether. An agent something spawned stays.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn forget(&mut self, agent_id: AgentId) -> anyhow::Result<()> {
        let Some(index) = self.position(agent_id) else {
            return Ok(());
        };
        let zns_id = &self.file.agents[index].zns_id;
        if self
            .file
            .agents
            .iter()
            .any(|a| a.parent.as_ref() == Some(zns_id))
        {
            return Ok(());
        }
        self.file.agents.remove(index);
        self.persist()
    }

    /// The agents holding comparisons, oldest first.
    #[must_use]
    pub fn compares(&self) -> Vec<&RegisteredAgent> {
        self.file.agents.iter().filter(|a| a.is_compare()).collect()
    }

    /// Give `agent_id` the chat title `title`. Blank titles are ignored.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown or the registry cannot be
    /// written.
    pub fn set_title(&mut self, agent_id: AgentId, title: &str) -> anyhow::Result<()> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(());
        }
        let Some(index) = self.position(agent_id) else {
            anyhow::bail!("no such agent");
        };
        self.file.agents[index].title = Some(title.to_string());
        self.persist()
    }

    /// Give `agent_id` the subtask name `subtitle`. Blank subtitles are
    /// ignored.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown or the registry cannot be
    /// written.
    pub fn set_subtitle(&mut self, agent_id: AgentId, subtitle: &str) -> anyhow::Result<()> {
        let subtitle = subtitle.trim();
        if subtitle.is_empty() {
            return Ok(());
        }
        let Some(index) = self.position(agent_id) else {
            anyhow::bail!("no such agent");
        };
        self.file.agents[index].subtitle = Some(subtitle.to_string());
        self.persist()
    }

    /// Whether nothing registered spawned `agent`: it is listed at the top
    /// of its project.
    fn is_top_level(&self, agent: &RegisteredAgent) -> bool {
        agent
            .parent
            .as_deref()
            .is_none_or(|parent| !self.file.agents.iter().any(|a| a.zns_id == parent))
    }

    /// Register an instance of the permanent agent `definition`, named
    /// `<definition> #<n>`, spawned by the agent with ZNS id `parent` (in
    /// its project), or else in the launch directory's project.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_instance(
        &mut self,
        definition: &str,
        parent: Option<String>,
    ) -> anyhow::Result<RegisteredAgent> {
        let project_id = match parent.as_deref() {
            Some(parent) => self.project_id_of_zns(parent),
            None => Some(self.launch_project()),
        };
        self.push(RegisteredAgent {
            parent,
            project_id,
            ..self.new_instance(definition)
        })
    }

    /// Register an instance of the permanent agent `definition` at the top
    /// of project `project_id`.
    ///
    /// # Errors
    /// Returns an error if the project is unknown or the registry cannot be
    /// written.
    pub fn add_instance_to_project(
        &mut self,
        definition: &str,
        project_id: &str,
    ) -> anyhow::Result<RegisteredAgent> {
        let Some(project) = self.file.projects.iter_mut().find(|p| p.id == project_id) else {
            anyhow::bail!("no such project");
        };
        project.removed = false;
        self.push(RegisteredAgent {
            project_id: Some(project_id.to_string()),
            ..self.new_instance(definition)
        })
    }

    /// A fresh instance of `definition`, named `<definition> #<n>`, not yet
    /// registered. `n` is one past both the highest number any instance of
    /// `definition` has in its name and the highest number its code prefix
    /// was ever given, so neither a name nor a code is reused after an
    /// instance is dropped, even the highest-numbered one, and [`code::assign`]
    /// gives the instance the code with the same number.
    fn new_instance(&self, definition: &str) -> RegisteredAgent {
        let highest_named = self
            .file
            .agents
            .iter()
            .filter(|a| a.definition.as_deref() == Some(definition))
            .filter_map(|a| code::number_in_name(&a.name, definition))
            .max()
            .unwrap_or(0);
        let highest_coded = self
            .file
            .prefixes
            .get(definition)
            .map_or(0, |prefix| self.file.last_numbers.last(prefix));
        let number = highest_named.max(highest_coded).saturating_add(1);
        RegisteredAgent {
            definition: Some(definition.to_string()),
            ..Self::new_agent(format!("{definition} #{number}"))
        }
    }

    /// The agent `aura -p` runs as, registered on first use in the launch
    /// directory's project. It is an agent of its own, so a headless run
    /// never replaces the current session of an agent the terminal shows.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn headless(&mut self) -> anyhow::Result<RegisteredAgent> {
        if let Some(agent) = self
            .file
            .agents
            .iter()
            .find(|a| a.zns_id == HEADLESS_ZNS_ID)
        {
            return Ok(agent.clone());
        }
        let project_id = self.launch_project();
        self.push(RegisteredAgent {
            zns_id: HEADLESS_ZNS_ID.to_string(),
            project_id: Some(project_id),
            ..Self::new_agent(HEADLESS_AGENT_NAME.to_string())
        })
    }

    /// A fresh agent named `name`, not yet registered.
    fn new_agent(name: String) -> RegisteredAgent {
        RegisteredAgent {
            zns_id: format!("0://terminal/{}", uuid::Uuid::new_v4()),
            name,
            created_at: Utc::now(),
            definition: None,
            parent: None,
            project_id: None,
            folder: None,
            archived: false,
            removed: false,
            title: None,
            code: None,
            subtitle: None,
            legacy_member_of: None,
            id: IdCache::default(),
        }
    }

    /// Register `agent`, giving it its code, and save.
    fn push(&mut self, mut agent: RegisteredAgent) -> anyhow::Result<RegisteredAgent> {
        if agent.code.is_none() {
            agent.code = Some(code::assign(
                &agent,
                &self.file.agents,
                &mut self.file.prefixes,
                &mut self.file.last_numbers,
            ));
        }
        self.file.agents.push(agent.clone());
        self.persist()?;
        Ok(agent)
    }

    /// Listed agents in display order, paired with how deeply each is
    /// nested: project by project, each top-level agent followed by the
    /// agents it spawned, one level deeper. Archived agents and everything
    /// under them are left out (see [`Self::archived_tree`]).
    #[must_use]
    pub fn tree(&self) -> Vec<(&RegisteredAgent, usize)> {
        let index = self.index();
        let mut walk = Walk::new(self.file.agents.len());
        for project in self.file.projects.iter().filter(|p| !p.removed) {
            for agent in self.file.agents.iter().filter(|a| {
                a.project_id.as_deref() == Some(project.id.as_str()) && index.is_top_level(a)
            }) {
                if !agent.archived && !agent.removed {
                    self.visit(&index, agent, 0, false, &mut walk);
                }
            }
        }
        walk.out
    }

    /// One pass over the agents: where each ZNS id is, and the agents each
    /// one spawned, in registry order.
    fn index(&self) -> Index<'_> {
        let agents = &self.file.agents;
        let mut by_zns = HashMap::with_capacity(agents.len());
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, agent) in agents.iter().enumerate() {
            by_zns.entry(agent.zns_id.as_str()).or_insert(i);
            if let Some(parent) = agent.parent.as_deref() {
                children.entry(parent).or_default().push(i);
            }
        }
        Index {
            agents,
            by_zns,
            children,
        }
    }

    /// The Archive: each archived agent no archived agent spawned, project
    /// by project, followed by the agents it spawned, with depth counted
    /// from it.
    #[must_use]
    pub fn archived_tree(&self) -> Vec<(&RegisteredAgent, usize)> {
        let index = self.index();
        let mut walk = Walk::new(self.file.agents.len());
        let heads =
            |agent: &&RegisteredAgent| agent.archived && !index.spawners(agent).any(|p| p.archived);
        for project in &self.file.projects {
            for agent in self
                .file
                .agents
                .iter()
                .filter(|a| a.project_id.as_deref() == Some(project.id.as_str()))
                .filter(heads)
            {
                self.visit(&index, agent, 0, true, &mut walk);
            }
        }
        for agent in self
            .file
            .agents
            .iter()
            .filter(|a| {
                a.project_id
                    .as_deref()
                    .is_none_or(|id| self.project(id).is_none())
            })
            .filter(heads)
        {
            self.visit(&index, agent, 0, true, &mut walk);
        }
        walk.out
    }

    fn visit<'a>(
        &'a self,
        index: &Index<'a>,
        agent: &'a RegisteredAgent,
        depth: usize,
        with_archived: bool,
        walk: &mut Walk<'a>,
    ) {
        // A parent cycle would loop forever; registries never form one, but
        // a hand-edited file could.
        if walk.out.len() > self.file.agents.len() || !walk.seen.insert(agent.zns_id.as_str()) {
            return;
        }
        walk.out.push((agent, depth));
        let mut spawned: Vec<&RegisteredAgent> = index
            .children
            .get(agent.zns_id.as_str())
            .into_iter()
            .flatten()
            .map(|&i| &self.file.agents[i])
            .filter(|a| with_archived || !(a.archived || a.removed))
            .collect();
        if agent.is_compare() || agent.is_compare_arm() {
            // Runs register as their copies become ready, in no set order.
            spawned.sort_by(|x, y| run_order(&x.name).cmp(&run_order(&y.name)));
        }
        for child in spawned {
            self.visit(index, child, depth + 1, with_archived, walk);
        }
    }

    /// The agents that spawned `agent`, nearest first.
    fn spawners(&self, agent: &RegisteredAgent) -> Vec<&RegisteredAgent> {
        let mut out = Vec::new();
        let mut current = agent;
        while let Some(parent) = current
            .parent
            .as_deref()
            .and_then(|zns| self.file.agents.iter().find(|a| a.zns_id == zns))
        {
            if out.len() >= self.file.agents.len() {
                break;
            }
            out.push(parent);
            current = parent;
        }
        out
    }

    /// Whether `agent` is on the list: its project is, and neither it nor
    /// an agent that spawned it is archived or left behind by a removal.
    fn is_listed(&self, agent: &RegisteredAgent) -> bool {
        let project_listed = agent
            .project_id
            .as_deref()
            .and_then(|id| self.project(id))
            .is_some_and(|p| !p.removed);
        project_listed
            && !agent.archived
            && !agent.removed
            && !self.spawners(agent).iter().any(|p| p.archived || p.removed)
    }

    /// Projects on the list, in order: every project not removed, with
    /// its agents listed or all of them archived.
    #[must_use]
    pub fn listed_projects(&self) -> Vec<&Project> {
        self.file.projects.iter().filter(|p| !p.removed).collect()
    }

    /// How many agents of project `project_id` are listed, and how many
    /// sit in the Archive.
    #[must_use]
    pub fn project_counts(&self, project_id: &str) -> (usize, usize) {
        let count = |tree: Vec<(&RegisteredAgent, usize)>| {
            tree.iter()
                .filter(|(a, _)| a.project_id.as_deref() == Some(project_id))
                .count()
        };
        (count(self.tree()), count(self.archived_tree()))
    }

    /// [`Self::project_counts`] of every project, from one walk of each
    /// list.
    #[must_use]
    pub fn project_counts_all(&self) -> HashMap<String, (usize, usize)> {
        let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
        for (agent, _) in self.tree() {
            if let Some(id) = &agent.project_id {
                counts.entry(id.clone()).or_default().0 += 1;
            }
        }
        for (agent, _) in self.archived_tree() {
            if let Some(id) = &agent.project_id {
                counts.entry(id.clone()).or_default().1 += 1;
            }
        }
        counts
    }

    /// `agent_id` and every agent under it.
    #[must_use]
    pub fn subtree(&self, agent_id: AgentId) -> Vec<AgentId> {
        let Some(agent) = self.find(agent_id) else {
            return Vec::new();
        };
        let index = self.index();
        let mut walk = Walk::new(self.file.agents.len());
        self.visit(&index, agent, 0, true, &mut walk);
        walk.out.into_iter().map(|(a, _)| a.agent_id()).collect()
    }

    /// Move `agent_id`, and the agents it spawned, to the Archive. The
    /// other agents of its project stay listed.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown, it is the last listed
    /// agent, or the registry cannot be written.
    pub fn archive(&mut self, agent_id: AgentId) -> anyhow::Result<()> {
        let Some(index) = self.position(agent_id) else {
            anyhow::bail!("no such agent");
        };
        let hiding: HashSet<AgentId> = self.subtree(agent_id).into_iter().collect();
        if self
            .tree()
            .iter()
            .all(|(agent, _)| hiding.contains(&agent.agent_id()))
        {
            anyhow::bail!("it is the last agent in the list; create another project first");
        }
        self.file.agents[index].archived = true;
        self.persist()
    }

    /// Bring `agent_id` back from the Archive, with the agents that spawned
    /// it so it has its old place in the list. No other agent of its
    /// project comes back. When its project was removed in the meantime,
    /// the project comes back holding just it.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown or the registry cannot be
    /// written.
    pub fn restore(&mut self, agent_id: AgentId) -> anyhow::Result<()> {
        let Some(agent) = self.find(agent_id) else {
            anyhow::bail!("no such agent");
        };
        let mut restoring: Vec<String> = self
            .spawners(agent)
            .iter()
            .map(|a| a.zns_id.clone())
            .collect();
        restoring.push(agent.zns_id.clone());
        let project_id = agent.project_id.clone();
        for agent in &mut self.file.agents {
            if restoring.contains(&agent.zns_id) {
                agent.archived = false;
                agent.removed = false;
            }
        }
        if let Some(project) = self
            .file
            .projects
            .iter_mut()
            .find(|p| Some(&p.id) == project_id.as_ref())
        {
            project.removed = false;
        }
        self.persist()
    }

    /// Take project `project_id` off the list with its agents. Agents in
    /// the Archive stay there, so the project and the agents that spawned
    /// them are kept, hidden, to put back when one is restored. The other
    /// agents are dropped from the registry; records and the project's
    /// folder are left alone. Returns the agents that left the list.
    ///
    /// # Errors
    /// Returns an error if the project is unknown, it holds every listed
    /// agent, or the registry cannot be written.
    pub fn remove_project(&mut self, project_id: &str) -> anyhow::Result<Vec<AgentId>> {
        if self.project(project_id).is_none() {
            anyhow::bail!("no such project");
        }
        let in_project = |a: &RegisteredAgent| a.project_id.as_deref() == Some(project_id);
        let tree = self.tree();
        if tree.iter().all(|(agent, _)| in_project(agent)) {
            anyhow::bail!("it is the last project in the list; create another one first");
        }
        let gone: Vec<AgentId> = tree
            .iter()
            .filter(|(agent, _)| in_project(agent))
            .map(|(agent, _)| agent.agent_id())
            .collect();
        let heads: Vec<&RegisteredAgent> = self
            .archived_tree()
            .into_iter()
            .filter(|(agent, depth)| *depth == 0 && in_project(agent))
            .map(|(agent, _)| agent)
            .collect();
        let archived: Vec<AgentId> = heads
            .iter()
            .flat_map(|head| self.subtree(head.agent_id()))
            .collect();
        let hiding: Vec<AgentId> = heads
            .iter()
            .flat_map(|head| self.spawners(head))
            .map(RegisteredAgent::agent_id)
            .collect();
        for agent in &mut self.file.agents {
            if hiding.contains(&agent.agent_id()) {
                agent.removed = true;
            }
        }
        self.file.agents.retain(|agent| {
            let id = agent.agent_id();
            !in_project(agent) || archived.contains(&id) || hiding.contains(&id)
        });
        if let Some(project) = self.file.projects.iter_mut().find(|p| p.id == project_id) {
            project.removed = true;
        }
        if self
            .file
            .active
            .as_deref()
            .is_some_and(|zns| !self.file.agents.iter().any(|a| a.zns_id == zns))
        {
            self.file.active = None;
        }
        self.persist()?;
        Ok(gone)
    }

    /// List the projects `order` names in that order, in the places they
    /// held between them. Other projects keep their places, so a view
    /// showing some of the projects can reorder just those.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn reorder_projects(&mut self, order: &[String]) -> anyhow::Result<()> {
        let slots: Vec<usize> = (0..self.file.projects.len())
            .filter(|&i| order.contains(&self.file.projects[i].id))
            .collect();
        if reorder_slots(&mut self.file.projects, &slots, order, |p| p.id.clone()) {
            self.persist()?;
        }
        Ok(())
    }

    /// List the top-level agents of project `project_id` that `order`
    /// names (by hex id) in that order, in the places they held between
    /// them. The agents each spawned move with it.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn reorder_agents(&mut self, project_id: &str, order: &[String]) -> anyhow::Result<()> {
        let slots: Vec<usize> = (0..self.file.agents.len())
            .filter(|&i| {
                let agent = &self.file.agents[i];
                agent.project_id.as_deref() == Some(project_id)
                    && self.is_top_level(agent)
                    && order.contains(&agent.agent_id().to_hex())
            })
            .collect();
        if reorder_slots(&mut self.file.agents, &slots, order, |a| {
            a.agent_id().to_hex()
        }) {
            self.persist()?;
        }
        Ok(())
    }

    fn position(&self, agent_id: AgentId) -> Option<usize> {
        self.file
            .agents
            .iter()
            .position(|agent| agent.agent_id() == agent_id)
    }

    /// Write the rows that changed since the last write, in one batch.
    /// What is remembered as stored moves on only once the batch landed,
    /// so a failed write is retried by the next one.
    fn persist(&mut self) -> anyhow::Result<()> {
        let (diff, next) = self.persisted.diff(persist::to_rows(&self.file));
        if diff.is_empty() {
            return Ok(());
        }
        self.store.apply(&diff).with_context(|| {
            format!(
                "saving agent registry in {} ({} agents, {} projects changed)",
                self.data_dir.display(),
                diff.agents_put.len() + diff.agents_del.len(),
                diff.projects_put.len() + diff.projects_del.len(),
            )
        })?;
        self.persisted = next;
        Ok(())
    }

    /// Data directory the registry belongs to.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// A fresh coordinator for the channel `slug`, working in `folder`,
    /// not yet registered: an instance of
    /// [`crate::agent_definitions::COORDINATOR_DEFINITION`] named
    /// `#<slug> coordinator` in the launch directory's project. Its code is
    /// given by [`Self::adopt`]. The launch project is made in memory when
    /// missing, as for other new agents, and saved with the next write.
    pub(crate) fn coordinator_draft(&mut self, slug: &str, folder: PathBuf) -> RegisteredAgent {
        let project_id = self.launch_project();
        RegisteredAgent {
            definition: Some(crate::agent_definitions::COORDINATOR_DEFINITION.to_string()),
            project_id: Some(project_id),
            folder: Some(folder),
            ..Self::new_agent(format!("#{slug} coordinator"))
        }
    }

    /// The row `agent`, not yet registered, has once [`Self::adopt`]
    /// appends it (before its code is assigned).
    pub(crate) fn record_for(&self, agent: &RegisteredAgent) -> aura_store_db::AgentRecord {
        persist::appended_row(&self.file, agent)
    }

    /// Register `agent`, whose row may already be stored (as a channel's
    /// coordinator is), giving it its code, and save. The stored row is
    /// rewritten with the code.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub(crate) fn adopt(&mut self, agent: RegisteredAgent) -> anyhow::Result<RegisteredAgent> {
        self.push(agent)
    }

    /// The stored row of the registered agent `agent_id`, as the registry
    /// last saved it.
    pub(crate) fn row_of(&self, agent_id: AgentId) -> Option<aura_store_db::AgentRecord> {
        persist::registered_row(&self.file, agent_id)
    }

    /// The store the registry is kept in.
    pub(crate) fn store(&self) -> &AgentStore {
        &self.store
    }
}

/// The registry in `<data_dir>/agents.json`, for the one-time import into
/// the store; empty when there is no such file.
fn read_registry_file(data_dir: &Path) -> anyhow::Result<RegistryFile> {
    let path = data_dir.join(REGISTRY_FILE);
    if !path.exists() {
        return Ok(RegistryFile::default());
    }
    let body = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))
}

/// Rename `<data_dir>/agents.json` to `agents.json.migrated` once its
/// contents are in the store, so it is never imported again and the
/// original stays on disk. A failed rename is only logged: the store is
/// not empty any more, so the file is not read again either way.
fn retire_registry_file(data_dir: &Path) {
    let path = data_dir.join(REGISTRY_FILE);
    if !path.exists() {
        return;
    }
    let target = data_dir.join(MIGRATED_REGISTRY_FILE);
    if let Err(error) = fs::rename(&path, &target) {
        tracing::warn!(
            from = %path.display(),
            to = %target.display(),
            %error,
            "could not rename the imported agent registry"
        );
    } else {
        tracing::info!(to = %target.display(), "imported agents.json into the store");
    }
}

/// Lookups built in one pass over the registry's agents, so a walk of the
/// lists finds parents and children without scanning every agent.
struct Index<'a> {
    agents: &'a [RegisteredAgent],
    /// Position of the first agent with each ZNS id
    by_zns: HashMap<&'a str, usize>,
    /// Positions of the agents naming each ZNS id as parent, in order
    children: HashMap<&'a str, Vec<usize>>,
}

impl<'a> Index<'a> {
    fn parent_of(&self, agent: &RegisteredAgent) -> Option<&'a RegisteredAgent> {
        let parent = agent.parent.as_deref()?;
        self.by_zns.get(parent).map(|&i| &self.agents[i])
    }

    /// As [`AgentRegistry::is_top_level`].
    fn is_top_level(&self, agent: &RegisteredAgent) -> bool {
        self.parent_of(agent).is_none()
    }

    /// The agents that spawned `agent`, nearest first. Stops after as many
    /// steps as there are agents, in case of a parent cycle.
    fn spawners<'s>(
        &'s self,
        agent: &'a RegisteredAgent,
    ) -> impl Iterator<Item = &'a RegisteredAgent> + 's
    where
        'a: 's,
    {
        let mut current = agent;
        let mut left = self.agents.len();
        std::iter::from_fn(move || {
            if left == 0 {
                return None;
            }
            left -= 1;
            let parent = self.parent_of(current)?;
            current = parent;
            Some(parent)
        })
    }
}

/// Rows of one walk of the lists, and the agents it has listed.
struct Walk<'a> {
    out: Vec<(&'a RegisteredAgent, usize)>,
    seen: HashSet<&'a str>,
}

impl Walk<'_> {
    fn new(capacity: usize) -> Self {
        Self {
            out: Vec::with_capacity(capacity),
            seen: HashSet::with_capacity(capacity),
        }
    }
}

fn legacy_or_new_zns_id(data_dir: &Path) -> anyhow::Result<String> {
    let identity_file = data_dir.join(LEGACY_IDENTITY_FILE);
    if identity_file.exists() {
        let zns_id = fs::read_to_string(&identity_file)
            .with_context(|| format!("reading {}", identity_file.display()))?;
        return Ok(zns_id);
    }
    let zns_id = format!("0://terminal/{}", uuid::Uuid::new_v4());
    fs::write(&identity_file, &zns_id)
        .with_context(|| format!("writing {}", identity_file.display()))?;
    Ok(zns_id)
}

mod code;
mod persist;

#[cfg(test)]
mod tests;
