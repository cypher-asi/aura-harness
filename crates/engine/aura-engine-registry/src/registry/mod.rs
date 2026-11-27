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

use aura_core_types::{AgentId, Identity};
use aura_store_db::{AgentStore, RocksStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

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
/// listed. From version 4 a channel's coordinator is named `coordinator`
/// rather than `#<slug> coordinator`.
const REGISTRY_VERSION: u32 = 4;
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

/// Where a channel's coordinator lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorHome {
    /// At the top of the project with this id, working in its folder.
    Project(String),
    /// In the launch directory's project, working in this folder (the
    /// channel's own, for a channel without a project).
    Folder(PathBuf),
}

/// The project and own folder an agent is moved to by
/// [`AgentRegistry::place_agents`]. `folder: None` has it work where the
/// agents that spawned it, or else its project, do (see
/// [`AgentRegistry::project_folder`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPlacement {
    pub agent_id: AgentId,
    pub project_id: Option<String>,
    pub folder: Option<PathBuf>,
}

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
    /// When it was moved to the Archive; orders the Archive, newest first.
    /// None for listed agents and for ones archived before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
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
    /// such as `REN36` for `rust-engineer #36`. Unique in the registry and
    /// never changed once assigned: the registry gives one to every agent
    /// it registers, and to agents of older registries when it loads them
    /// (older `REN-36` codes become `REN36`; two-letter ones such as
    /// `RE36` are given a new three-letter code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Short name of the subtask a spawned permanent-agent instance was
    /// given ("Fix parser escapes"), written by the title model from the
    /// task prompt. Shown after the code (`REN36: Fix parser escapes`)
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
    /// Agents in a channel, as the hub tells it
    /// ([`Self::mark_channel_members`]). They live with their channel, not
    /// a project: removing their project keeps them, and they stay listed.
    channel_members: HashSet<AgentId>,
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

mod code;
mod create;
mod edit;
mod load;
mod lookup;
mod migrate;
mod persist;
mod placement;
mod tree;

pub use crate::error::AgentRegistryError;
use create::canonical;
use migrate::{legacy_or_new_zns_id, read_registry_file, retire_registry_file, ImportSource};

#[cfg(test)]
mod tests;
