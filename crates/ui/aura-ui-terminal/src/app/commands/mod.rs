//! UI command processing for the App: what the session sends.

mod agents;
mod approvals;
mod channels;
mod hud;
mod records;
mod stream;
mod tools;
mod turn;
mod workspace;

use super::{App, AppState};
use crate::events::UiCommand;
use tracing::debug;

impl App {
    fn cmd_set_models(
        &mut self,
        options: Vec<crate::events::ModelOptionView>,
        current: String,
        route: Option<String>,
        user: Option<String>,
        plans: Vec<String>,
        thinking: Option<String>,
    ) {
        self.account.models = options;
        self.account.current_model = current;
        self.account.model_route = route;
        self.account.thinking = thinking;
        self.account.user = user;
        self.account.plans = plans;
        self.account.model_picker.clamp(&self.account.models);
        let pending_ready = self.pending.model.as_deref().is_some_and(|id| {
            self.account
                .models
                .iter()
                .any(|m| m.id == id && m.available)
        });
        if pending_ready {
            if let Some(id) = self.pending.model.take() {
                self.choose_model(&id);
            }
        }
    }

    fn cmd_prompt_link_code(&mut self, provider: &str) {
        self.state = AppState::LinkCode;
        self.input.text.clear();
        self.input.cursor_pos = 0;
        self.status = format!("{provider} — paste the code from the sign-in page");
    }

    fn cmd_end_link(&mut self) {
        self.pending.model = None;
        if self.state == AppState::LinkCode {
            self.state = AppState::Idle;
            self.input.text.clear();
            self.input.cursor_pos = 0;
        }
    }

    /// Whether `cmd` lists the agents, projects or processes just as the
    /// app has them. The session sends its lists again whenever any
    /// agent's state moves, mostly unchanged.
    fn lists_nothing_new(&self, cmd: &UiCommand) -> bool {
        match cmd {
            UiCommand::SetAgents(agents) => *agents == self.agents,
            UiCommand::SetProjects(projects) => *projects == self.project_list.projects,
            UiCommand::SetProcesses(processes) => *processes == self.all_processes,
            _ => false,
        }
    }

    /// Process a UI command from the kernel.
    pub fn process_command(&mut self, cmd: UiCommand) {
        debug!(?cmd, "Processing UI command");
        if self.lists_nothing_new(&cmd) {
            return;
        }
        if !matches!(cmd, UiCommand::ForAgent { .. }) {
            self.ticks.redraw = true;
        }
        let unhandled = self
            .turn_ui_command(cmd)
            .and_then(|cmd| self.stream_ui_command(cmd))
            .and_then(|cmd| self.tool_ui_command(cmd))
            .and_then(|cmd| self.approval_ui_command(cmd))
            .and_then(|cmd| self.hud_ui_command(cmd))
            .and_then(|cmd| self.agent_ui_command(cmd))
            .and_then(|cmd| self.channel_ui_command(cmd))
            .and_then(|cmd| self.record_ui_command(cmd))
            .and_then(|cmd| self.workspace_ui_command(cmd))
            .and_then(|cmd| self.trace_ui_command(cmd));
        if let Some(cmd) = unhandled {
            self.account_ui_command(cmd);
        }
    }

    /// Apply a command about run graphs, flows, benches or the log; any
    /// other comes back.
    fn trace_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::GraphLoad { run_id, nodes } => self.load_graph(run_id, nodes),
            UiCommand::GraphUpsert { run_id, nodes } => {
                self.trace_feed.push(&self.active_agent_id, &run_id, &nodes);
                self.upsert_graph(&run_id, nodes);
                self.log.mark_stale();
            }
            UiCommand::FlowLoad {
                agent_id,
                lanes,
                nodes,
            } => self.load_flow(&agent_id, lanes, nodes),
            UiCommand::FlowUpsert {
                agent_id,
                lanes,
                nodes,
            } => self.upsert_flow(&agent_id, lanes, nodes),
            UiCommand::BenchList { benches } => self.cmd_bench_list(benches),
            UiCommand::BenchUpsert(view) => self.cmd_bench_upsert(*view),
            UiCommand::BenchSessions {
                agent_id,
                sessions,
                examples,
            } => {
                self.cmd_bench_sessions(&agent_id, sessions, examples);
            }
            UiCommand::BenchEstimate { agent_id, estimate } => {
                self.cmd_bench_estimate(&agent_id, estimate);
            }
            UiCommand::BenchNotice(notice) => self.cmd_bench_notice(notice),
            UiCommand::LogView(view) => self.cmd_log_view(*view),
            UiCommand::LogNotice { agent_id, notice } => self.cmd_log_notice(&agent_id, notice),
            other => return Some(other),
        }
        None
    }

    /// Apply a command about the API, models or sign-in. Every other
    /// command is applied by the handlers before this one.
    fn account_ui_command(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::SetApiStatus { active, .. } => {
                self.account.api_active = active;
            }
            UiCommand::SetModels {
                options,
                current,
                route,
                user,
                plans,
                thinking,
            } => self.cmd_set_models(options, current, route, user, plans, thinking),
            UiCommand::PromptLinkCode { provider } => self.cmd_prompt_link_code(&provider),
            UiCommand::EndLink => self.cmd_end_link(),
            _ => {}
        }
    }
}
