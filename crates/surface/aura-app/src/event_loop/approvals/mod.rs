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

mod prompt;
mod rules;

pub(crate) use rules::ApprovalRule;
use rules::{write_atomically, ProjectRules};

use aura_agent_kernel::PendingToolPrompt;
use aura_core_types::{AgentId, ToolState};
use aura_ui::{ApprovalLevel, UiCommand};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// How long an unanswered prompt waits before the call is denied.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

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

#[cfg(test)]
mod tests;
