//! What an agent's session has used and is working on: the data behind
//! the status line, the todo strip, and the `/context`, `/cost` and
//! `/todos` commands. Each agent has its own, swapped in with its chat.

use super::format::{format_cost, format_tokens};
use crate::events::{CallStats, ContextView, JobView, TodoState, TodoView};

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
        let limit = self.context_limit.filter(|l| *l > 0)?;
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
            input_tokens: input,
            cache_read_tokens: read,
            cache_write_tokens: write,
            output_tokens: 100,
            cost_usd: cost,
            context_limit: Some(200_000),
        }
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
