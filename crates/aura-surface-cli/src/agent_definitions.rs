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
use aura_model_reasoner::ToolDefinition;
use aura_tools::agents::task::TASK_TOOL_NAME;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

const DEFINITIONS_DIR: &str = "agent_defs";
const MAX_NAME_LEN: usize = 40;
const MAX_FILE_BYTES: u64 = 64 * 1024;
const NEW_DEFINITION_NAME: &str = "new-agent";

const SEED_DEFINITIONS: &[(&str, &str)] = &[
    (
        "explorer.md",
        "---
name: explorer
description: Read-only codebase exploration. Spawn it to find files, symbols, and how something works before changing it.
tools: [list_files, read_file, stat_file, find_files, search_code, run_command]
---
Explore the codebase using read and search tools and safe verification commands only.
Do not modify files.
Return the relevant files, symbols, and conclusions.
",
    ),
    (
        "reviewer.md",
        "---
name: reviewer
description: Reviews code for bugs, regressions, and missing tests. Spawn it after making changes.
tools: [list_files, read_file, stat_file, find_files, search_code, run_command]
---
Review the requested code for correctness risks.
Lead with findings and cite concrete files and lines.
Point out behavior that is not covered by tests.
Do not modify files.
",
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
    /// Load the definitions, creating the directory with starter
    /// definitions the first time.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or seeded.
    pub fn load_or_seed(data_dir: &Path) -> anyhow::Result<Self> {
        let dir = data_dir.join(DEFINITIONS_DIR);
        if !dir.exists() {
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            for (file, content) in SEED_DEFINITIONS {
                let path = dir.join(file);
                fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
            }
        }
        let mut definitions = Self {
            dir,
            definitions: Vec::new(),
        };
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

    #[test]
    fn seeds_starter_definitions_once() {
        let dir = tempfile::tempdir().unwrap();
        let defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
        let names: Vec<&str> = defs.all().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["explorer", "reviewer"]);

        let mut defs = defs;
        defs.delete("explorer").unwrap();
        let reloaded = AgentDefinitions::load_or_seed(dir.path()).unwrap();
        assert_eq!(reloaded.all().len(), 1);
        assert!(reloaded.get("REVIEWER").is_some());
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
        assert_eq!(defs.all().len(), 2);
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
