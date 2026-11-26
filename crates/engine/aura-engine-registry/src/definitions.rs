//! Permanent agent definitions: named roles that agents can spawn.
//!
//! Each definition is a Markdown file in `<data_dir>/agent_defs/` with YAML
//! frontmatter (`name`, `description`, and optionally `model`, `tools` and
//! `skills`) and the agent's rules as the body. Instances created from a
//! definition run with its rules, skills, tools and model.
//!
//! The directory is re-read before every turn, so edits made outside the
//! terminal apply to the next turn without a restart.

use anyhow::{bail, Context};
use aura_context_skills::prompt::{inject_full_skills, SkillPromptEntry};
use aura_context_skills::SkillManager;
use aura_exec_tools::agents::get_agent_state::GET_AGENT_STATE_TOOL_NAME;
use aura_exec_tools::agents::list_agents::LIST_AGENTS_TOOL_NAME;
use aura_exec_tools::agents::send_to_agent::SEND_TO_AGENT_TOOL_NAME;
use aura_exec_tools::agents::task::TASK_TOOL_NAME;
use aura_exec_tools::todo::{ASSIGN_TOOL_NAME, TODO_WRITE_TOOL_NAME};
use aura_model::ToolDefinition;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const DEFINITIONS_DIR: &str = "agent_defs";
const MAX_NAME_LEN: usize = 40;
const MAX_FILE_BYTES: u64 = 64 * 1024;
const NEW_DEFINITION_NAME: &str = "new-agent";

/// The definition channel coordinators run. Only channels create its
/// instances: it can't be started on its own and is never offered to
/// `task` or `assign`.
pub const COORDINATOR_DEFINITION: &str = "coordinator";

/// The tools a coordinator's turns offer, whatever its definition lists:
/// it hands work out and checks on it, and does none of it itself.
pub const COORDINATOR_TOOLS: &[&str] = &[
    ASSIGN_TOOL_NAME,
    SEND_TO_AGENT_TOOL_NAME,
    LIST_AGENTS_TOOL_NAME,
    GET_AGENT_STATE_TOOL_NAME,
    TODO_WRITE_TOOL_NAME,
];

/// Names of the seeded definitions already added once, one per line.
const SEEDED_FILE: &str = ".seeded";

/// Built-in definitions, each added once to a roster; deleting one never
/// re-adds it. The specialists match AURA's default agent roster.
const SEED_DEFINITIONS: &[(&str, &str)] = &[
    ("explorer", include_str!("definitions/explorer.md")),
    ("reviewer", include_str!("definitions/reviewer.md")),
    ("implement", include_str!("definitions/implement.md")),
    ("tests", include_str!("definitions/tests.md")),
    ("engineer", include_str!("definitions/engineer.md")),
    (
        "rust-engineer",
        include_str!("definitions/rust-engineer.md"),
    ),
    (
        "frontend-engineer",
        include_str!("definitions/frontend-engineer.md"),
    ),
    ("ui-designer", include_str!("definitions/ui-designer.md")),
    ("web-designer", include_str!("definitions/web-designer.md")),
    (
        "product-manager",
        include_str!("definitions/product-manager.md"),
    ),
    (
        COORDINATOR_DEFINITION,
        include_str!("definitions/coordinator.md"),
    ),
];

/// Seeds written by versions that seeded only when creating the directory.
const LEGACY_SEEDS: &[&str] = &["explorer", "reviewer"];

/// Built-in definitions as earlier versions wrote them: name and blake3
/// hash of the file. A file still byte for byte like one was never edited,
/// so loading writes the current built-in over it.
const RETIRED_SEEDS: &[(&str, &str)] = &[
    (
        COORDINATOR_DEFINITION,
        "6576782db9578ef2edeb45ccdba0983f9a9a74f1999112f010e5fa14d85216e3",
    ),
    (
        COORDINATOR_DEFINITION,
        "a861b09a8d8705ef2a184ee24f6f16b9ba4880a8302f7d4f61d34fc50c4eae31",
    ),
];

#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    skills: Vec<String>,
}

/// One permanent agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    /// Unique name; agents spawn the definition by it.
    pub name: String,
    /// What the agent does and when to spawn it.
    pub description: String,
    /// Model for its instances; `None` uses the current `/model`.
    pub model: Option<String>,
    /// Tools its instances may use; `None` allows every built-in tool
    /// except `task`.
    pub tools: Option<Vec<String>>,
    /// Skills whose full instructions are added to its prompt.
    pub skills: Vec<String>,
    /// Standing instructions for every instance.
    pub rules: String,
    /// File the definition was read from.
    pub path: PathBuf,
}

impl AgentDefinition {
    /// Whether this is the channel coordinator, which only channels create
    /// instances of (see [`COORDINATOR_DEFINITION`]).
    #[must_use]
    pub fn is_coordinator(&self) -> bool {
        self.name == COORDINATOR_DEFINITION
    }

    /// Parse a definition file's contents.
    ///
    /// # Errors
    /// Returns an error if the frontmatter is missing or invalid, or the
    /// name is not a valid definition name.
    pub fn parse(content: &str, path: &Path) -> anyhow::Result<Self> {
        let content = content.trim_start_matches('\u{feff}');
        let Some(after_open) = content.strip_prefix("---") else {
            bail!("missing `---` frontmatter at the top of the file");
        };
        let Some(close) = after_open.find("\n---") else {
            bail!("frontmatter is not closed with `---`");
        };
        let yaml = &after_open[..close];
        let rest = &after_open[close + "\n---".len()..];
        let body = rest.split_once('\n').map_or("", |(_, body)| body);

        let frontmatter: Frontmatter = serde_yaml::from_str(yaml).context("invalid frontmatter")?;
        let name = frontmatter.name.trim().to_string();
        validate_name(&name)?;
        Ok(Self {
            name,
            description: frontmatter.description.trim().to_string(),
            model: frontmatter
                .model
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty()),
            tools: frontmatter.tools,
            skills: frontmatter.skills,
            rules: body.trim().to_string(),
            path: path.to_path_buf(),
        })
    }

    /// Append this definition's identity, rules and skills to `prompt`.
    ///
    /// Skills that are not installed are skipped with a warning.
    pub fn extend_system_prompt(&self, prompt: &mut String, skills: &SkillManager) {
        prompt.push_str(&format!(
            "\n\n<agent_definition name=\"{}\">\nYou are an instance of the permanent agent \"{}\".",
            self.name, self.name
        ));
        if !self.description.is_empty() {
            prompt.push_str(&format!(" Your role: {}", self.description));
        }
        if !self.rules.is_empty() {
            prompt.push_str("\nFollow these rules:\n\n");
            prompt.push_str(&self.rules);
        }
        prompt.push_str("\n</agent_definition>");

        let found: Vec<_> = self
            .skills
            .iter()
            .filter_map(|name| match skills.get(name) {
                Ok(skill) => Some(skill),
                Err(e) => {
                    warn!(definition = %self.name, skill = %name, error = %e, "Skill not found");
                    None
                }
            })
            .collect();
        let entries: Vec<SkillPromptEntry<'_>> = found
            .iter()
            .map(|skill| SkillPromptEntry {
                name: &skill.frontmatter.name,
                description: &skill.frontmatter.description,
                body: &skill.body,
                dir_path: &skill.dir_path,
                agent_target_id: None,
                agent_target_name: None,
            })
            .collect();
        inject_full_skills(prompt, &entries);
    }

    /// The subset of `tools` its instances may use.
    #[must_use]
    pub fn filter_tools(&self, tools: &[ToolDefinition]) -> Vec<ToolDefinition> {
        tools
            .iter()
            .filter(|tool| match &self.tools {
                Some(allowed) => allowed.contains(&tool.name),
                None => tool.name != TASK_TOOL_NAME,
            })
            .cloned()
            .collect()
    }
}

/// Check that `name` can be used as a definition name and file name.
///
/// # Errors
/// Returns an error describing what is wrong with the name.
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        bail!("name must not be empty");
    }
    if name.len() > MAX_NAME_LEN {
        bail!("name must be at most {MAX_NAME_LEN} characters");
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid || name.starts_with(['-', '_']) {
        bail!("name `{name}` must use lowercase letters, digits, `-` or `_`");
    }
    Ok(())
}

/// The permanent agents in `<data_dir>/agent_defs/`.
#[derive(Debug, Clone)]
pub struct AgentDefinitions {
    dir: PathBuf,
    definitions: Vec<AgentDefinition>,
}

impl AgentDefinitions {
    /// Load the definitions, first adding each built-in definition this
    /// directory has never had. One whose name is already taken is skipped
    /// for good. A built-in still exactly as an earlier version wrote it
    /// (see [`RETIRED_SEEDS`]) is brought up to date.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or seeded.
    pub fn load_or_seed(data_dir: &Path) -> anyhow::Result<Self> {
        let dir = data_dir.join(DEFINITIONS_DIR);
        let mut seeded = if dir.exists() {
            read_seeded(&dir)
        } else {
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            Vec::new()
        };
        let mut definitions = Self {
            dir,
            definitions: Vec::new(),
        };
        definitions.reload();

        let mut added = false;
        for (name, content) in SEED_DEFINITIONS {
            if seeded.iter().any(|s| s == name) {
                continue;
            }
            seeded.push((*name).to_string());
            added = true;
            let path = definitions.dir.join(format!("{name}.md"));
            if definitions.get(name).is_none() && !path.exists() {
                fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
            }
        }
        if added {
            let path = definitions.dir.join(SEEDED_FILE);
            let mut list = seeded.join("\n");
            list.push('\n');
            fs::write(&path, list).with_context(|| format!("writing {}", path.display()))?;
        }
        for name in refresh_retired(&definitions.dir, RETIRED_SEEDS)? {
            info!(
                definition = name,
                "Updated a built-in agent definition nobody edited"
            );
        }

        for problem in definitions.reload() {
            warn!(%problem, "Skipping agent definition");
        }
        Ok(definitions)
    }

    /// Re-read the directory, returning a description of every file that
    /// could not be used.
    pub fn reload(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut paths: Vec<PathBuf> = match fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .collect(),
            Err(e) => {
                problems.push(format!("{}: {e}", self.dir.display()));
                Vec::new()
            }
        };
        paths.sort();

        let mut definitions: Vec<AgentDefinition> = Vec::with_capacity(paths.len());
        for path in paths {
            match read_definition(&path) {
                Ok(definition) => {
                    if definitions.iter().any(|d| d.name == definition.name) {
                        problems.push(format!(
                            "{}: another file already defines `{}`",
                            path.display(),
                            definition.name
                        ));
                    } else {
                        definitions.push(definition);
                    }
                }
                Err(e) => problems.push(format!("{}: {e:#}", path.display())),
            }
        }
        self.definitions = definitions;
        problems
    }

    /// Every loaded definition, sorted by file name.
    #[must_use]
    pub fn all(&self) -> &[AgentDefinition] {
        &self.definitions
    }

    /// Whether no definitions are loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Look up a definition by name, ignoring case.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        let name = name.trim();
        self.definitions
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// Write a skeleton definition and return its path. `name` defaults to
    /// the first free `new-agent`, `new-agent-2`, ...
    ///
    /// # Errors
    /// Returns an error if the name is invalid or taken, or the file cannot
    /// be written.
    pub fn create(&mut self, name: Option<&str>) -> anyhow::Result<PathBuf> {
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => {
                let name = name.to_lowercase();
                validate_name(&name)?;
                if self.get(&name).is_some() || self.dir.join(format!("{name}.md")).exists() {
                    bail!("a permanent agent named `{name}` already exists");
                }
                name
            }
            None => self.free_name(),
        };
        let path = self.dir.join(format!("{name}.md"));
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        fs::write(&path, skeleton(&name)).with_context(|| format!("writing {}", path.display()))?;
        self.reload();
        Ok(path)
    }

    /// Delete the file of the definition named `name`.
    ///
    /// # Errors
    /// Returns an error if no such definition exists or its file cannot be
    /// removed.
    pub fn delete(&mut self, name: &str) -> anyhow::Result<AgentDefinition> {
        let Some(definition) = self.get(name).cloned() else {
            bail!("no permanent agent named `{name}`");
        };
        fs::remove_file(&definition.path)
            .with_context(|| format!("removing {}", definition.path.display()))?;
        self.reload();
        Ok(definition)
    }

    fn free_name(&self) -> String {
        (1..)
            .map(|n| {
                if n == 1 {
                    NEW_DEFINITION_NAME.to_string()
                } else {
                    format!("{NEW_DEFINITION_NAME}-{n}")
                }
            })
            .find(|name| self.get(name).is_none() && !self.dir.join(format!("{name}.md")).exists())
            .unwrap_or_else(|| NEW_DEFINITION_NAME.to_string())
    }
}

/// The seeds `dir` has already had. A directory without the list was
/// created by a version that seeded only [`LEGACY_SEEDS`].
fn read_seeded(dir: &Path) -> Vec<String> {
    match fs::read_to_string(dir.join(SEEDED_FILE)) {
        Ok(list) => list
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => LEGACY_SEEDS.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// Write the current built-in over each seed file in `dir` that is byte
/// for byte one of the `retired` versions (name and blake3 hash), and
/// return the names written. A file that can't be read is left alone.
fn refresh_retired(dir: &Path, retired: &[(&str, &str)]) -> anyhow::Result<Vec<&'static str>> {
    let mut refreshed = Vec::new();
    for (name, content) in SEED_DEFINITIONS {
        if !retired.iter().any(|(retired, _)| retired == name) {
            continue;
        }
        let path = dir.join(format!("{name}.md"));
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let hash = blake3::hash(&bytes).to_hex();
        if !retired.contains(&(*name, hash.as_str())) {
            continue;
        }
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        refreshed.push(*name);
    }
    Ok(refreshed)
}

fn read_definition(path: &Path) -> anyhow::Result<AgentDefinition> {
    let size = fs::metadata(path)?.len();
    if size > MAX_FILE_BYTES {
        bail!("file is larger than {MAX_FILE_BYTES} bytes");
    }
    let content = fs::read_to_string(path)?;
    AgentDefinition::parse(&content, path)
}

fn skeleton(name: &str) -> String {
    format!(
        "---
name: {name}
description: What this agent does and when other agents should spawn it.
# model: <model id>                              (default: the current /model)
# tools: [read_file, search_code, run_command]   (default: every built-in tool)
# skills: [skill-name]                           (from ./skills or ~/.aura/skills)
---
Write the rules this agent follows here.
"
    )
}

#[cfg(test)]
mod tests;
