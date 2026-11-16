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
use aura_exec_tools::agents::task::TASK_TOOL_NAME;
use aura_model::ToolDefinition;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

const DEFINITIONS_DIR: &str = "agent_defs";
const MAX_NAME_LEN: usize = 40;
const MAX_FILE_BYTES: u64 = 64 * 1024;
const NEW_DEFINITION_NAME: &str = "new-agent";

/// Names of the seeded definitions already added once, one per line.
const SEEDED_FILE: &str = ".seeded";

/// Built-in definitions, each added once to a roster; deleting one never
/// re-adds it. The specialists match AURA's default agent roster.
const SEED_DEFINITIONS: &[(&str, &str)] = &[
    ("explorer", include_str!("agent_definitions/explorer.md")),
    ("reviewer", include_str!("agent_definitions/reviewer.md")),
    ("engineer", include_str!("agent_definitions/engineer.md")),
    (
        "rust-engineer",
        include_str!("agent_definitions/rust-engineer.md"),
    ),
    (
        "frontend-engineer",
        include_str!("agent_definitions/frontend-engineer.md"),
    ),
    (
        "ui-designer",
        include_str!("agent_definitions/ui-designer.md"),
    ),
    (
        "web-designer",
        include_str!("agent_definitions/web-designer.md"),
    ),
    (
        "product-manager",
        include_str!("agent_definitions/product-manager.md"),
    ),
];

/// Seeds written by versions that seeded only when creating the directory.
const LEGACY_SEEDS: &[&str] = &["explorer", "reviewer"];

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
    /// for good.
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
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, "", serde_json::json!({"type": "object"}))
    }

    #[test]
    fn parses_frontmatter_and_rules() {
        let content = "---\nname: reviewer\ndescription: Reviews diffs.\nmodel: claude-x\n\
                       tools: [read_file]\nskills: [deploy]\n---\nBe strict.\n\nCite files.\n";
        let def = AgentDefinition::parse(content, Path::new("reviewer.md")).unwrap();
        assert_eq!(def.name, "reviewer");
        assert_eq!(def.description, "Reviews diffs.");
        assert_eq!(def.model.as_deref(), Some("claude-x"));
        assert_eq!(def.tools, Some(vec!["read_file".to_string()]));
        assert_eq!(def.skills, vec!["deploy".to_string()]);
        assert_eq!(def.rules, "Be strict.\n\nCite files.");
    }

    #[test]
    fn rejects_missing_frontmatter_and_bad_names() {
        assert!(AgentDefinition::parse("no frontmatter", Path::new("x.md")).is_err());
        assert!(AgentDefinition::parse("---\nname: x\n", Path::new("x.md")).is_err());
        assert!(AgentDefinition::parse("---\nname: Bad Name\n---\n", Path::new("x.md")).is_err());
        assert!(validate_name("ok-name_2").is_ok());
        assert!(validate_name("-leading").is_err());
        assert!(validate_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    const SEEDED_NAMES: [&str; 8] = [
        "engineer",
        "explorer",
        "frontend-engineer",
        "product-manager",
        "reviewer",
        "rust-engineer",
        "ui-designer",
        "web-designer",
    ];

    fn names(defs: &AgentDefinitions) -> Vec<&str> {
        defs.all().iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn every_seed_parses_under_its_own_name() {
        for (name, content) in SEED_DEFINITIONS {
            let def = AgentDefinition::parse(content, Path::new("seed.md"))
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(def.name, *name);
            assert!(!def.description.is_empty(), "{name} has no description");
            assert!(!def.rules.is_empty(), "{name} has no rules");
        }
    }

    #[test]
    fn seeds_starter_definitions_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
        assert_eq!(names(&defs), SEEDED_NAMES);

        defs.delete("explorer").unwrap();
        defs.delete("ui-designer").unwrap();
        let reloaded = AgentDefinitions::load_or_seed(dir.path()).unwrap();
        assert_eq!(reloaded.all().len(), SEEDED_NAMES.len() - 2);
        assert!(reloaded.get("explorer").is_none());
        assert!(reloaded.get("ui-designer").is_none());
        assert!(reloaded.get("REVIEWER").is_some());
    }

    #[test]
    fn seeds_new_defaults_into_a_directory_from_before_the_seed_list() {
        let dir = tempfile::tempdir().unwrap();
        let defs_dir = dir.path().join(DEFINITIONS_DIR);
        fs::create_dir_all(&defs_dir).unwrap();
        fs::write(
            defs_dir.join("reviewer.md"),
            "---\nname: reviewer\n---\nMine.\n",
        )
        .unwrap();
        fs::write(
            defs_dir.join("web.md"),
            "---\nname: web-designer\n---\nMine.\n",
        )
        .unwrap();

        let defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
        assert!(
            defs.get("explorer").is_none(),
            "a deleted legacy seed came back"
        );
        assert_eq!(defs.get("reviewer").unwrap().rules, "Mine.");
        assert_eq!(defs.get("web-designer").unwrap().rules, "Mine.");
        assert!(!defs_dir.join("web-designer.md").exists());
        for name in [
            "engineer",
            "rust-engineer",
            "frontend-engineer",
            "ui-designer",
            "product-manager",
        ] {
            assert!(defs.get(name).is_some(), "missing {name}");
        }
    }

    #[test]
    fn create_writes_a_loadable_skeleton_with_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
        let first = defs.create(None).unwrap();
        let second = defs.create(None).unwrap();
        assert!(first.ends_with("new-agent.md"));
        assert!(second.ends_with("new-agent-2.md"));
        assert!(defs.get("new-agent-2").is_some());

        defs.create(Some("Planner")).unwrap();
        assert!(defs.get("planner").is_some());
        assert!(defs.create(Some("planner")).is_err());
        assert!(defs.create(Some("not valid")).is_err());
    }

    #[test]
    fn reload_reports_broken_and_duplicate_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
        let defs_dir = dir.path().join(DEFINITIONS_DIR);
        fs::write(defs_dir.join("broken.md"), "not a definition").unwrap();
        fs::write(defs_dir.join("zz-copy.md"), "---\nname: reviewer\n---\n").unwrap();

        let problems = defs.reload();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(defs.all().len(), SEEDED_NAMES.len());
    }

    #[test]
    fn filter_tools_honors_the_allow_list_and_hides_task_by_default() {
        let tools = [tool("read_file"), tool("write_file"), tool("task")];
        let mut def = AgentDefinition::parse("---\nname: a\n---\n", Path::new("a.md")).unwrap();
        let names = |def: &AgentDefinition| -> Vec<String> {
            def.filter_tools(&tools)
                .into_iter()
                .map(|t| t.name)
                .collect()
        };
        assert_eq!(names(&def), ["read_file", "write_file"]);

        def.tools = Some(vec!["read_file".into(), "task".into()]);
        assert_eq!(names(&def), ["read_file", "task"]);
    }

    #[test]
    fn system_prompt_includes_rules() {
        let def = AgentDefinition::parse(
            "---\nname: a\ndescription: Does A.\nskills: [missing]\n---\nRule one.\n",
            Path::new("a.md"),
        )
        .unwrap();
        let skills = SkillManager::new(aura_context_skills::SkillLoader::new(
            aura_context_skills::loader::SkillLoaderConfig::default(),
        ));
        let mut prompt = "base".to_string();
        def.extend_system_prompt(&mut prompt, &skills);
        assert!(prompt.starts_with("base\n\n<agent_definition name=\"a\">"));
        assert!(prompt.contains("Your role: Does A."));
        assert!(prompt.contains("Rule one."));
    }
}
