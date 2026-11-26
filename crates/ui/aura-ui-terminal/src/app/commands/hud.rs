//! What the HUD shows of the agent: mode, usage, todos, jobs, queue,
//! context, instructions and policies.

use crate::app::App;
use crate::components::{Message, MessageRole};
use crate::events::{ContextView, TodoView, UiCommand};

impl App {
    /// Apply a command that updates the HUD; any other comes back.
    pub(super) fn hud_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::SetMode(mode) => self.hud.mode = Some(mode),
            UiCommand::SetApproval(level) => self.hud.approval = Some(level),
            UiCommand::CallStats(call) => self.hud.stats.add(&call),
            UiCommand::Compacted(text) => self.cmd_compacted(&text),
            UiCommand::SetTodos(todos) => self.cmd_set_todos(todos),
            UiCommand::SetJobs(jobs) => self.hud.jobs = jobs,
            UiCommand::SetQueue(queue) => self.set_queue(queue),
            UiCommand::SetSessionTotals(totals) => self.hud.stats.restore(&totals),
            UiCommand::SetInstructions(files) => self.hud.instructions = Some(files),
            UiCommand::SetPolicies(view) => self.hud.policies = Some(view),
            UiCommand::SetContext(context) => self.cmd_set_context(context),
            other => return Some(other),
        }
        None
    }

    fn cmd_compacted(&mut self, text: &str) {
        self.hud.stats.compactions = self.hud.stats.compactions.saturating_add(1);
        self.add_message(Message::new(MessageRole::System, text));
    }

    fn cmd_set_todos(&mut self, todos: Vec<TodoView>) {
        self.hud.todos = todos;
        if self.hud.todos.is_empty() {
            self.reset_todo_drawer();
        } else if self.hud.has_open_todos() {
            // Work under way shows the drawer by itself; once it
            // finishes, the drawer hides again until `/todos`.
            self.unforce_todo_drawer();
        }
    }

    fn cmd_set_context(&mut self, context: ContextView) {
        if context.limit_tokens.is_some() {
            self.hud.stats.context_limit = context.limit_tokens;
        }
        self.hud.context = Some(context);
    }
}
