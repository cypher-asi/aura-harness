//! Conversation and session state, driven by [`UiCommand`]s.
//!
//! [`Model::apply`] mirrors how the terminal's `App::process_command`
//! treats each command, so both frontends show the same conversation for
//! the same stream. Nothing here draws; the views read it.

use aura_ui_core::events::{
    ActivityData, AgentDefinitionSummary, AgentSummary, CallUsage, MessageData, MessageRole,
    ModelOptionView, ProcessSummary, RecordSummary, UiCommand,
};
use aura_ui_core::format::format_tool_result;
use aura_ui_core::sandbox::SandboxTree;
use chrono::{DateTime, Local};
use iced::widget::markdown;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Conversation entries kept; every tool call and thinking block counts.
pub const MAX_ENTRIES: usize = 2000;
/// Records kept in the Records panel.
pub const MAX_RECORDS: usize = 100;
/// How long a toast stays in the status bar.
pub const TOAST_TTL: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Processing,
    AwaitingApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Success,
    Error,
    Interrupted,
}

pub struct Reply {
    pub raw: String,
    pub markdown: markdown::Content,
    pub streaming: bool,
    pub model: Option<String>,
    pub usage: Option<CallUsage>,
}

impl Reply {
    fn new(text: &str, streaming: bool) -> Self {
        Self {
            raw: text.to_string(),
            markdown: markdown::Content::parse(text),
            streaming,
            model: None,
            usage: None,
        }
    }

    fn push(&mut self, text: &str) {
        self.raw.push_str(text);
        self.markdown.push_str(text);
    }
}

pub struct Thinking {
    pub text: String,
    pub running: bool,
    pub started: Option<Instant>,
    pub duration_ms: Option<u64>,
}

pub struct Tool {
    pub id: String,
    pub name: String,
    pub args: String,
    pub result: Option<String>,
    pub status: ToolStatus,
    pub policy: Option<String>,
    pub retry: Option<(u32, u32)>,
    pub started: Option<Instant>,
    pub duration_ms: Option<u64>,
}

pub struct Shell {
    pub command: String,
    pub output: Option<String>,
    pub exit_code: Option<i32>,
}

pub enum Body {
    User(String),
    Assistant(Box<Reply>),
    System { text: String, level: Level },
    Thinking(Thinking),
    Tool(Tool),
    Shell(Shell),
}

pub struct Entry {
    pub id: u64,
    pub at: DateTime<Local>,
    pub body: Body,
    /// Whether an activity block (thinking, tool) shows its details
    pub expanded: bool,
}

impl Entry {
    #[must_use]
    pub fn is_activity(&self) -> bool {
        matches!(self.body, Body::Thinking(_) | Body::Tool(_))
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        match &self.body {
            Body::Thinking(t) => t.running,
            Body::Tool(t) => t.status == ToolStatus::Running,
            Body::Shell(s) => s.output.is_none(),
            Body::Assistant(r) => r.streaming,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Approval {
    pub id: String,
    pub tool: String,
    pub description: String,
}

/// What the app has to do after a command, beyond redrawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Open this file in the user's editor and report back when done
    OpenEditor(PathBuf),
    /// A model picked before its sign-in can now be selected
    ChooseModel(String),
    /// The sandbox root changed or files may have; re-read the tree
    RefreshSandbox,
}

#[derive(Default)]
pub struct Totals {
    pub tokens: u64,
    pub cost_usd: f64,
    pub calls: u32,
}

pub struct Model {
    pub entries: VecDeque<Entry>,
    next_id: u64,
    pub phase: Phase,
    pub status: String,
    pub approval: Option<Approval>,
    pub toast: Option<(String, Level, Instant)>,
    pub records: VecDeque<RecordSummary>,
    pub agents: Vec<AgentSummary>,
    pub active_agent: String,
    pub definitions: Vec<AgentDefinitionSummary>,
    pub processes: Vec<ProcessSummary>,
    pub models: Vec<ModelOptionView>,
    pub current_model: String,
    pub user: Option<String>,
    pub plans: Vec<String>,
    pub api_url: Option<String>,
    pub api_active: bool,
    pub sandbox: SandboxTree,
    /// Provider whose pasted sign-in code is awaited
    pub link_prompt: Option<String>,
    /// Model to select once its sign-in succeeds
    pub pending_model: Option<String>,
    /// Whether new activity blocks start expanded
    pub expand_activity: bool,
    pub totals: Totals,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            next_id: 0,
            phase: Phase::Idle,
            status: "Ready".to_string(),
            approval: None,
            toast: None,
            records: VecDeque::new(),
            agents: Vec::new(),
            active_agent: String::new(),
            definitions: Vec::new(),
            processes: Vec::new(),
            models: Vec::new(),
            current_model: String::new(),
            user: None,
            plans: Vec::new(),
            api_url: None,
            api_active: false,
            sandbox: SandboxTree::default(),
            link_prompt: None,
            pending_model: None,
            expand_activity: false,
            totals: Totals::default(),
        }
    }
}

impl Model {
    /// Apply one command from the event loop.
    pub fn apply(&mut self, cmd: UiCommand) -> Option<Effect> {
        match cmd {
            UiCommand::SetStatus(status) => self.status = status,
            UiCommand::StartStreaming => {
                self.push(Body::Assistant(Box::new(Reply::new("", true))));
            }
            UiCommand::AppendText(text) => {
                if let Some(Body::Assistant(reply)) = self.entries.back_mut().map(|e| &mut e.body) {
                    if reply.streaming {
                        reply.push(&text);
                    }
                }
            }
            UiCommand::FinishStreaming => self.finish_streaming(),
            UiCommand::SetReplyModel { model, usage } => self.set_reply_model(&model, usage),
            UiCommand::StartThinking => self.push(Body::Thinking(Thinking {
                text: String::new(),
                running: true,
                started: Some(Instant::now()),
                duration_ms: None,
            })),
            UiCommand::AppendThinking(text) => {
                if self.live_thinking().is_none() {
                    self.apply(UiCommand::StartThinking);
                }
                if let Some(thinking) = self.live_thinking() {
                    thinking.text.push_str(&text);
                }
            }
            UiCommand::FinishThinking => {
                if let Some(thinking) = self.live_thinking() {
                    finish_thinking(thinking);
                }
            }
            UiCommand::ShowMessage(data) => self.show_message(&data),
            UiCommand::ShowTool(data) => self.push(Body::Tool(Tool {
                id: data.id,
                name: data.name,
                args: data.args,
                result: None,
                status: ToolStatus::Running,
                policy: None,
                retry: None,
                started: Some(Instant::now()),
                duration_ms: None,
            })),
            UiCommand::UpdateToolArgs { id, args } => {
                self.update_tool(&id, |tool| tool.args = args);
            }
            UiCommand::ToolPolicy { id, tool, decision } => {
                let line = format!("policy: {tool} {decision}");
                if !self.update_tool(&id, |step| step.policy = Some(decision)) {
                    self.push_system(&line, Level::Info);
                }
            }
            UiCommand::ToolRetrying {
                id,
                attempt,
                max_attempts,
            } => {
                self.update_tool(&id, |tool| tool.retry = Some((attempt, max_attempts)));
            }
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => {
                self.update_tool(&id, |tool| {
                    tool.result = Some(format_tool_result(&tool.name, &result));
                    tool.status = if success {
                        ToolStatus::Success
                    } else {
                        ToolStatus::Error
                    };
                    tool.retry = None;
                    tool.duration_ms = elapsed_ms(tool.started);
                });
                return Some(Effect::RefreshSandbox);
            }
            UiCommand::CommandFinished { output, exit_code } => {
                let running = self
                    .entries
                    .iter_mut()
                    .rev()
                    .find_map(|e| match &mut e.body {
                        Body::Shell(shell) if shell.output.is_none() => Some(shell),
                        _ => None,
                    });
                if let Some(shell) = running {
                    shell.output = Some(output);
                    shell.exit_code = exit_code;
                }
                return Some(Effect::RefreshSandbox);
            }
            UiCommand::RequestApproval {
                id,
                tool,
                description,
            } => {
                self.approval = Some(Approval {
                    id,
                    tool,
                    description,
                });
                self.phase = Phase::AwaitingApproval;
            }
            UiCommand::ShowError(msg) => self.notify(&msg, "error: ", Level::Error),
            UiCommand::ShowSuccess(msg) => self.notify(&msg, "✓ ", Level::Success),
            UiCommand::ShowWarning(msg) => self.notify(&msg, "warning: ", Level::Warning),
            UiCommand::TurnStarted => {
                if self.phase == Phase::Idle {
                    self.phase = Phase::Processing;
                    self.status = "Thinking...".to_string();
                }
            }
            UiCommand::Complete => {
                self.finish_turn();
                return Some(Effect::RefreshSandbox);
            }
            UiCommand::ClearConversation => self.entries.clear(),
            UiCommand::NewRecord(record) => {
                self.records.push_back(record);
                while self.records.len() > MAX_RECORDS {
                    self.records.pop_front();
                }
            }
            UiCommand::SetAgents(agents) => {
                self.agents = agents;
                self.mark_active();
            }
            UiCommand::SetActiveAgent(id) => {
                self.active_agent = id;
                self.mark_active();
            }
            UiCommand::SetAgentDefinitions(definitions) => self.definitions = definitions,
            UiCommand::OpenEditor(path) => return Some(Effect::OpenEditor(path)),
            UiCommand::ClearRecords => self.records.clear(),
            UiCommand::LoadConversation(history) => self.load(history),
            UiCommand::SetSandboxRoot(root) => {
                self.sandbox.set_root(root);
                return Some(Effect::RefreshSandbox);
            }
            UiCommand::SetProcesses(processes) => self.processes = processes,
            UiCommand::SetApiStatus { url, active } => {
                self.api_url = url;
                self.api_active = active;
            }
            UiCommand::SetModels {
                options,
                current,
                user,
                plans,
            } => {
                self.models = options;
                self.current_model = current;
                self.user = user;
                self.plans = plans;
                let ready = self
                    .pending_model
                    .as_deref()
                    .is_some_and(|id| self.models.iter().any(|m| m.id == id && m.available));
                if ready {
                    return self.pending_model.take().map(Effect::ChooseModel);
                }
            }
            UiCommand::PromptLinkCode { provider } => {
                self.status = format!("{provider} — paste the code from the sign-in page");
                self.link_prompt = Some(provider);
            }
            UiCommand::EndLink => {
                self.pending_model = None;
                self.link_prompt = None;
            }
        }
        None
    }

    /// The user sent a chat message.
    pub fn send_user(&mut self, text: &str) {
        self.push(Body::User(text.to_string()));
        self.phase = Phase::Processing;
        self.status = "Thinking...".to_string();
    }

    /// The user ran a shell command.
    pub fn send_command(&mut self, command: &str) {
        self.push(Body::Shell(Shell {
            command: command.to_string(),
            output: None,
            exit_code: None,
        }));
        self.phase = Phase::Processing;
        self.status = "Running...".to_string();
    }

    pub fn resolve_approval(&mut self, approved: bool) -> Option<Approval> {
        let approval = self.approval.take()?;
        self.phase = Phase::Processing;
        self.status = if approved {
            "Approved, continuing...".to_string()
        } else {
            "Denied".to_string()
        };
        Some(approval)
    }

    pub fn push_system(&mut self, text: &str, level: Level) {
        self.push(Body::System {
            text: text.to_string(),
            level,
        });
    }

    /// A message for the status bar only.
    pub fn toast(&mut self, text: impl Into<String>, level: Level) {
        self.toast = Some((text.into(), level, Instant::now()));
    }

    pub fn expire_toast(&mut self, now: Instant) {
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, _, at)| now.duration_since(*at) >= TOAST_TTL)
        {
            self.toast = None;
        }
    }

    /// Whether anything on screen moves on its own (spinners, the aura).
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.phase != Phase::Idle
            || self.entries.iter().rev().take(64).any(Entry::is_live)
            || self.processes.iter().any(|p| p.status.is_live())
    }

    pub fn toggle(&mut self, id: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.expanded = !entry.expanded;
        }
    }

    #[must_use]
    pub fn active_agent(&self) -> Option<&AgentSummary> {
        self.agents.iter().find(|a| a.id == self.active_agent)
    }

    fn push(&mut self, body: Body) {
        let expanded = matches!(body, Body::Thinking(_) | Body::Tool(_)) && self.expand_activity;
        self.entries.push_back(Entry {
            id: self.next_id,
            at: Local::now(),
            body,
            expanded,
        });
        self.next_id += 1;
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
    }

    fn notify(&mut self, msg: &str, prefix: &str, level: Level) {
        self.push_system(&format!("{prefix}{msg}"), level);
        self.toast(msg, level);
    }

    fn finish_streaming(&mut self) {
        let empty = match self.entries.back_mut().map(|e| &mut e.body) {
            Some(Body::Assistant(reply)) if reply.streaming => {
                reply.streaming = false;
                reply.raw.is_empty()
            }
            _ => false,
        };
        if empty {
            self.entries.pop_back();
        }
    }

    fn set_reply_model(&mut self, model: &str, usage: Option<CallUsage>) {
        let replies = self
            .entries
            .iter_mut()
            .rev()
            .take_while(|e| !matches!(e.body, Body::User(_)))
            .filter_map(|e| match &mut e.body {
                Body::Assistant(reply) if reply.model.is_none() => Some(reply),
                _ => None,
            });
        for reply in replies {
            reply.model = Some(model.to_string());
            reply.usage = usage;
        }
        if let Some(usage) = usage {
            self.totals.tokens += usage.tokens;
            self.totals.cost_usd += usage.cost_usd.unwrap_or(0.0);
            self.totals.calls += 1;
        }
    }

    fn show_message(&mut self, data: &MessageData) {
        let body = match data.role {
            MessageRole::User => Body::User(data.content.clone()),
            MessageRole::System => Body::System {
                text: data.content.clone(),
                level: Level::Info,
            },
            MessageRole::Assistant => {
                let mut reply = Reply::new(&data.content, data.is_streaming);
                reply.model.clone_from(&data.model);
                reply.usage = data.usage;
                Body::Assistant(Box::new(reply))
            }
        };
        self.push(body);
    }

    fn live_thinking(&mut self) -> Option<&mut Thinking> {
        self.entries
            .iter_mut()
            .rev()
            .find_map(|e| match &mut e.body {
                Body::Thinking(t) if t.running => Some(t),
                _ => None,
            })
    }

    /// Apply `update` to the tool call `id`; `false` when it isn't shown.
    fn update_tool(&mut self, id: &str, update: impl FnOnce(&mut Tool)) -> bool {
        let tool = self
            .entries
            .iter_mut()
            .rev()
            .find_map(|e| match &mut e.body {
                Body::Tool(tool) if tool.id == id => Some(tool),
                _ => None,
            });
        match tool {
            Some(tool) => {
                update(tool);
                true
            }
            None => false,
        }
    }

    fn finish_turn(&mut self) {
        self.finish_streaming();
        for entry in &mut self.entries {
            match &mut entry.body {
                Body::Thinking(t) if t.running => finish_thinking(t),
                Body::Tool(t) if t.status == ToolStatus::Running => {
                    t.status = ToolStatus::Interrupted;
                    t.duration_ms = elapsed_ms(t.started);
                    t.retry = None;
                }
                _ => {}
            }
        }
        self.phase = Phase::Idle;
        self.approval = None;
        self.status = "Ready".to_string();
    }

    fn load(&mut self, history: Vec<MessageData>) {
        self.entries.clear();
        for data in history {
            let at = data
                .timestamp_ms
                .and_then(|ms| i64::try_from(ms).ok())
                .and_then(DateTime::from_timestamp_millis)
                .map(|t| t.with_timezone(&Local));
            let body = if let Some(activity) = &data.activity {
                match activity.clone() {
                    ActivityData::Thinking { text, duration_ms } => Body::Thinking(Thinking {
                        text,
                        running: false,
                        started: None,
                        duration_ms,
                    }),
                    ActivityData::Tool {
                        id,
                        name,
                        args,
                        result,
                        is_error,
                        duration_ms,
                        policy,
                    } => Body::Tool(Tool {
                        result: result.map(|r| format_tool_result(&name, &r)),
                        id,
                        name,
                        args,
                        status: if is_error {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Success
                        },
                        policy,
                        retry: None,
                        started: None,
                        duration_ms,
                    }),
                }
            } else if let Some(command) = &data.command {
                Body::Shell(Shell {
                    command: command.clone(),
                    output: Some(String::new()),
                    exit_code: None,
                })
            } else {
                self.show_message(&MessageData {
                    is_streaming: false,
                    ..data
                });
                if let (Some(at), Some(entry)) = (at, self.entries.back_mut()) {
                    entry.at = at;
                }
                continue;
            };
            self.push(body);
            if let (Some(at), Some(entry)) = (at, self.entries.back_mut()) {
                entry.at = at;
            }
        }
    }

    fn mark_active(&mut self) {
        for agent in &mut self.agents {
            agent.is_active = agent.id == self.active_agent;
        }
    }
}

fn finish_thinking(thinking: &mut Thinking) {
    thinking.running = false;
    thinking.duration_ms = elapsed_ms(thinking.started);
}

fn elapsed_ms(started: Option<Instant>) -> Option<u64> {
    started.map(|s| u64::try_from(s.elapsed().as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_ui_core::events::ToolData;

    fn reply_texts(model: &Model) -> Vec<String> {
        model
            .entries
            .iter()
            .filter_map(|e| match &e.body {
                Body::Assistant(r) => Some(r.raw.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn streaming_builds_one_reply() {
        let mut model = Model::default();
        model.send_user("hi");
        model.apply(UiCommand::StartStreaming);
        model.apply(UiCommand::AppendText("Hel".into()));
        model.apply(UiCommand::AppendText("lo".into()));
        model.apply(UiCommand::FinishStreaming);
        model.apply(UiCommand::Complete);
        assert_eq!(reply_texts(&model), ["Hello"]);
        assert_eq!(model.phase, Phase::Idle);
        assert_eq!(model.status, "Ready");
    }

    #[test]
    fn empty_stream_leaves_no_reply() {
        let mut model = Model::default();
        model.apply(UiCommand::StartStreaming);
        model.apply(UiCommand::FinishStreaming);
        assert!(model.entries.is_empty());
    }

    #[test]
    fn reply_model_names_every_reply_since_the_user_message() {
        let mut model = Model::default();
        model.send_user("q");
        for part in ["a", "b"] {
            model.apply(UiCommand::StartStreaming);
            model.apply(UiCommand::AppendText(part.into()));
            model.apply(UiCommand::FinishStreaming);
        }
        model.apply(UiCommand::SetReplyModel {
            model: "m1".into(),
            usage: Some(CallUsage {
                tokens: 10,
                cost_usd: Some(0.5),
            }),
        });
        let models: Vec<_> = model
            .entries
            .iter()
            .filter_map(|e| match &e.body {
                Body::Assistant(r) => r.model.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(models, ["m1", "m1"]);
        assert_eq!(model.totals.tokens, 10);
    }

    #[test]
    fn tool_lifecycle_and_policy() {
        let mut model = Model::default();
        model.apply(UiCommand::ShowTool(ToolData {
            id: "t1".into(),
            name: "read_file".into(),
            args: "{}".into(),
        }));
        model.apply(UiCommand::ToolPolicy {
            id: "t1".into(),
            tool: "read_file".into(),
            decision: "allowed by user.on".into(),
        });
        model.apply(UiCommand::ToolPolicy {
            id: "missing".into(),
            tool: "write_file".into(),
            decision: "denied".into(),
        });
        let effect = model.apply(UiCommand::CompleteTool {
            id: "t1".into(),
            result: "contents".into(),
            success: true,
        });
        assert_eq!(effect, Some(Effect::RefreshSandbox));
        let Body::Tool(tool) = &model.entries[0].body else {
            panic!("expected a tool entry");
        };
        assert_eq!(tool.status, ToolStatus::Success);
        assert_eq!(tool.policy.as_deref(), Some("allowed by user.on"));
        assert!(matches!(
            &model.entries[1].body,
            Body::System { text, .. } if text == "policy: write_file denied"
        ));
    }

    #[test]
    fn complete_interrupts_live_activity() {
        let mut model = Model::default();
        model.apply(UiCommand::AppendThinking("hmm".into()));
        model.apply(UiCommand::ShowTool(ToolData {
            id: "t".into(),
            name: "run_command".into(),
            args: "{}".into(),
        }));
        model.apply(UiCommand::Complete);
        assert!(model.entries.iter().all(|e| !e.is_live()));
        assert!(matches!(
            &model.entries[1].body,
            Body::Tool(t) if t.status == ToolStatus::Interrupted
        ));
    }

    #[test]
    fn command_output_fills_the_running_shell() {
        let mut model = Model::default();
        model.send_command("ls");
        model.apply(UiCommand::CommandFinished {
            output: "a\nb".into(),
            exit_code: None,
        });
        assert!(matches!(
            &model.entries[0].body,
            Body::Shell(s) if s.output.as_deref() == Some("a\nb")
        ));
    }

    #[test]
    fn pending_model_is_chosen_once_available() {
        let mut model = Model {
            pending_model: Some("claude/opus".into()),
            ..Model::default()
        };
        let option = |available| ModelOptionView {
            id: "claude/opus".into(),
            label: "Opus".into(),
            local: false,
            installed: false,
            group: "Claude".into(),
            provider: Some("claude".into()),
            available,
        };
        let set = |available| UiCommand::SetModels {
            options: vec![option(available)],
            current: "local/x".into(),
            user: None,
            plans: vec![],
        };
        assert_eq!(model.apply(set(false)), None);
        assert_eq!(
            model.apply(set(true)),
            Some(Effect::ChooseModel("claude/opus".into()))
        );
        assert!(model.pending_model.is_none());
    }

    #[test]
    fn load_conversation_restores_activity_and_commands() {
        let mut model = Model::default();
        model.send_user("old");
        let mut tool = MessageData::new(MessageRole::Assistant, "");
        tool.activity = Some(ActivityData::Tool {
            id: "t".into(),
            name: "list_files".into(),
            args: "{}".into(),
            result: Some("x".into()),
            is_error: false,
            duration_ms: Some(5),
            policy: None,
        });
        let mut shell = MessageData::new(MessageRole::User, "");
        shell.command = Some("git status".into());
        model.apply(UiCommand::LoadConversation(vec![
            MessageData::new(MessageRole::User, "hello"),
            tool,
            shell,
            MessageData::new(MessageRole::Assistant, "**hi**"),
        ]));
        assert_eq!(model.entries.len(), 4);
        assert!(matches!(&model.entries[1].body, Body::Tool(t) if t.duration_ms == Some(5)));
        assert!(matches!(&model.entries[2].body, Body::Shell(s) if s.command == "git status"));
        assert_eq!(reply_texts(&model), ["**hi**"]);
    }

    #[test]
    fn entries_are_capped() {
        let mut model = Model::default();
        for i in 0..MAX_ENTRIES + 10 {
            model.push_system(&i.to_string(), Level::Info);
        }
        assert_eq!(model.entries.len(), MAX_ENTRIES);
        assert!(matches!(&model.entries[0].body, Body::System { text, .. } if text == "10"));
    }
}
