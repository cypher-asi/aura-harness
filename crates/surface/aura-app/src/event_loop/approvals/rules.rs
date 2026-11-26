//! "Always allow" rules: what a rule covers, and how the rules of each
//! project folder are kept on disk.

use super::super::run_command::{RUN_COMMAND_TOOL, SHELL_SYNTAX};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::warn;

/// File under the data directory that keeps the "always allow" rules.
const RULES_FILE: &str = "approvals.json";

/// A call the user allowed for good in one project: a tool, and for
/// `run_command` the words its command line starts with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ApprovalRule {
    pub tool: String,
    /// Leading words of the command line (`["cargo", "test"]`); only for
    /// `run_command`, which is never allowed by tool name alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

impl ApprovalRule {
    /// The rule "always allow" makes of a call, or `None` when calls like it
    /// cannot be told apart safely (a shell script with shell syntax).
    pub(crate) fn for_call(tool: &str, args: &Value) -> Option<Self> {
        let command = if tool == RUN_COMMAND_TOOL {
            Some(command_prefix(&command_words(args)?))
        } else {
            None
        };
        Some(Self {
            tool: tool.to_string(),
            command,
        })
    }

    /// Whether the rule allows calling `tool` with `args`.
    pub(crate) fn allows(&self, tool: &str, args: &Value) -> bool {
        if self.tool != tool {
            return false;
        }
        match &self.command {
            Some(prefix) => {
                command_words(args).is_some_and(|words| words.starts_with(prefix.as_slice()))
            }
            None => tool != RUN_COMMAND_TOOL,
        }
    }

    /// What the rule allows, for the prompt: "`cargo test` commands".
    pub(super) fn label(&self) -> String {
        match &self.command {
            Some(prefix) => format!("`{}` commands", prefix.join(" ")),
            None => self.tool.clone(),
        }
    }
}

/// The words of a `run_command` call's command line. `None` for a shell
/// script that uses shell syntax: `cargo test && rm -rf ~` must not pass
/// for a `cargo test` command.
pub(super) fn command_words(args: &Value) -> Option<Vec<String>> {
    if let Some(program) = args.get("program").and_then(Value::as_str) {
        let mut words = vec![program.to_string()];
        words.extend(
            args.get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
        return Some(words);
    }
    let script = args
        .get("shell_script")
        .or_else(|| args.get("command"))
        .and_then(Value::as_str)?;
    if script.contains(SHELL_SYNTAX) {
        return None;
    }
    let words: Vec<String> = script.split_whitespace().map(str::to_string).collect();
    (!words.is_empty()).then_some(words)
}

/// Whether the `run_command` call `args` plainly stays inside `folder`: no
/// shell syntax, and no argument naming a path above it or outside it.
/// Best effort, for systems that cannot confine commands: a program can
/// still write anywhere it likes.
pub(super) fn stays_in_folder(args: &Value, folder: &Path) -> bool {
    let Some(words) = command_words(args) else {
        return false;
    };
    if args
        .get("cwd")
        .or_else(|| args.get("working_dir"))
        .is_some()
    {
        return false;
    }
    let outside = |path: &str| {
        let path = Path::new(path);
        path.components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
            || (path.is_absolute() && !path.starts_with(folder) && path != Path::new("/dev/null"))
    };
    words.iter().skip(1).all(|word| {
        let value = word
            .split_once('=')
            .map_or(word.as_str(), |(_, value)| value);
        !word.starts_with('~') && !outside(word) && !outside(value)
    })
}

/// The program, plus its first argument when that reads as a subcommand
/// (`cargo test`, `git status`), so allowing `cargo test` doesn't allow
/// `cargo publish`.
fn command_prefix(words: &[String]) -> Vec<String> {
    let is_subcommand = |word: &str| {
        word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    };
    words
        .iter()
        .enumerate()
        .take_while(|(i, word)| *i == 0 || (*i == 1 && is_subcommand(word)))
        .map(|(_, word)| word.clone())
        .collect()
}

/// "Always allow" rules by project folder, as kept on disk.
#[derive(Debug, Default, Serialize, Deserialize)]
struct RulesFile {
    #[serde(default)]
    projects: BTreeMap<String, Vec<ApprovalRule>>,
}

/// The rules and where they are kept.
#[derive(Debug)]
pub(super) struct ProjectRules {
    pub(super) path: PathBuf,
    file: RulesFile,
}

impl ProjectRules {
    /// Read the rules in `data_dir`. A missing file means no rules; an
    /// unreadable one is logged and treated the same.
    pub(super) fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(RULES_FILE);
        let file = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!(path = %path.display(), error = %e, "Ignoring unreadable approval rules");
                RulesFile::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RulesFile::default(),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Could not read approval rules");
                RulesFile::default()
            }
        };
        Self { path, file }
    }

    pub(super) fn allows(&self, folder: &Path, tool: &str, args: &Value) -> bool {
        self.file
            .projects
            .get(&project_key(folder))
            .is_some_and(|rules| rules.iter().any(|rule| rule.allows(tool, args)))
    }

    /// Labels of the rules kept for `folder`, in the order they were added.
    pub(super) fn labels(&self, folder: &Path) -> Vec<String> {
        self.file
            .projects
            .get(&project_key(folder))
            .map(|rules| rules.iter().map(ApprovalRule::label).collect())
            .unwrap_or_default()
    }

    /// Add `rule` for `folder`, returning the file contents to save, or
    /// `None` when the rule was already there.
    pub(super) fn add(&mut self, folder: &Path, rule: ApprovalRule) -> Option<Vec<u8>> {
        let rules = self.file.projects.entry(project_key(folder)).or_default();
        if rules.contains(&rule) {
            return None;
        }
        rules.push(rule);
        match serde_json::to_vec_pretty(&self.file) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                warn!(error = %e, "Could not serialize approval rules");
                None
            }
        }
    }
}

fn project_key(folder: &Path) -> String {
    folder.display().to_string()
}

/// Write `bytes` to `path` through a temporary file, so a crash never
/// leaves half a rules file.
pub(super) async fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}
