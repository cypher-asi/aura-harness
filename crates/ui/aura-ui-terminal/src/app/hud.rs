//! What an agent's session has used and is working on: the data behind
//! the status line, the todo strip, and the `/context`, `/cost` and
//! `/todos` commands. Each agent has its own, swapped in with its chat.

use super::format::{format_cost, format_tokens};
use crate::events::{
    CallStats, ContextView, InstructionFileView, InstructionStatus, JobView, SessionTotals,
    TodoState, TodoView,
};
use crate::mode_command::{ApprovalLevel, PermissionMode};

/// Model usage of an agent's session so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionStats {
    /// Model calls made
    pub calls: u32,
    /// Prompt tokens neither read from nor written to the cache
    pub input_tokens: u64,
    /// Prompt tokens read from the cache
    pub cache_read_tokens: u64,
    /// Prompt tokens written to the cache
    pub cache_write_tokens: u64,
    /// Output tokens
    pub output_tokens: u64,
    /// Estimated cost of the priced calls
    pub cost_usd: f64,
    /// Whether any call had a price
    pub priced: bool,
    /// Prompt of the latest call: what is in the context window now
    pub context_tokens: u64,
    /// Context window of the model, when known
    pub context_limit: Option<u64>,
    /// Times older history was summarized
    pub compactions: u32,
    /// Models the calls went to, first use first
    pub models: Vec<String>,
}

impl SessionStats {
    /// Add one model call.
    pub fn add(&mut self, call: &CallStats) {
        self.calls = self.calls.saturating_add(1);
        self.input_tokens += call.input_tokens;
        self.cache_read_tokens += call.cache_read_tokens;
        self.cache_write_tokens += call.cache_write_tokens;
        self.output_tokens += call.output_tokens;
        if let Some(cost) = call.cost_usd {
            self.cost_usd += cost;
            self.priced = true;
        }
        self.context_tokens = call.prompt_tokens();
        if call.context_limit.is_some() {
            self.context_limit = call.context_limit;
        }
        if !call.model.is_empty() && !self.models.contains(&call.model) {
            self.models.push(call.model.clone());
        }
    }

    /// Take the totals of a session opened or resumed, keeping what is
    /// known about the model's context window.
    pub fn restore(&mut self, totals: &SessionTotals) {
        *self = Self {
            calls: totals.calls,
            input_tokens: totals.input_tokens,
            cache_read_tokens: totals.cache_read_tokens,
            cache_write_tokens: totals.cache_write_tokens,
            output_tokens: totals.output_tokens,
            cost_usd: totals.cost_usd.unwrap_or(0.0),
            priced: totals.cost_usd.is_some(),
            context_tokens: totals.context_tokens,
            context_limit: self.context_limit,
            compactions: 0,
            models: totals.models.clone(),
        };
    }

    /// Share of prompt tokens served from the cache, in percent. `None`
    /// before any call.
    #[must_use]
    pub fn cache_hit_percent(&self) -> Option<u64> {
        let prompt = self.input_tokens + self.cache_read_tokens + self.cache_write_tokens;
        (prompt > 0).then(|| self.cache_read_tokens * 100 / prompt)
    }

    /// How full the context window is, in percent, when the window is known.
    #[must_use]
    pub fn context_percent(&self) -> Option<u64> {
        self.context_percent_in(self.context_limit)
    }

    /// How full a context window of `limit` tokens would be, in percent.
    #[must_use]
    pub fn context_percent_in(&self, limit: Option<u64>) -> Option<u64> {
        let limit = limit.filter(|l| *l > 0)?;
        Some((self.context_tokens * 100 / limit).min(100))
    }
}

/// Everything the status line and the HUD commands show for one agent.
#[derive(Debug, Clone, Default)]
pub struct Hud {
    /// Usage so far
    pub stats: SessionStats,
    /// The agent's todo list
    pub todos: Vec<TodoView>,
    /// The agent's background jobs
    pub jobs: Vec<JobView>,
    /// Work waiting for the agent's current work to end
    pub queued: usize,
    /// Make-up of the context as of the last turn
    pub context: Option<ContextView>,
    /// The kind of work the agent may do; `None` until the event loop
    /// says
    pub mode: Option<PermissionMode>,
    /// Which tool calls ask first; `None` until the event loop says
    pub approval: Option<ApprovalLevel>,
    /// Project instruction files the agent's turns load; `None` until
    /// the agent reports them
    pub instructions: Option<Vec<InstructionFileView>>,
}

impl Hud {
    /// Background jobs still running.
    #[must_use]
    pub fn running_jobs(&self) -> usize {
        self.jobs.iter().filter(|job| job.running).count()
    }

    /// `(done, total)` over the todo items that weren't cancelled.
    #[must_use]
    pub fn todo_progress(&self) -> (usize, usize) {
        let live = self
            .todos
            .iter()
            .filter(|t| t.state != TodoState::Cancelled);
        let total = live.clone().count();
        let done = live.filter(|t| t.state == TodoState::Completed).count();
        (done, total)
    }

    /// The item being worked on, else the next one not started.
    #[must_use]
    pub fn current_todo(&self) -> Option<&TodoView> {
        self.todos
            .iter()
            .find(|t| t.state == TodoState::InProgress)
            .or_else(|| self.todos.iter().find(|t| t.state == TodoState::Pending))
    }

    /// `/context`: what fills the context window.
    #[must_use]
    pub fn context_report(&self) -> String {
        let stats = &self.stats;
        let limit = self
            .context
            .and_then(|c| c.limit_tokens)
            .or(stats.context_limit);
        let mut out = String::from("Context");
        match (stats.context_tokens, limit) {
            (0, _) => out.push_str(": no model call yet"),
            (used, Some(limit)) => out.push_str(&format!(
                ": {} of {} tokens ({}%)",
                format_tokens(used),
                format_tokens(limit),
                stats.context_percent().unwrap_or(0)
            )),
            (used, None) => out.push_str(&format!(": {} tokens", format_tokens(used))),
        }
        if let Some(context) = self.context {
            out.push_str("\nEstimated from the last turn:");
            for (label, tokens) in [
                ("system prompt", context.system_tokens),
                ("tools", context.tools_tokens),
                ("skills", context.skills_tokens),
                ("conversation", context.conversation_tokens),
            ] {
                if tokens > 0 {
                    out.push_str(&format!("\n  {label:<14}{:>8}", format_tokens(tokens)));
                }
            }
        }
        if let Some(hit) = stats.cache_hit_percent() {
            out.push_str(&format!("\nPrompt cache hits: {hit}%"));
        }
        if stats.compactions > 0 {
            out.push_str(&format!(
                "\nHistory summarized {} time{}",
                stats.compactions,
                plural(stats.compactions)
            ));
        }
        if let Some(files) = &self.instructions {
            out.push_str(&instructions_report(files));
        }
        out
    }

    /// `/cost`: what the session has used and cost.
    #[must_use]
    pub fn cost_report(&self) -> String {
        let stats = &self.stats;
        if stats.calls == 0 {
            return "Cost: no model call yet".to_string();
        }
        let cost = if stats.priced {
            format!("about {} at API list prices", format_cost(stats.cost_usd))
        } else {
            "no priced calls (local or unpriced models)".to_string()
        };
        format!(
            "Cost: {cost}\n{} model call{}\n  new input   {:>8}\n  cache read  {:>8}\n  cache write {:>8}\n  output      {:>8}",
            stats.calls,
            plural(stats.calls),
            format_tokens(stats.input_tokens),
            format_tokens(stats.cache_read_tokens),
            format_tokens(stats.cache_write_tokens),
            format_tokens(stats.output_tokens),
        )
    }
}

/// The `/context` lines listing the project instruction files.
fn instructions_report(files: &[InstructionFileView]) -> String {
    if files.is_empty() {
        return "\nProject instructions: none found (/init writes an AGENTS.md)".to_string();
    }
    // Rough bytes-to-tokens ratio for English prose and code.
    let tokens = |bytes: usize| format_tokens(u64::try_from(bytes / 4).unwrap_or(u64::MAX));
    let total: usize = files.iter().map(|f| f.bytes).sum();
    let mut out = format!("\nProject instructions (~{} tokens):", tokens(total));
    for file in files {
        let detail = match file.status {
            InstructionStatus::Included => tokens(file.bytes),
            InstructionStatus::Truncated(whole) => {
                format!("{} (cut from {})", tokens(file.bytes), tokens(whole))
            }
            InstructionStatus::OnRequest => "read when relevant".to_string(),
            InstructionStatus::Omitted => "left out (size limit)".to_string(),
        };
        out.push_str(&format!("\n  {}  {detail}", file.path));
        if let Some(globs) = &file.globs {
            out.push_str(&format!(" for {globs}"));
        }
    }
    out
}

const fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(input: u64, read: u64, write: u64, cost: Option<f64>) -> CallStats {
        CallStats {
            model: "aura-claude-sonnet-4-6".to_string(),
            input_tokens: input,
            cache_read_tokens: read,
            cache_write_tokens: write,
            output_tokens: 100,
            cost_usd: cost,
            context_limit: Some(200_000),
        }
    }

    #[test]
    fn models_are_counted_once_and_kept_on_restore() {
        let mut stats = SessionStats::default();
        stats.add(&call(10, 0, 0, None));
        stats.add(&CallStats {
            model: "aura-claude-haiku-4-5".to_string(),
            ..call(10, 0, 0, None)
        });
        stats.add(&call(10, 0, 0, None));
        assert_eq!(stats.models, ["aura-claude-sonnet-4-6", "aura-claude-haiku-4-5"]);

        stats.restore(&SessionTotals {
            calls: 7,
            models: vec!["claude/claude-opus-4-7".to_string()],
            ..SessionTotals::default()
        });
        assert_eq!(stats.models, ["claude/claude-opus-4-7"]);
    }

    #[test]
    fn totals_and_rates() {
        let mut stats = SessionStats::default();
        stats.add(&call(1_000, 0, 9_000, Some(0.05)));
        stats.add(&call(500, 9_000, 500, Some(0.01)));

        assert_eq!(stats.calls, 2);
        assert_eq!(stats.context_tokens, 10_000);
        assert_eq!(stats.context_percent(), Some(5));
        assert_eq!(stats.cache_hit_percent(), Some(45));
        assert!((stats.cost_usd - 0.06).abs() < 1e-9);
        assert!(stats.priced);
    }

    #[test]
    fn unpriced_calls_say_so() {
        let mut hud = Hud::default();
        hud.stats.add(&call(10, 0, 0, None));
        assert!(hud.cost_report().contains("no priced calls"));
    }

    #[test]
    fn context_report_lists_instruction_files() {
        let file = |path: &str, bytes, globs: Option<&str>, status| InstructionFileView {
            path: path.to_string(),
            bytes,
            globs: globs.map(str::to_string),
            status,
        };
        let mut hud = Hud::default();
        assert!(!hud.context_report().contains("Project instructions"));

        hud.instructions = Some(Vec::new());
        assert!(hud.context_report().contains("none found (/init"));

        hud.instructions = Some(vec![
            file("~/p/AGENTS.md", 4_000, None, InstructionStatus::Included),
            file(
                "~/p/.cursor/rules/rust.mdc",
                400,
                Some("**/*.rs"),
                InstructionStatus::Included,
            ),
            file(
                "~/p/.cursor/rules/db.mdc",
                0,
                None,
                InstructionStatus::OnRequest,
            ),
            file("/AGENTS.md", 0, None, InstructionStatus::Omitted),
        ]);
        let report = hud.context_report();
        assert!(
            report.contains("Project instructions (~1.1k tokens):"),
            "{report}"
        );
        assert!(report.contains("~/p/AGENTS.md  1.0k"), "{report}");
        assert!(report.contains("rust.mdc  100 for **/*.rs"), "{report}");
        assert!(report.contains("db.mdc  read when relevant"), "{report}");
        assert!(
            report.contains("/AGENTS.md  left out (size limit)"),
            "{report}"
        );
    }

    #[test]
    fn todo_progress_skips_cancelled_items() {
        let todo = |content: &str, state| TodoView {
            content: content.to_string(),
            state,
        };
        let hud = Hud {
            todos: vec![
                todo("a", TodoState::Completed),
                todo("b", TodoState::Cancelled),
                todo("c", TodoState::Pending),
                todo("d", TodoState::InProgress),
            ],
            ..Hud::default()
        };
        assert_eq!(hud.todo_progress(), (1, 3));
        assert_eq!(hud.current_todo().map(|t| t.content.as_str()), Some("d"));
    }
}
