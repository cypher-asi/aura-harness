//! Tool calls that wait for the user's answer.
//!
//! The kernel asks [`TerminalApprovals`] about every call its policy sets
//! to `ask`. The prompt goes to the chat of the agent that made the call
//! (agents run turns in parallel, each waiting on its own calls), and the
//! call's tool future waits here until the user answers, the turn is
//! cancelled, or [`TerminalApprovals::timeout`] passes. Both of the last
//! two deny the call once, so a turn never hangs.
//!
//! "Always allow" answers become [`ApprovalRule`]s for the agent's project
//! folder, kept in `<data_dir>/approvals.json`. They let calls through
//! without a prompt at the `ask` approval level; `strict` ignores them and
//! offers no "always allow". At `yolo` the kernel asks only about commands
//! it cannot confine to the agent's folder, and those that plainly stay
//! in it run without a prompt too.

use super::run_command::{is_user_tool_use, RUN_COMMAND_TOOL, SHELL_SYNTAX};
use async_trait::async_trait;
use aura_agent_kernel::{
    PendingToolPrompt, ToolApprovalError, ToolApprovalPrompter, ToolApprovalRemember,
    ToolApprovalResponse,
};
use aura_core_types::text::truncate_with_ellipsis;
use aura_core_types::{AgentId, ToolState};
use aura_ui::{ApprovalLevel, UiCommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// File under the data directory that keeps the "always allow" rules.
const RULES_FILE: &str = "approvals.json";

/// How long an unanswered prompt waits before the call is denied.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long a prompt may take to reach the UI. The command channel only
/// stays full while the UI is gone.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest description of a call shown in the prompt.
const MAX_DESCRIPTION_CHARS: usize = 400;

/// What the user answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// Run this call.
    Once,
    /// Run this call and calls like it in this project from now on.
    Always,
    /// Don't run it.
    Deny,
}

/// Why an answer could not be applied.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum AnswerError {
    /// The call stopped waiting (answered, timed out, or cancelled).
    #[error("that approval is no longer waiting")]
    NotWaiting,
}

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
    fn label(&self) -> String {
        match &self.command {
            Some(prefix) => format!("`{}` commands", prefix.join(" ")),
            None => self.tool.clone(),
        }
    }
}

/// The words of a `run_command` call's command line. `None` for a shell
/// script that uses shell syntax: `cargo test && rm -rf ~` must not pass
/// for a `cargo test` command.
fn command_words(args: &Value) -> Option<Vec<String>> {
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
fn stays_in_folder(args: &Value, folder: &Path) -> bool {
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
struct ProjectRules {
    path: PathBuf,
    file: RulesFile,
}

impl ProjectRules {
    /// Read the rules in `data_dir`. A missing file means no rules; an
    /// unreadable one is logged and treated the same.
    fn load(data_dir: &Path) -> Self {
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

    fn allows(&self, folder: &Path, tool: &str, args: &Value) -> bool {
        self.file
            .projects
            .get(&project_key(folder))
            .is_some_and(|rules| rules.iter().any(|rule| rule.allows(tool, args)))
    }

    /// Labels of the rules kept for `folder`, in the order they were added.
    fn labels(&self, folder: &Path) -> Vec<String> {
        self.file
            .projects
            .get(&project_key(folder))
            .map(|rules| rules.iter().map(ApprovalRule::label).collect())
            .unwrap_or_default()
    }

    /// Add `rule` for `folder`, returning the file contents to save, or
    /// `None` when the rule was already there.
    fn add(&mut self, folder: &Path, rule: ApprovalRule) -> Option<Vec<u8>> {
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

/// A call waiting for its answer.
#[derive(Debug)]
struct Waiting {
    agent_id: AgentId,
    tool: String,
    args: Value,
    reply: oneshot::Sender<ToolState>,
}

/// The terminal's [`ToolApprovalPrompter`]: asks in the chat of the agent
/// that made the call, and remembers "always allow" per project.
#[derive(Debug)]
pub struct TerminalApprovals {
    commands: mpsc::Sender<UiCommand>,
    /// Calls waiting for an answer, by request id
    pending: Mutex<HashMap<String, Waiting>>,
    rules: Mutex<ProjectRules>,
    /// Project folder of each agent with a kernel
    folders: Mutex<HashMap<AgentId, PathBuf>>,
    /// Approval level of each agent the hub set one for
    levels: Mutex<HashMap<AgentId, ApprovalLevel>>,
    /// Approval level of every other agent
    default_level: Mutex<ApprovalLevel>,
    timeout: Duration,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TerminalApprovals {
    /// Prompts go to `commands`; rules are kept in `data_dir`. Unanswered
    /// prompts are denied after `AURA_TUI_APPROVAL_TIMEOUT_SECS` (default
    /// ten minutes).
    #[must_use]
    pub fn new(commands: mpsc::Sender<UiCommand>, data_dir: &Path) -> Self {
        let timeout = std::env::var("AURA_TUI_APPROVAL_TIMEOUT_SECS")
            .ok()
            .and_then(|raw| {
                let parsed = raw.trim().parse::<u64>().ok().filter(|secs| *secs > 0);
                if parsed.is_none() {
                    warn!(value = %raw, "Ignoring unreadable AURA_TUI_APPROVAL_TIMEOUT_SECS");
                }
                parsed
            })
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
        Self::with_timeout(commands, data_dir, timeout)
    }

    /// [`Self::new`] with an explicit timeout.
    #[must_use]
    pub fn with_timeout(
        commands: mpsc::Sender<UiCommand>,
        data_dir: &Path,
        timeout: Duration,
    ) -> Self {
        Self {
            commands,
            pending: Mutex::new(HashMap::new()),
            rules: Mutex::new(ProjectRules::load(data_dir)),
            folders: Mutex::new(HashMap::new()),
            levels: Mutex::new(HashMap::new()),
            default_level: Mutex::new(ApprovalLevel::default()),
            timeout,
        }
    }

    /// Note that `agent_id` is at approval `level` from now on.
    pub fn set_level(&self, agent_id: AgentId, level: ApprovalLevel) {
        lock(&self.levels).insert(agent_id, level);
    }

    /// The approval level of agents [`Self::set_level`] was not called
    /// for.
    pub fn set_default_level(&self, level: ApprovalLevel) {
        *lock(&self.default_level) = level;
    }

    fn level_of(&self, agent_id: AgentId) -> ApprovalLevel {
        lock(&self.levels)
            .get(&agent_id)
            .copied()
            .unwrap_or_else(|| *lock(&self.default_level))
    }

    /// How long a prompt waits before its call is denied.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Note the project folder `agent_id` works in, which its "always
    /// allow" rules belong to.
    pub fn set_folder(&self, agent_id: AgentId, folder: PathBuf) {
        lock(&self.folders).insert(agent_id, folder);
    }

    fn folder_of(&self, agent_id: AgentId) -> Option<PathBuf> {
        lock(&self.folders).get(&agent_id).cloned()
    }

    /// What the "always allow" rules of `agent_id`'s project folder allow,
    /// as the prompt words them ("`cargo test` commands"). Empty when the
    /// agent has no folder or its project no rules.
    pub(crate) fn rules_for(&self, agent_id: AgentId) -> Vec<String> {
        self.folder_of(agent_id)
            .map(|folder| lock(&self.rules).labels(&folder))
            .unwrap_or_default()
    }

    /// Whether any call of `agent_id` waits for an answer.
    #[must_use]
    pub fn is_waiting(&self, agent_id: AgentId) -> bool {
        lock(&self.pending)
            .values()
            .any(|waiting| waiting.agent_id == agent_id)
    }

    /// Apply the user's answer to the call waiting as `request_id`,
    /// returning the agent that made the call.
    pub(crate) async fn answer(
        &self,
        request_id: &str,
        answer: Answer,
    ) -> Result<AgentId, AnswerError> {
        let waiting = lock(&self.pending)
            .remove(request_id)
            .ok_or(AnswerError::NotWaiting)?;
        if answer == Answer::Always {
            self.remember(&waiting).await;
        }
        let decision = match answer {
            Answer::Once | Answer::Always => ToolState::Allow,
            Answer::Deny => ToolState::Deny,
        };
        let agent_id = waiting.agent_id;
        // The call may have been dropped meanwhile; nothing then waits.
        let _ = waiting.reply.send(decision);
        if answer == Answer::Always {
            self.allow_covered(agent_id).await;
        }
        Ok(agent_id)
    }

    /// Allow the calls of `agent_id` still waiting that its project's rules
    /// now cover, e.g. the other edits of a batch after "always allow".
    async fn allow_covered(&self, agent_id: AgentId) {
        let Some(folder) = self.folder_of(agent_id) else {
            return;
        };
        let covered: Vec<(String, Waiting)> = {
            let rules = lock(&self.rules);
            let mut pending = lock(&self.pending);
            let ids: Vec<String> = pending
                .iter()
                .filter(|(_, waiting)| {
                    waiting.agent_id == agent_id
                        && rules.allows(&folder, &waiting.tool, &waiting.args)
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| pending.remove(&id).map(|waiting| (id, waiting)))
                .collect()
        };
        for (id, waiting) in covered {
            info!(tool = %waiting.tool, "Allowed a waiting call by the new approval rule");
            let _ = waiting.reply.send(ToolState::Allow);
            self.close_prompt(agent_id, id).await;
        }
    }

    /// Keep an "always allow" rule for the call's project.
    async fn remember(&self, waiting: &Waiting) {
        let Some(rule) = ApprovalRule::for_call(&waiting.tool, &waiting.args) else {
            return;
        };
        let Some(folder) = self.folder_of(waiting.agent_id) else {
            warn!(agent = %waiting.agent_id, "No project folder to keep the approval rule for");
            return;
        };
        let (path, bytes) = {
            let mut rules = lock(&self.rules);
            let Some(bytes) = rules.add(&folder, rule) else {
                return;
            };
            (rules.path.clone(), bytes)
        };
        if let Err(e) = write_atomically(&path, &bytes).await {
            warn!(path = %path.display(), error = %e, "Could not save approval rules");
            let _ = self
                .commands
                .send(UiCommand::ShowWarning(format!(
                    "Allowed for this session only: could not save {}: {e}",
                    path.display()
                )))
                .await;
        }
    }

    /// Deny every call `agent_id` is waiting on, e.g. when its turn is
    /// cancelled.
    pub async fn cancel_agent(&self, agent_id: AgentId) {
        let cancelled: Vec<(String, Waiting)> = {
            let mut pending = lock(&self.pending);
            let ids: Vec<String> = pending
                .iter()
                .filter(|(_, waiting)| waiting.agent_id == agent_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| pending.remove(&id).map(|waiting| (id, waiting)))
                .collect()
        };
        for (id, waiting) in cancelled {
            debug!(%agent_id, request = %id, "Denying a waiting call of a cancelled turn");
            let _ = waiting.reply.send(ToolState::Deny);
            self.close_prompt(agent_id, id).await;
        }
    }

    /// Deny every waiting call, e.g. when the terminal quits.
    pub fn cancel_all(&self) {
        for (_, waiting) in lock(&self.pending).drain() {
            let _ = waiting.reply.send(ToolState::Deny);
        }
    }

    async fn close_prompt(&self, agent_id: AgentId, request_id: String) {
        let _ = self
            .commands
            .send(UiCommand::ForAgent {
                agent_id: agent_id.to_hex(),
                command: Box::new(UiCommand::CloseApproval(request_id)),
            })
            .await;
    }

    /// Wait for the answer to `request_id`, denying it once the timeout
    /// passes.
    async fn wait(
        &self,
        prompt: &PendingToolPrompt,
        reply: oneshot::Receiver<ToolState>,
    ) -> ToolState {
        match tokio::time::timeout(self.timeout, reply).await {
            Ok(Ok(decision)) => decision,
            // Dropped by `cancel_all`: the terminal is quitting.
            Ok(Err(_)) => ToolState::Deny,
            Err(_) => {
                lock(&self.pending).remove(&prompt.request_id);
                warn!(
                    tool = %prompt.tool_name,
                    timeout_secs = self.timeout.as_secs(),
                    "No answer to the approval prompt; denying the call"
                );
                self.close_prompt(prompt.agent_id, prompt.request_id.clone())
                    .await;
                let _ = self
                    .commands
                    .send(UiCommand::ForAgent {
                        agent_id: prompt.agent_id.to_hex(),
                        command: Box::new(UiCommand::ShowWarning(format!(
                            "No answer within {}s; denied {}",
                            self.timeout.as_secs(),
                            prompt.tool_name
                        ))),
                    })
                    .await;
                ToolState::Deny
            }
        }
    }
}

const fn once(decision: ToolState) -> ToolApprovalResponse {
    ToolApprovalResponse {
        decision,
        remember: ToolApprovalRemember::Once,
    }
}

#[async_trait]
impl ToolApprovalPrompter for TerminalApprovals {
    async fn prompt(
        &self,
        prompt: PendingToolPrompt,
    ) -> Result<ToolApprovalResponse, ToolApprovalError> {
        // Commands the user typed in the chat are already their decision.
        // The kernel's request id is `<agent hex>:<tool use id>`.
        let typed = prompt
            .request_id
            .split_once(':')
            .is_some_and(|(_, tool_use_id)| is_user_tool_use(tool_use_id));
        if typed {
            return Ok(once(ToolState::Allow));
        }
        let folder = self.folder_of(prompt.agent_id);
        let level = self.level_of(prompt.agent_id);
        if level == ApprovalLevel::Yolo
            && prompt.tool_name == RUN_COMMAND_TOOL
            && folder
                .as_deref()
                .is_some_and(|folder| stays_in_folder(&prompt.args, folder))
        {
            info!("Allowed a command that stays in the project folder (yolo)");
            return Ok(once(ToolState::Allow));
        }
        let ruled = level != ApprovalLevel::Strict
            && folder.as_deref().is_some_and(|folder| {
                lock(&self.rules).allows(folder, &prompt.tool_name, &prompt.args)
            });
        if ruled {
            info!(tool = %prompt.tool_name, "Allowed by a project approval rule");
            return Ok(once(ToolState::Allow));
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        lock(&self.pending).insert(
            prompt.request_id.clone(),
            Waiting {
                agent_id: prompt.agent_id,
                tool: prompt.tool_name.clone(),
                args: prompt.args.clone(),
                reply: reply_tx,
            },
        );
        let always = match (
            ApprovalRule::for_call(&prompt.tool_name, &prompt.args),
            folder,
        ) {
            (Some(rule), Some(_)) if level != ApprovalLevel::Strict => {
                format!("{} in this project", rule.label())
            }
            _ => String::new(),
        };
        let request = UiCommand::ForAgent {
            agent_id: prompt.agent_id.to_hex(),
            command: Box::new(UiCommand::RequestApproval {
                id: prompt.request_id.clone(),
                tool: prompt.tool_name.clone(),
                description: describe(&prompt.tool_name, &prompt.args),
                always,
            }),
        };
        let delivered = tokio::time::timeout(DELIVERY_TIMEOUT, self.commands.send(request)).await;
        if !matches!(delivered, Ok(Ok(()))) {
            lock(&self.pending).remove(&prompt.request_id);
            return Err(ToolApprovalError::DeliveryFailed);
        }
        Ok(once(self.wait(&prompt, reply_rx).await))
    }
}

/// What a call would do, in the words the prompt shows.
fn describe(tool: &str, args: &Value) -> String {
    let text = if tool == RUN_COMMAND_TOOL {
        describe_command(args)
    } else if let Some(path) = args.get("path").and_then(Value::as_str) {
        path.to_string()
    } else {
        args.to_string()
    };
    truncate_with_ellipsis(&text, MAX_DESCRIPTION_CHARS)
}

fn describe_command(args: &Value) -> String {
    let line = args
        .get("shell_script")
        .or_else(|| args.get("command"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| command_words(args).map(|words| words.join(" ")))
        .unwrap_or_else(|| args.to_string());
    let mut notes = Vec::new();
    if let Some(cwd) = args
        .get("cwd")
        .or_else(|| args.get("working_dir"))
        .and_then(Value::as_str)
    {
        notes.push(format!("in {cwd}"));
    }
    if args.get("background").and_then(Value::as_bool) == Some(true) {
        notes.push("in the background".to_string());
    }
    if notes.is_empty() {
        line
    } else {
        format!("{line} ({})", notes.join(", "))
    }
}

/// Write `bytes` to `path` through a temporary file, so a crash never
/// leaves half a rules file.
async fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

#[cfg(test)]
mod tests;
