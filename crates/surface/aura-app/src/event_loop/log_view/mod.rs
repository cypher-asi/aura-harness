//! The LOG tab's requests: the run log of the agent on screen and of every
//! agent below it, written to the run's `log.json` and laid out as a
//! [`LogView`].

use std::path::Path;
use std::sync::Arc;

mod event;
mod run;
mod step;
mod summary;

use aura_agent_trace::RunLog;
use aura_core_types::AgentId;
use aura_ui::events::{LogField, LogRequest, LogView};
use aura_ui::UiCommand;
use serde::Serialize;

use super::subagents::lock;
use super::LoopState;
use crate::agent_registry::{AgentRegistry, RegisteredAgent};
use run::push_run;

/// A request from the LOG tab shown for `agent_id`'s chat.
pub(super) async fn handle(state: &LoopState<'_>, agent_id: AgentId, request: LogRequest) {
    let LogRequest::Load = request;
    let traces = Arc::clone(&state.hub.traces);
    let hex = agent_id.to_hex();
    let (name, also) = {
        let registry = lock(&state.hub.registry);
        let name = registry
            .find(agent_id)
            .map(|agent| agent.display_name().into_owned())
            .unwrap_or_default();
        let also = benchmark_runs(&registry, agent_id, state.compare.running_host());
        (name, also)
    };
    let built = tokio::task::spawn_blocking(move || traces.run_log(agent_id, &name, &also))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    let command = match built {
        Ok((log, path)) => UiCommand::LogView(Box::new(view(&hex, &log, &path))),
        Err(notice) => UiCommand::LogNotice {
            agent_id: hex.clone(),
            notice,
        },
    };
    let _ = state
        .commands
        .send(UiCommand::ForAgent {
            agent_id: hex,
            command: Box::new(command),
        })
        .await;
}

/// Most benchmark runs a log takes in, newest kept.
const MAX_BENCH_RUNS: usize = 64;

/// The agents whose runs the log of `agent` takes in beside the runs it
/// spawned: those of a benchmark below it, which no call of its run
/// started. On the agent holding a benchmark, or one of its arms, that
/// benchmark's (or arm's) runs; on the agent that asked for benchmarks,
/// those of the one `running`, else of its newest.
pub(super) fn benchmark_runs(
    registry: &AgentRegistry,
    agent: AgentId,
    running: Option<AgentId>,
) -> Vec<AgentId> {
    let Some(found) = registry.find(agent) else {
        return Vec::new();
    };
    let held = if found.is_compare() || found.is_compare_arm() {
        Some(agent)
    } else {
        let below: Vec<AgentId> = registry
            .compares()
            .iter()
            .map(|host| host.agent_id())
            .filter(|&host| registry.ancestry(host).contains(&agent))
            .collect();
        running
            .filter(|host| below.contains(host))
            .or_else(|| below.last().copied())
    };
    let Some(held) = held else {
        return Vec::new();
    };
    let below = registry.descendants(held);
    let mut runs: Vec<AgentId> = registry
        .agents()
        .iter()
        .filter(|agent| !agent.is_compare_arm())
        .map(RegisteredAgent::agent_id)
        .filter(|id| below.contains(id))
        .collect();
    let extra = runs.len().saturating_sub(MAX_BENCH_RUNS);
    runs.drain(..extra);
    runs
}

/// `log`, written to `path`, as the LOG tab of agent `agent` shows it.
pub(super) fn view(agent: &str, log: &RunLog, path: &Path) -> LogView {
    let mut runs = Vec::new();
    push_run(&mut runs, log, 0);
    LogView {
        agent_id: agent.to_string(),
        path: path.display().to_string(),
        json: pretty(log),
        runs,
    }
}

fn pretty<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// Whether `prompt` says what the run's `title`, its prompt's start,
/// already does.
fn repeats(title: &str, prompt: &str) -> bool {
    let title = title.trim_end_matches('…');
    let prompt = prompt.trim_end_matches('…');
    !title.is_empty() && (prompt.starts_with(title) || title.starts_with(prompt))
}

/// `sonnet-5-5` for `claude/claude-sonnet-5-5`.
fn short_model(model: &str) -> &str {
    let name = model.rsplit('/').next().unwrap_or(model);
    name.strip_prefix("claude-").unwrap_or(name)
}

fn field(label: &str, value: impl Into<String>) -> LogField {
    (label.to_string(), value.into())
}

fn time(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || ms.to_string(),
            |at| {
                at.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            },
        )
}

#[cfg(test)]
mod tests;
