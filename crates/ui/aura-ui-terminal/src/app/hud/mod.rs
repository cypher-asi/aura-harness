//! What an agent's session has used and is working on: the data behind
//! the status line, the todo strip, and the `/context`, `/cost` and
//! `/todos` commands. Each agent has its own, swapped in with its chat.

use super::format::{format_partial_cost, format_tokens};
use crate::events::{
    CallStats, ContextView, InstructionFileView, InstructionStatus, JobView, PolicyView,
    QueuedItemView, SessionTotals, SourceUsage, TodoState, TodoView,
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
    /// Calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
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
        if call.unpriced {
            self.unpriced_calls = self.unpriced_calls.saturating_add(1);
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
            unpriced_calls: totals.unpriced_calls,
            context_tokens: totals.context_tokens,
            context_limit: self.context_limit,
            compactions: 0,
            models: totals.models.clone(),
        };
    }

    /// What the session cost so far: `$0.042`, `$0.042+?` when some calls
    /// had no known price, `None` when none had one.
    #[must_use]
    pub fn cost_label(&self) -> Option<String> {
        format_partial_cost(self.priced.then_some(self.cost_usd), self.unpriced_calls)
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
    /// Work waiting for the agent's current work to end, oldest first
    pub queue: Vec<QueuedItemView>,
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
    /// The policy the agent's kernel enforces, for the POLICIES tab;
    /// `None` until the event loop says
    pub policies: Option<PolicyView>,
}

impl Hud {
    /// How much work waits for the agent's current work to end.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

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

    /// Whether any todo item is still pending or in progress. A list with
    /// every item closed is finished and no longer shows in the chat.
    #[must_use]
    pub fn has_open_todos(&self) -> bool {
        self.todos.iter().any(|t| !t.state.is_closed())
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
        let cost = match stats.cost_label() {
            Some(cost) if stats.unpriced_calls > 0 => format!(
                "about {cost} at API list prices, leaving out {} call{} with no known price",
                stats.unpriced_calls,
                plural(stats.unpriced_calls)
            ),
            Some(cost) => format!("about {cost} at API list prices"),
            None => "no priced calls (local or unpriced models)".to_string(),
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

/// The `/cost` lines saying who paid for the session's tokens, summed
/// over `turns` (each turn's split by sign-in, as its reply footer shows
/// it). Empty when no turn says.
pub fn sources_report<'a>(turns: impl IntoIterator<Item = &'a [SourceUsage]>) -> String {
    let mut totals: Vec<(String, u64, Option<f64>, u32)> = Vec::new();
    for source in turns.into_iter().flatten() {
        let at = match totals.iter().position(|(label, ..)| *label == source.label) {
            Some(at) => at,
            None => {
                totals.push((source.label.clone(), 0, None, 0));
                totals.len() - 1
            }
        };
        let total = &mut totals[at];
        total.1 += source.tokens;
        if let Some(cost) = source.cost_usd {
            total.2 = Some(total.2.unwrap_or(0.0) + cost);
        }
        total.3 = total.3.saturating_add(source.unpriced_calls);
    }
    if totals.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nBy sign-in:");
    for (label, tokens, cost, unpriced_calls) in totals {
        out.push_str(&format!("\n  {label:<14}{:>8} tok", format_tokens(tokens)));
        if let Some(cost) = format_partial_cost(cost, unpriced_calls) {
            out.push_str(&format!("  {cost}"));
        }
    }
    out
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
mod tests;
