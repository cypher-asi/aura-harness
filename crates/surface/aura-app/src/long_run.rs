//! How the terminal runs long tasks: no iteration caps, keep working while
//! todo items are open, compact only under real pressure, and optional
//! per-turn time and cost limits.

use aura_agent::AgentLoopConfig;
use aura_exec_tools::jobs::{JobKillTool, JobOutputTool};
use aura_exec_tools::todo::{TodoWriteTool, TODO_WRITE_TOOL_NAME};
use aura_exec_tools::web::{WebFetchSettings, WebFetchTool, WebSearchSettings, WebSearchTool};
use aura_exec_tools::{Tool, ToolExecutor};
use aura_model::ToolDefinition;
use std::time::Duration;
use tracing::{info, warn};

/// Add the planning, background-job and web tools to the terminal's tool
/// list. `run_command` offers `background` itself; the job tools read and
/// stop what it starts.
pub(crate) fn add_long_run_tools(tools: &mut Vec<ToolDefinition>, web: &WebTools) {
    let has_run_command = tools.iter().any(|tool| tool.name == "run_command");
    if !tools.iter().any(|tool| tool.name == TODO_WRITE_TOOL_NAME) {
        tools.push(TodoWriteTool.definition());
    }
    if has_run_command {
        tools.push(JobOutputTool.definition());
        tools.push(JobKillTool.definition());
    }
    tools.extend(web.definitions());
}

/// `web_fetch` and `web_search`, built once and registered on every
/// agent's executor (clones share one HTTP client).
#[derive(Clone)]
pub(crate) struct WebTools {
    /// `None` when the HTTP client could not be created.
    fetch: Option<WebFetchTool>,
    search: WebSearchTool,
}

impl WebTools {
    pub(crate) fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let fetch = WebFetchTool::new(WebFetchSettings::from_lookup(&get))
            .inspect_err(|e| warn!(error = %e, "web_fetch is unavailable"))
            .ok();
        let search_settings = WebSearchSettings::from_lookup(&get);
        match search_settings.provider_name() {
            Some(provider) => info!(provider, "web_search enabled"),
            None => info!("web_search has no provider configured"),
        }
        Self {
            fetch,
            search: WebSearchTool::new(&search_settings),
        }
    }

    /// Register the tools on `executor`.
    pub(crate) fn register(&self, executor: &mut ToolExecutor) {
        if let Some(fetch) = &self.fetch {
            executor.register(Box::new(fetch.clone()));
        }
        executor.register(Box::new(self.search.clone()));
    }

    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = Vec::new();
        if let Some(fetch) = &self.fetch {
            definitions.push(fetch.definition());
        }
        definitions.push(self.search.definition());
        definitions
    }
}

/// Context pressure at which the terminal starts compacting.
const DEFAULT_COMPACTION_TRIGGER: f64 = aura_config::COMPACTION_TRIGGER_DEFAULT;

/// Timeout of a foreground `run_command` that doesn't set one. Builds and
/// test suites routinely take a minute or two; anything longer should set
/// `timeout_ms` or run with `background: true`.
pub(crate) const COMMAND_TIMEOUT_MS: u64 = 2 * 60 * 1000;

/// Longest any single tool call may run, including a foreground command
/// that asks for a long `timeout_ms`.
const MAX_TOOL_RUN: Duration = Duration::from_secs(10 * 60);

/// Limits for one turn, read from the environment once at startup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LongRunSettings {
    /// `AURA_TUI_TURN_TIMEOUT` (`90s`, `45m`, `2h`; a bare number is
    /// minutes). Unset means a turn runs until it is done or cancelled.
    pub turn_timeout: Option<Duration>,
    /// `AURA_TUI_MAX_TURN_COST_USD`: stop a turn whose model calls cost
    /// more than this at list prices.
    pub max_turn_cost_usd: Option<f64>,
    /// `AURA_COMPACTION_TRIGGER` (0.0–1.0).
    pub compaction_trigger: f64,
}

impl Default for LongRunSettings {
    /// The settings with no environment variables set.
    fn default() -> Self {
        Self::from_lookup(|_| None)
    }
}

impl LongRunSettings {
    pub(crate) fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let turn_timeout = get("AURA_TUI_TURN_TIMEOUT").and_then(|raw| {
            let parsed = parse_duration(&raw);
            if parsed.is_none() {
                warn!(value = %raw, "Ignoring unreadable AURA_TUI_TURN_TIMEOUT");
            }
            parsed
        });
        let max_turn_cost_usd = get("AURA_TUI_MAX_TURN_COST_USD").and_then(|raw| {
            let parsed = raw.trim().parse::<f64>().ok().filter(|v| *v > 0.0);
            if parsed.is_none() {
                warn!(value = %raw, "Ignoring unreadable AURA_TUI_MAX_TURN_COST_USD");
            }
            parsed
        });
        let compaction_trigger = get("AURA_COMPACTION_TRIGGER")
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|v| (0.0..=1.0).contains(v))
            .unwrap_or(DEFAULT_COMPACTION_TRIGGER);
        Self {
            turn_timeout,
            max_turn_cost_usd,
            compaction_trigger,
        }
    }

    /// Make `config` run until the work is done.
    pub(crate) fn apply(&self, config: &mut AgentLoopConfig) {
        config.max_iterations = usize::MAX;
        config.max_turns_per_task = u32::MAX;
        config.max_iterations_per_task = u32::MAX;
        config.per_tool_timeout = config.per_tool_timeout.max(MAX_TOOL_RUN);
        config.compaction_trigger = Some(self.compaction_trigger);
        config.continue_open_todos = true;
        // Check after every step that edits files (still once per step): with
        // a cooldown, a fix made in the step after a failed check could end
        // the turn unchecked.
        config.auto_build_cooldown = 0;
    }
}

/// `90s`, `45m`, `2h`, or a bare number of minutes.
pub fn parse_duration(raw: &str) -> Option<Duration> {
    let raw = raw.trim().to_ascii_lowercase();
    let (number, unit_secs) = if let Some(n) = raw.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = raw.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = raw.strip_suffix('s') {
        (n, 1)
    } else {
        (raw.as_str(), 60)
    };
    let value: u64 = number.trim().parse().ok()?;
    (value > 0).then(|| Duration::from_secs(value.saturating_mul(unit_secs)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(vars: &[(&str, &str)]) -> LongRunSettings {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        LongRunSettings::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn defaults_have_no_limits() {
        let s = settings(&[]);
        assert_eq!(s.turn_timeout, None);
        assert_eq!(s.max_turn_cost_usd, None);
        assert!((s.compaction_trigger - DEFAULT_COMPACTION_TRIGGER).abs() < f64::EPSILON);
    }

    #[test]
    fn reads_limits() {
        let s = settings(&[
            ("AURA_TUI_TURN_TIMEOUT", "2h"),
            ("AURA_TUI_MAX_TURN_COST_USD", "5.50"),
            ("AURA_COMPACTION_TRIGGER", "0.8"),
        ]);
        assert_eq!(s.turn_timeout, Some(Duration::from_secs(7200)));
        assert_eq!(s.max_turn_cost_usd, Some(5.5));
        assert!((s.compaction_trigger - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn bare_timeout_is_minutes_and_junk_is_ignored() {
        assert_eq!(
            settings(&[("AURA_TUI_TURN_TIMEOUT", "45")]).turn_timeout,
            Some(Duration::from_secs(45 * 60))
        );
        assert_eq!(
            settings(&[("AURA_TUI_TURN_TIMEOUT", "soon")]).turn_timeout,
            None
        );
        let s = settings(&[("AURA_COMPACTION_TRIGGER", "7")]);
        assert!((s.compaction_trigger - DEFAULT_COMPACTION_TRIGGER).abs() < f64::EPSILON);
    }

    #[test]
    fn web_tools_are_listed_even_without_a_search_provider() {
        let web = WebTools::from_lookup(|_| None);
        let mut tools = Vec::new();
        add_long_run_tools(&mut tools, &web);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["todo_write", "web_fetch", "web_search"]);
    }

    #[test]
    fn apply_lifts_caps() {
        let mut config = AgentLoopConfig::for_agent("claude-test-model");
        settings(&[]).apply(&mut config);
        assert_eq!(config.max_iterations, usize::MAX);
        assert_eq!(config.max_iterations_per_task, u32::MAX);
        assert!(config.continue_open_todos);
        assert_eq!(config.auto_build_cooldown, 0);
        assert!(config.per_tool_timeout >= MAX_TOOL_RUN);
        assert_eq!(config.compaction_trigger, Some(DEFAULT_COMPACTION_TRIGGER));
    }
}
