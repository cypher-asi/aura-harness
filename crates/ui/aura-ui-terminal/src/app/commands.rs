//! UI command processing for the App: what the session sends.

use super::{App, AppState, DetailView, NotificationType, PendingApproval, SidePanelTab};
use crate::{
    components::{elapsed_ms, Activity, BootConsole, Message, MessageRole, ToolStatus, ToolStep},
    events::{BootUpdate, CallUsage, UiCommand, ERROR_PREFIX, SUCCESS_PREFIX, WARNING_PREFIX},
};
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
        self.models = options;
        self.current_model = current;
        self.model_route = route;
        self.thinking = thinking;
        self.user = user;
        self.plans = plans;
        self.model_picker.clamp(&self.models);
        let pending_ready = self
            .pending_model
            .as_deref()
            .is_some_and(|id| self.models.iter().any(|m| m.id == id && m.available));
        if pending_ready {
            if let Some(id) = self.pending_model.take() {
                self.choose_model(&id);
            }
        }
    }

    fn cmd_prompt_link_code(&mut self, provider: &str) {
        self.state = AppState::LinkCode;
        self.input.clear();
        self.cursor_pos = 0;
        self.status = format!("{provider} — paste the code from the sign-in page");
    }

    fn cmd_end_link(&mut self) {
        self.pending_model = None;
        if self.state == AppState::LinkCode {
            self.state = AppState::Idle;
            self.input.clear();
            self.cursor_pos = 0;
        }
    }

    /// Whether `cmd` lists the agents, projects or processes just as the
    /// app has them. The session sends its lists again whenever any
    /// agent's state moves, mostly unchanged.
    fn lists_nothing_new(&self, cmd: &UiCommand) -> bool {
        match cmd {
            UiCommand::SetAgents(agents) => *agents == self.agents,
            UiCommand::SetProjects(projects) => *projects == self.projects,
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
            self.redraw = true;
        }
        match cmd {
            UiCommand::SetStatus(status) => self.status = status,
            UiCommand::StartStreaming => self.cmd_start_streaming(),
            UiCommand::AppendText(text) => self.cmd_append_text(&text),
            UiCommand::FinishStreaming => self.cmd_finish_streaming(),
            UiCommand::SetReplyModel {
                model,
                usage,
                sources,
            } => self.cmd_set_reply_model(&model, usage, &sources),
            UiCommand::StartThinking => self.add_activity(Activity::thinking()),
            UiCommand::AppendThinking(thinking) => self.cmd_append_thinking(&thinking),
            UiCommand::FinishThinking => self.cmd_finish_thinking(),
            UiCommand::ShowMessage(data) => self.cmd_show_message(&data),
            UiCommand::ShowTool(data) => {
                self.add_activity(Activity::tool(&data.id, &data.name, &data.args));
            }
            UiCommand::UpdateToolArgs { id, args } => {
                self.update_tool(&id, |tool| tool.args = args);
            }
            UiCommand::ToolPolicy { id, tool, decision } => {
                self.cmd_tool_policy(&id, &tool, decision);
            }
            UiCommand::ToolRetrying {
                id,
                attempt,
                max_attempts,
            } => {
                self.update_tool(&id, |tool| {
                    tool.retry = Some((attempt, max_attempts));
                    tool.live.clear();
                });
            }
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => {
                self.cmd_complete_tool(&id, &result, success);
                self.refresh_sandbox();
            }
            UiCommand::ToolOutput { id, chunk } => {
                self.update_tool(&id, |tool| {
                    if tool.status == ToolStatus::Running {
                        tool.live.push(&chunk);
                    }
                });
            }
            UiCommand::ToolDiff { id, diff } => {
                self.update_tool(&id, |tool| tool.diff = Some(diff));
            }
            UiCommand::ShowDiff { title, files } => {
                self.add_message(Message::diff(&title, files));
            }
            UiCommand::CommandOutput { chunk } => {
                if let Some(message) = self
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|m| m.is_running_shell())
                {
                    message.append_shell_output(&chunk);
                }
            }
            UiCommand::Boot { id, update } => self.cmd_boot(&id, update),
            UiCommand::CommandFinished { output, exit_code } => {
                if let Some(message) = self
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|m| m.is_running_shell())
                {
                    message.finish_shell(&output, exit_code);
                }
                self.refresh_sandbox();
            }
            UiCommand::RequestApproval {
                id,
                tool,
                description,
                always,
            } => {
                if self.pending_approvals.is_empty() {
                    self.status = format!("Waiting for approval to run {tool}");
                }
                if !self.pending_approvals.iter().any(|a| a.id == id) {
                    self.mark_awaiting_approval(&id, true);
                    self.pending_approvals.push_back(PendingApproval {
                        id,
                        tool,
                        description,
                        always,
                    });
                }
                self.state = AppState::AwaitingApproval;
            }
            UiCommand::CloseApproval(id) => self.cmd_close_approval(&id),
            UiCommand::SetMode(mode) => self.hud.mode = Some(mode),
            UiCommand::SetApproval(level) => self.hud.approval = Some(level),
            UiCommand::ShowError(msg) => {
                self.cmd_notification(&msg, ERROR_PREFIX, NotificationType::Error);
            }
            UiCommand::ShowSuccess(msg) => {
                self.cmd_notification(&msg, SUCCESS_PREFIX, NotificationType::Success);
            }
            UiCommand::ShowWarning(msg) => {
                self.cmd_notification(&msg, WARNING_PREFIX, NotificationType::Warning);
            }
            UiCommand::Complete => {
                self.cmd_complete();
                self.refresh_sandbox();
                if self.log.is_open() {
                    self.request_log();
                }
            }
            UiCommand::TurnStarted { prompt } => self.cmd_turn_started(prompt),
            UiCommand::UserSteered(text) => self.cmd_user_steered(&text),
            UiCommand::CallStats(call) => self.hud.stats.add(&call),
            UiCommand::Compacted(text) => {
                self.hud.stats.compactions = self.hud.stats.compactions.saturating_add(1);
                self.add_message(Message::new(MessageRole::System, &text));
            }
            UiCommand::SetTodos(todos) => {
                if todos.is_empty() {
                    self.todos_open = false;
                    // The next list starts short, whatever the last one did.
                    self.todos_expanded = false;
                }
                self.hud.todos = todos;
            }
            UiCommand::SetJobs(jobs) => self.hud.jobs = jobs,
            UiCommand::SetQueue(queue) => self.set_queue(queue),
            UiCommand::SetSessionTotals(totals) => self.hud.stats.restore(&totals),
            UiCommand::SetInstructions(files) => self.hud.instructions = Some(files),
            UiCommand::SetPolicies(view) => self.hud.policies = Some(view),
            UiCommand::SetContext(context) => {
                if context.limit_tokens.is_some() {
                    self.hud.stats.context_limit = context.limit_tokens;
                }
                self.hud.context = Some(context);
            }
            UiCommand::ForAgent { agent_id, command } => {
                self.process_agent_command(&agent_id, *command);
            }
            UiCommand::SetPlacement { agent_id, state } => self.cmd_set_placement(agent_id, state),
            UiCommand::SetHost(view) => self.cmd_set_host(*view),
            UiCommand::SetRepo(view) => self.cmd_set_repo(*view),
            UiCommand::SetChannels(channels) => self.set_channels(channels),
            UiCommand::SetChannelTasks { .. } => {
                // The channel app shows only its chat, members and info;
                // members' tasks are seen from PROJECTS.
            }
            UiCommand::ChannelPost { channel_id, post } => self.add_channel_post(channel_id, post),
            UiCommand::SetChannelPosts { channel_id, posts } => {
                self.set_channel_posts(channel_id, posts);
            }
            UiCommand::ClearConversation => self.cmd_clear_conversation(),
            UiCommand::NewRecord(record) => self.cmd_new_record(record),
            UiCommand::SetProjects(projects) => {
                self.projects = projects;
                self.projects_changed_under_channel_form();
            }
            UiCommand::SetAgents(agents) => self.cmd_set_agents(agents),
            UiCommand::SetArchivedAgents(agents) => self.set_archived(agents),
            UiCommand::SetActiveAgent(agent_id) => self.cmd_set_active_agent(&agent_id),
            UiCommand::SwitchFailed(agent_id) => self.cmd_switch_failed(&agent_id),
            UiCommand::ChatReady => self.reveal_chat(),
            UiCommand::SetAgentDefinitions(definitions) => self.set_definitions(definitions),
            UiCommand::OpenEditor(path) => self.editor_request = Some(path),
            UiCommand::ClearRecords => {
                self.records.clear();
                self.selected_record = 0;
                if self.detail == Some(DetailView::Record) {
                    self.close_detail();
                }
            }
            UiCommand::SetRecords(records) => self.cmd_set_records(records),
            UiCommand::LoadConversation(history) => self.cmd_load_conversation(history),
            UiCommand::SetSandboxRoot(root) => self.set_sandbox_root(root),
            UiCommand::SetSandboxRemote(root) => self.set_sandbox_remote(root),
            UiCommand::SandboxListing { dir, entries } => self.set_sandbox_listing(dir, entries),
            UiCommand::SandboxFile { path, text } => self.show_sandbox_file(&path, text),
            UiCommand::SetProcesses(processes) => self.set_processes(processes),
            UiCommand::AddMail(mail) => self.add_mail(mail),
            UiCommand::SetThreadSubject { thread_id, subject } => {
                self.set_thread_subject(thread_id, &subject);
            }
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
            UiCommand::SetApiStatus { active, .. } => {
                self.api_active = active;
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
            UiCommand::SetPlan(plan) => {
                let current = self.active_plan.as_ref().map(|p| p.path.as_str());
                if plan
                    .as_ref()
                    .is_some_and(|p| Some(p.path.as_str()) != current)
                {
                    self.show_side_tab(SidePanelTab::Tasks);
                }
                self.task_list.set_current_plan(plan.as_ref());
                self.active_plan = plan;
            }
            UiCommand::SetPlans(plans) => self.task_list.set_plans(plans),
            UiCommand::SetTasks(tasks) => self.task_list.set_tasks(tasks),
            UiCommand::UpsertTask(task) => self.task_list.upsert(task),
        }
    }

    fn cmd_start_streaming(&mut self) {
        self.streaming_content.clear();
        if self
            .messages
            .back()
            .is_some_and(|m| m.is_streaming() && m.content().is_empty())
        {
            return;
        }
        let mut msg = Message::new(MessageRole::Assistant, "");
        msg.set_streaming(true);
        self.add_message(msg);
    }

    fn cmd_append_text(&mut self, text: &str) {
        let before = self.streaming_content.len();
        self.streaming_content.push_str(text);
        if let Some(last_msg) = self.messages.back_mut() {
            if !last_msg.is_streaming() {
                return;
            }
            // Copying the whole reply on every delta grows with its length.
            if last_msg.content().len() == before {
                last_msg.append(text);
            } else {
                last_msg.set_content(&self.streaming_content);
            }
        }
    }

    fn cmd_finish_streaming(&mut self) {
        if let Some(last_msg) = self.messages.back_mut() {
            if last_msg.is_streaming() {
                last_msg.set_streaming(false);
                if self.streaming_content.is_empty() {
                    self.messages.pop_back();
                } else {
                    last_msg.set_content(&self.streaming_content);
                }
            }
        }
        self.streaming_content.clear();
    }

    /// Each turn's split of tokens by sign-in, as its last reply's
    /// footer shows it. Every reply of a turn carries the turn's split
    /// so far, so only the last one before the next prompt counts.
    pub(super) fn turn_sources(&self) -> Vec<&[crate::events::SourceUsage]> {
        let mut turns = Vec::new();
        let mut last: Option<&[crate::events::SourceUsage]> = None;
        for message in &self.messages {
            if message.role() == MessageRole::User {
                turns.extend(last.take());
            } else if message.role() == MessageRole::Assistant && !message.sources().is_empty() {
                last = Some(message.sources());
            }
        }
        turns.extend(last);
        turns
    }

    fn cmd_set_reply_model(
        &mut self,
        model: &str,
        usage: Option<CallUsage>,
        sources: &[crate::events::SourceUsage],
    ) {
        let replies = self
            .messages
            .iter_mut()
            .rev()
            .take_while(|m| m.role() != MessageRole::User)
            .filter(|m| m.role() == MessageRole::Assistant && m.activity_step().is_none());
        for message in replies {
            message.set_model(model);
            message.set_usage(usage);
            message.set_sources(sources);
        }
    }

    fn cmd_show_message(&mut self, data: &crate::events::MessageData) {
        let mut msg = Message::new(data.role, &data.content);
        if data.is_streaming {
            msg.set_streaming(true);
        }
        if let Some(model) = &data.model {
            msg.set_model(model);
        }
        msg.set_usage(data.usage);
        msg.set_sources(&data.sources);
        self.add_message(msg);
    }

    fn add_activity(&mut self, activity: Activity) {
        self.add_message(Message::activity(activity, self.activity_expanded));
    }

    /// The last thinking block still streaming.
    fn live_thinking(&mut self) -> Option<&mut Message> {
        self.messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m.activity_step(), Some(Activity::Thinking(step)) if step.running))
    }

    fn cmd_append_thinking(&mut self, text: &str) {
        if self.live_thinking().is_none() {
            self.add_activity(Activity::thinking());
        }
        if let Some(message) = self.live_thinking() {
            message.update_activity(|activity| {
                if let Activity::Thinking(step) = activity {
                    step.text.push_str(text);
                }
            });
        }
    }

    fn cmd_finish_thinking(&mut self) {
        if let Some(message) = self.live_thinking() {
            message.update_activity(|activity| activity.finish(""));
        }
    }

    /// Apply `update` to the tool call with use id `id`; `false` when no
    /// such call is shown.
    fn update_tool(&mut self, id: &str, update: impl FnOnce(&mut ToolStep)) -> bool {
        let found = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m.activity_step(), Some(Activity::Tool(tool)) if tool.id == id));
        let Some(message) = found else {
            return false;
        };
        message.update_activity(|activity| {
            if let Activity::Tool(tool) = activity {
                update(tool);
            }
        });
        true
    }

    /// Keep the policy decision with its tool call, so a run of calls
    /// stays one block; a call that isn't shown gets a `policy:` line.
    fn cmd_tool_policy(&mut self, id: &str, tool: &str, decision: String) {
        let line = format!("policy: {tool} {decision}");
        if !self.update_tool(id, |step| step.policy = Some(decision)) {
            self.add_message(Message::new(MessageRole::System, &line));
        }
    }

    fn cmd_complete_tool(&mut self, id: &str, result: &str, success: bool) {
        self.update_tool(id, |tool| {
            tool.result = Some(super::format::format_tool_result(&tool.name, result));
            tool.status = if success {
                ToolStatus::Success
            } else {
                ToolStatus::Error
            };
            tool.retry = None;
            tool.duration_ms = elapsed_ms(tool.started);
            tool.live.clear();
        });
    }

    /// End every activity block still in progress, e.g. when the turn ends.
    fn finish_live_activity(&mut self) {
        for message in &mut self.messages {
            if message.is_live_activity() {
                message.update_activity(|activity| activity.finish("interrupted"));
            }
        }
    }

    /// Change boot console `id`, adding it on its first update.
    fn cmd_boot(&mut self, id: &str, update: BootUpdate) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.boot_console().is_some_and(|console| console.id == id))
        {
            message.update_boot(|console| console.apply(update));
            return;
        }
        let title = match &update {
            BootUpdate::Begin { title } => title.as_str(),
            _ => "Cloud machine",
        };
        let mut console = BootConsole::new(id, title);
        console.apply(update);
        self.add_message(Message::boot(console));
    }

    fn cmd_notification(&mut self, msg: &str, prefix: &str, kind: NotificationType) {
        self.add_message(Message::new(MessageRole::System, &format!("{prefix}{msg}")));
        self.notification = Some((msg.to_string(), kind));
    }

    fn cmd_complete(&mut self) {
        if let Some(last_msg) = self.messages.back_mut() {
            if last_msg.is_streaming() {
                last_msg.set_streaming(false);
                if !self.streaming_content.is_empty() {
                    last_msg.set_content(&self.streaming_content);
                } else if last_msg.content().is_empty() {
                    self.messages.pop_back();
                }
            }
        }
        self.streaming_content.clear();
        self.finish_live_activity();
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
    }

    /// Drop the prompt for approval `id`, which stopped waiting; the turn
    /// goes on without it.
    fn cmd_close_approval(&mut self, id: &str) {
        let before = self.pending_approvals.len();
        self.pending_approvals.retain(|a| a.id != id);
        if self.pending_approvals.len() < before {
            self.mark_awaiting_approval(id, false);
            self.after_approval_closed();
        }
    }

    /// Mark the tool call behind approval `request_id` as waiting for the
    /// user, or no longer waiting. The kernel's request id is
    /// `<agent hex>:<tool use id>`.
    pub(super) fn mark_awaiting_approval(&mut self, request_id: &str, waiting: bool) {
        let tool_use_id = request_id
            .split_once(':')
            .map_or(request_id, |(_, tool_use_id)| tool_use_id);
        self.update_tool(tool_use_id, |tool| tool.awaiting_approval = waiting);
    }

    /// Show the next waiting prompt, or go back to the turn when none is
    /// left.
    pub(super) fn after_approval_closed(&mut self) {
        if let Some(next) = self.pending_approvals.front() {
            self.status = format!("Waiting for approval to run {}", next.tool);
        } else if self.state == AppState::AwaitingApproval {
            self.state = AppState::Processing;
        }
    }

    fn cmd_turn_started(&mut self, prompt: Option<String>) {
        if let Some(prompt) = prompt {
            let images = self.take_queued_previews(&prompt);
            self.add_message(Message::new(MessageRole::User, &prompt).with_images(images));
        }
        // A turn the user did not start (a background completion notice)
        // must not hide a login prompt or an approval on screen.
        if self.state == AppState::Idle {
            self.state = AppState::Processing;
            self.status = "Thinking...".to_string();
        }
    }

    /// Show a message sent into the running turn. A reply still streaming
    /// is split around it, so the rest of the reply keeps streaming below.
    fn cmd_user_steered(&mut self, text: &str) {
        let was_streaming = self.messages.back().is_some_and(Message::is_streaming);
        if was_streaming {
            self.cmd_finish_streaming();
        }
        self.add_message(Message::new(MessageRole::User, text));
        if was_streaming {
            self.cmd_start_streaming();
        }
    }

    fn cmd_clear_conversation(&mut self) {
        self.messages.clear();
    }

    fn cmd_load_conversation(&mut self, history: Vec<crate::events::MessageData>) {
        self.cmd_clear_conversation();
        self.streaming_content.clear();
        for data in history {
            let mut msg = if let Some(boot) = &data.boot {
                Message::boot(BootConsole::replay(boot, data.timestamp_ms))
            } else if let Some(activity) = &data.activity {
                Message::activity(Activity::from(activity), self.activity_expanded)
            } else if let Some(command) = data.command.as_deref() {
                let mut msg = Message::shell(command);
                msg.finish_shell("", None);
                msg
            } else {
                Message::new(data.role, &data.content)
            };
            if let Some(ts) = data
                .timestamp_ms
                .and_then(|ms| i64::try_from(ms).ok())
                .and_then(chrono::DateTime::from_timestamp_millis)
            {
                msg = msg.with_timestamp(ts);
            }
            if let Some(model) = &data.model {
                msg.set_model(model);
            }
            msg.set_usage(data.usage);
            self.messages.push_back(msg);
        }
        while self.messages.len() > super::MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.scroll_offset = 0;
        self.reveal_chat();
    }

    /// Append a record, oldest first. A selection on the newest record
    /// follows new arrivals; one moved up stays on its record.
    fn cmd_new_record(&mut self, record: crate::events::RecordSummary) {
        // Agents running in the background write records too; the panel
        // lists the active agent's.
        if !record.agent_id.is_empty()
            && !self.active_agent_id.is_empty()
            && record.agent_id != self.active_agent_id
        {
            return;
        }
        let follow = self.detail != Some(DetailView::Record)
            && self.selected_record + 1 >= self.records.len();
        self.records.push_back(record);
        while self.records.len() > super::MAX_RECORDS {
            self.records.pop_front();
            self.selected_record = self.selected_record.saturating_sub(1);
        }
        if follow {
            self.selected_record = self.records.len() - 1;
        }
    }

    /// Replace the records with `records`, oldest first, keeping the
    /// newest [`super::MAX_RECORDS`]. The records held already, as when an
    /// agent comes back on screen, stay as they are, highlight, scroll and
    /// Record detail view included. Otherwise the detail view closes, and
    /// the highlight stays on its record, or follows the newest when it
    /// was on it. Records all of another agent, sent for an agent the
    /// screen has left since, are dropped. The agent's records newer than
    /// all of `records` stay after them: written after `records` were read,
    /// they came on their own before them.
    fn cmd_set_records(&mut self, records: Vec<crate::events::RecordSummary>) {
        let sent = records.len();
        let active = &self.active_agent_id;
        let mut kept: std::collections::VecDeque<_> = records
            .into_iter()
            .filter(|record| {
                record.agent_id.is_empty() || active.is_empty() || record.agent_id == *active
            })
            .collect();
        if sent > 0 && kept.is_empty() {
            return;
        }
        let read_up_to = kept.back().map_or(0, |record| record.seq);
        kept.extend(
            self.records
                .iter()
                .filter(|record| {
                    !active.is_empty() && record.agent_id == *active && record.seq > read_up_to
                })
                .cloned(),
        );
        let excess = kept.len().saturating_sub(super::MAX_RECORDS);
        kept.drain(..excess);
        let same = |a: &crate::events::RecordSummary, b: &crate::events::RecordSummary| {
            a.seq == b.seq && a.full_hash == b.full_hash
        };
        if kept.len() == self.records.len()
            && kept.iter().zip(&self.records).all(|(a, b)| same(a, b))
        {
            return;
        }
        if self.detail == Some(DetailView::Record) {
            self.close_detail();
        }
        let newest = self.selected_record + 1 >= self.records.len();
        let on = self.records.get(self.selected_record).map(|r| r.seq);
        self.records = kept;
        let last = self.records.len().saturating_sub(1);
        self.selected_record = on
            .filter(|_| !newest)
            .and_then(|seq| self.records.iter().position(|r| r.seq == seq))
            .unwrap_or(last);
    }

    /// Point the Sandbox tree at `root`. A tree held for it already, as
    /// when its agent comes back on screen, keeps its opened folders,
    /// highlight and scroll, and is read again on the next tick to show
    /// what changed while it was away.
    fn set_sandbox_root(&mut self, root: std::path::PathBuf) {
        if !self.sandbox.is_remote() && self.sandbox.root() == Some(root.as_path()) {
            self.sandbox_stale = true;
            return;
        }
        self.sandbox.set_root(root);
        self.sandbox_scroll = 0;
    }

    fn cmd_set_agents(&mut self, agents: Vec<crate::events::AgentSummary>) {
        self.agents = agents;
        self.collapsed_projects
            .retain(|key| self.agents.iter().any(|agent| agent.project_key() == key));
        // An agent that lost its spawns drops its fold, so a new spawn
        // under it later shows up instead of hiding.
        let foldable: std::collections::HashSet<&str> = (0..self.agents.len())
            .filter(|&i| self.agent_has_spawns(i))
            .map(|i| self.agents[i].id.as_str())
            .collect();
        self.collapsed_agents
            .retain(|id| foldable.contains(id.as_str()));
        self.select_visible_active();
        self.refresh_side_scope();
    }

    /// Re-read the sandbox tree on the next tick, so files the agent
    /// writes show up.
    fn refresh_sandbox(&mut self) {
        self.sandbox_stale = true;
    }
}
