//! Agents, permanent agents and projects, for the Projects list.

use serde::{Deserialize, Serialize};

/// Summary of a permanent agent for display in the roster.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDefinitionSummary {
    /// Name agents spawn it by
    pub name: String,
    /// What it does and when to spawn it
    pub description: String,
    /// Its unique abbreviation, always three uppercase letters and the
    /// prefix of its instances' codes (`REN` for `rust-engineer`, whose
    /// instances are `REN1`, `REN2`, …), when known
    #[serde(default)]
    pub abbreviation: Option<String>,
}

/// Summary of an agent for display in the Swarm panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSummary {
    /// Agent ID (hex encoded)
    pub id: String,
    /// Display name
    pub name: String,
    /// ZNS identifier (e.g., "0://Agent09")
    pub zns_id: String,
    /// Whether this agent is currently active
    pub is_active: bool,
    /// Number of records for this agent
    pub record_count: u64,
    /// Last activity timestamp (HH:MM:SS or date)
    pub last_active: String,
    /// Time of its latest record, in Unix milliseconds
    #[serde(default)]
    pub last_active_ms: Option<u64>,
    /// When it was moved to the Archive, in Unix milliseconds; none for
    /// listed agents
    #[serde(default)]
    pub archived_at_ms: Option<u64>,
    /// Permanent agent this is an instance of
    #[serde(default)]
    pub definition: Option<String>,
    /// How many spawning agents it is nested under (0 for top-level agents)
    #[serde(default)]
    pub depth: usize,
    /// Whether it is working right now: running a turn, a command, or a
    /// task for the agent that spawned it
    #[serde(default)]
    pub running: bool,
    /// Whether it is a run of a benchmark, or works under one
    #[serde(default)]
    pub bench_run: bool,
    /// Id of the project it is in. A project's agents are listed together.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Name of the project it is in
    #[serde(default)]
    pub project: Option<String>,
    /// Whether it is in plan mode: it researches and writes a plan instead
    /// of changing code
    #[serde(default)]
    pub plan_mode: bool,
    /// Short code name, such as `REN36`, unique among the agents and never
    /// changed; shown at the right end of the agent's row
    #[serde(default)]
    pub code: Option<String>,
    /// Short name of the subtask a spawned agent works on, such as
    /// `Fix parser escapes`. `name` already shows it after the code
    /// (`REN36: Fix parser escapes`); kept apart so UIs can style it.
    #[serde(default)]
    pub subtitle: Option<String>,
    /// Title of what its chat is about, such as `Fix parser escapes`; for a
    /// channel member, what it works on. `name` already shows it; kept
    /// apart so a channel can show it under the member's name.
    #[serde(default)]
    pub title: Option<String>,
    /// Why it stopped working, when its last turn or its start failed:
    /// its row shows it in red until its next turn starts
    #[serde(default)]
    pub error: Option<String>,
    /// Whether it is starting on a cloud machine of its own, which it
    /// needs before it can work
    #[serde(default)]
    pub starting: bool,
    /// Whether its turn waits for the one already under way on its cloud
    /// machine, which must end before this one starts
    #[serde(default)]
    pub waiting: bool,
    /// Whether Aura is reconnecting to the turn running on its cloud
    /// machine, as it does after a restart
    #[serde(default)]
    pub reconnecting: bool,
}

impl AgentSummary {
    /// Key of the project it is listed in: the project's id, or its own id
    /// when it has none.
    #[must_use]
    pub fn project_key(&self) -> &str {
        self.project_id.as_deref().unwrap_or(&self.id)
    }
}

/// Whether `agents[index]` is the first agent of its project, in a list
/// that lists each project's agents together. An agent without a project
/// starts one of its own unless it was spawned.
#[must_use]
pub fn starts_project(agents: &[AgentSummary], index: usize) -> bool {
    let agent = &agents[index];
    match &agent.project_id {
        Some(id) => index == 0 || agents[index - 1].project_id.as_ref() != Some(id),
        None => agent.depth == 0,
    }
}

/// Where the projects with no agent in `agents`, every one of theirs
/// being archived, go among the projects of `agents`: for each,
/// the index of the agent it goes before (`agents.len()` for the end) and
/// its index in `projects`, in list order. Each keeps its place: as many
/// projects of `agents` come before it as come before it in `projects`,
/// however those were reordered since.
#[must_use]
pub fn empty_projects(projects: &[ProjectSummary], agents: &[AgentSummary]) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = (0..agents.len())
        .filter(|&i| starts_project(agents, i))
        .collect();
    let shown = |project: &ProjectSummary| agents.iter().any(|a| a.project_key() == project.id);
    let mut above = 0;
    let mut out = Vec::new();
    for (at, project) in projects.iter().enumerate() {
        if shown(project) {
            above += 1;
        } else {
            let before = starts.get(above).copied().unwrap_or(agents.len());
            out.push((before, at));
        }
    }
    out
}

/// A project of the projects list: a folder its agents work in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    /// Project id
    pub id: String,
    /// What it is called: its folder's name
    pub name: String,
    /// Full path of its folder
    pub folder: String,
    /// Whether the folder is still there
    pub folder_exists: bool,
    /// When it was made, in local time
    pub created: String,
    /// How many of its agents are listed
    pub listed_agents: usize,
    /// How many of its agents sit in the Archive
    pub archived_agents: usize,
}
