//! The LOG tab's requests: the run log of the agent on screen and of every
//! agent below it, written to the run's `log.json` and laid out as a
//! [`LogView`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

mod summary;

use aura_agent_trace::run_log::{LogEvent, LogStep, LogToolCall, LogTurn};
use aura_agent_trace::RunLog;
use aura_core_types::trace::HarnessAction;
use aura_core_types::AgentId;
use aura_ui::events::{
    LogField, LogRequest, LogRunView, LogSection, LogStatus, LogStepView, LogTurnView, LogView,
};
use aura_ui::UiCommand;
use serde::Serialize;

use super::compare::view::{duration, headline, money, tokens};
use super::subagents::lock;
use super::LoopState;
use crate::agent_registry::{AgentRegistry, RegisteredAgent};

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

/// Add `log` and the runs below it to `runs`, each after its spawner.
/// Returns the index of `log`'s run.
fn push_run(runs: &mut Vec<LogRunView>, log: &RunLog, depth: usize) -> usize {
    let index = runs.len();
    runs.push(LogRunView::default());
    let mut below: HashMap<&str, usize> = HashMap::new();
    for child in &log.subagents {
        below.insert(&child.run.run_id, push_run(runs, child, depth + 1));
    }
    let mut turns: Vec<LogTurnView> = log
        .turns
        .iter()
        .map(|turn| turn_view(turn, &below))
        .collect();
    let in_turns: Vec<&str> = log
        .turns
        .iter()
        .flat_map(|t| t.subagents.iter().map(String::as_str))
        .collect();
    let others: Vec<usize> = log
        .subagents
        .iter()
        .filter(|child| !in_turns.contains(&child.run.run_id.as_str()))
        .filter_map(|child| below.get(child.run.run_id.as_str()).copied())
        .collect();
    let run = &log.run;
    let title = if run.title.is_empty() {
        format!("run {}", run.run_id)
    } else {
        run.title.clone()
    };
    for turn in &mut turns {
        if repeats(&title, &turn.prompt) {
            turn.prompt.clear();
        }
    }
    let grouped = |status: LogStatus| others.iter().any(|&i| runs[i].status == status);
    let status = if log.turns.is_empty() && !others.is_empty() {
        // An agent with no run of its own is as its runs are.
        [LogStatus::Running, LogStatus::Error, LogStatus::Warn]
            .into_iter()
            .find(|&status| grouped(status))
            .unwrap_or(LogStatus::Ok)
    } else if log.turns.iter().any(|t| t.error.is_some()) {
        LogStatus::Error
    } else if run.ended_at_ms.is_none() || log.turns.last().is_some_and(|t| t.stop_reason.is_none())
    {
        LogStatus::Running
    } else if log.health.problems.is_empty() {
        LogStatus::Ok
    } else {
        LogStatus::Warn
    };
    let mut detail: Vec<String> = Some(short_model(&run.model))
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .into_iter()
        .collect();
    if log.turns.is_empty() && !others.is_empty() {
        detail.push(count(others.len(), "run"));
    } else {
        detail.push(count(log.turns.len(), "turn"));
    }
    if let Some(cost) = log.totals.cost_usd_with_subagents {
        detail.push(money(cost));
    }
    if let Some(ms) = run.duration_ms {
        detail.push(duration(ms));
    }
    if log.totals.subagents > 0 {
        detail.push(count(log.totals.subagents, "subagent"));
    }
    runs[index] = LogRunView {
        run_id: run.run_id.clone(),
        title,
        detail: detail.join(" · "),
        status,
        depth,
        fields: run_fields(log),
        files: file_fields(log),
        problems: log.health.problems.clone(),
        turns,
        runs: others,
        json: pretty(log),
    };
    index
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

fn run_fields(log: &RunLog) -> Vec<LogField> {
    let run = &log.run;
    let totals = &log.totals;
    let card = &log.scorecard;
    if run.source == "agent" {
        let mut fields = vec![field("agent", run.agent_id.as_deref().unwrap_or_default())];
        fields.push(field("runs", totals.subagents.to_string()));
        if let Some(cost) = totals.cost_usd_with_subagents {
            fields.push(field("cost", money(cost)));
        }
        return fields;
    }
    let mut fields = vec![field("run", &run.run_id)];
    if let Some(agent) = &run.agent_id {
        fields.push(field("agent", agent));
    }
    fields.push(field("model", &run.model));
    let source = if run.mode.is_empty() {
        run.source.clone()
    } else {
        format!("{} · {}", run.source, run.mode)
    };
    fields.push(field("source", source));
    if !run.workspace.is_empty() {
        fields.push(field("workspace", &run.workspace));
    }
    fields.push(field("started", time(run.started_at_ms)));
    fields.push(field(
        "ended",
        run.ended_at_ms.map_or_else(|| "running".to_string(), time),
    ));
    if let Some(ms) = run.duration_ms {
        fields.push(field("wall time", duration(ms)));
    }
    if let Some(spawn) = &run.spawned_by {
        let mut by = format!("run {}", spawn.run_id);
        if let Some(turn) = spawn.turn {
            by.push_str(&format!(", turn {turn}"));
        }
        if let Some(step) = spawn.step {
            by.push_str(&format!(", step {step}"));
        }
        fields.push(field("spawned by", by));
    }
    fields.push(field(
        "calls",
        format!(
            "{} model, {} tool ({} failed), {} harness decisions",
            totals.model_calls, totals.tool_calls, totals.tool_errors, totals.decisions
        ),
    ));
    fields.push(field(
        "tokens",
        format!(
            "{} in, {} out, {} cache read, {} cache write",
            tokens(totals.usage.input),
            tokens(totals.usage.output),
            tokens(totals.usage.cache_read),
            tokens(totals.usage.cache_write)
        ),
    ));
    if let Some(cost) = totals.cost_usd {
        fields.push(field("cost", money(cost)));
    }
    if totals.subagents > 0 {
        fields.push(field("subagents", totals.subagents.to_string()));
        if let Some(cost) = totals.cost_usd_with_subagents {
            fields.push(field("cost with subagents", money(cost)));
        }
    }
    fields.push(field(
        "work",
        format!(
            "{} reads ({} redundant), {} writes, {} failed edits, {} files changed",
            card.reads, card.redundant_reads, card.writes, card.failed_edits, card.files_changed
        ),
    ));
    if card.blocked_calls + card.retries + card.cache_hits > 0 {
        fields.push(field(
            "harness",
            format!(
                "{} blocked, {} retries, {} cache hits",
                card.blocked_calls, card.retries, card.cache_hits
            ),
        ));
    }
    if let Some(ms) = card.ms_to_first_edit {
        fields.push(field("first edit after", duration(ms)));
    }
    if let Some(outcome) = card.outcome {
        fields.push(field("outcome", format!("{outcome:+.2}")));
    }
    fields.push(field("trace level", &run.trace_level));
    fields.push(field("harness version", &run.harness_version));
    fields
}

fn file_fields(log: &RunLog) -> Vec<LogField> {
    let files = &log.files;
    let mut fields = vec![
        field("log", &files.log),
        field("folder", &files.bundle),
        field("steps", &files.steps),
        field("record", &files.record),
        field("summary", &files.summary),
    ];
    if let Some(blobs) = &files.blobs {
        fields.push(field("blobs", blobs));
    }
    fields.retain(|(_, path)| !path.is_empty());
    fields
}

fn turn_view(turn: &LogTurn, below: &HashMap<&str, usize>) -> LogTurnView {
    let calls: HashMap<&str, &LogToolCall> = turn
        .steps
        .iter()
        .filter_map(|step| match &step.event {
            LogEvent::ModelCall { tool_calls, .. } => Some(tool_calls),
            _ => None,
        })
        .flatten()
        .map(|call| (call.tool_use_id.as_str(), call))
        .collect();
    let steps: Vec<LogStepView> = turn
        .steps
        .iter()
        .map(|s| step_view(s, below, &calls))
        .collect();
    let named: Vec<usize> = steps.iter().flat_map(|s| s.spawned.clone()).collect();
    let spawned = turn
        .subagents
        .iter()
        .filter_map(|run| below.get(run.as_str()).copied())
        .filter(|i| !named.contains(i))
        .collect();
    let detail = [
        count(turn.model_calls, "call"),
        turn.stop_reason
            .clone()
            .unwrap_or_else(|| "running".to_string()),
    ];
    let status = if turn.error.is_some() {
        LogStatus::Error
    } else if turn.stop_reason.is_none() {
        LogStatus::Running
    } else if turn.tool_errors > 0 || steps.iter().any(|s| s.status == LogStatus::Warn) {
        LogStatus::Warn
    } else {
        LogStatus::Ok
    };
    LogTurnView {
        turn: turn.turn,
        prompt: turn
            .prompt
            .as_deref()
            .map_or_else(String::new, |p| headline(p, 200)),
        detail: detail.join(" · "),
        tokens: format!(
            "{} tok",
            tokens(turn.usage.input + turn.usage.output + turn.usage.cache_read)
        ),
        cost: turn.cost_usd.map(money).unwrap_or_default(),
        took: turn.duration_ms.map(duration).unwrap_or_default(),
        status,
        steps,
        spawned,
        json: pretty(turn),
    }
}

fn section(title: &str, body: impl Into<String>, code: bool) -> LogSection {
    LogSection {
        title: title.to_string(),
        body: body.into(),
        code,
    }
}

/// What a step's row shows beside its number and columns.
struct Row {
    kind: &'static str,
    label: String,
    summary: String,
    status: LogStatus,
}

impl Row {
    fn new(kind: &'static str, label: String, summary: String, status: LogStatus) -> Self {
        Self {
            kind,
            label,
            summary,
            status,
        }
    }
}

/// `step` laid out, its tool results labelled by the call they answer in
/// `calls`, by tool use id.
#[allow(clippy::too_many_lines)]
fn step_view(
    step: &LogStep,
    below: &HashMap<&str, usize>,
    calls: &HashMap<&str, &LogToolCall>,
) -> LogStepView {
    let mut fields = vec![
        field("seq", step.seq.to_string()),
        field("time", time(step.at_ms)),
        field("actor", &step.actor),
    ];
    if let Some(i) = step.iteration {
        fields.push(field("iteration", i.to_string()));
    }
    if let Some(ms) = step.duration_ms {
        fields.push(field("took", duration(ms)));
    }
    let mut sections = Vec::new();
    let mut spawned = Vec::new();
    let (mut tokens_used, mut cost) = (String::new(), String::new());
    let row = match &step.event {
        LogEvent::UserPrompt { text } => {
            sections.push(section("Prompt", text, false));
            Row::new("prompt", headline(text, 160), String::new(), LogStatus::Ok)
        }
        LogEvent::ModelCall {
            model,
            purpose,
            thinking,
            thinking_chars,
            thinking_omitted,
            text,
            tool_calls,
            stop_reason,
            error,
            usage,
            cost_usd,
        } => {
            fields.push(field("model", model));
            if let Some(purpose) = purpose {
                fields.push(field("for", purpose));
            }
            fields.push(field("stop reason", stop_reason));
            if let Some(error) = error {
                sections.push(section("Error", error, false));
            }
            fields.push(field(
                "tokens",
                format!(
                    "{} in, {} out, {} cache read, {} cache write",
                    tokens(usage.input),
                    tokens(usage.output),
                    tokens(usage.cache_read),
                    tokens(usage.cache_write)
                ),
            ));
            if let Some(cost) = cost_usd {
                fields.push(field("cost", money(*cost)));
            }
            match thinking {
                Some(thinking) => sections.push(section("Thinking", thinking, false)),
                None if *thinking_omitted => sections.push(section(
                    "Thinking",
                    format!("{thinking_chars} characters, not kept by this run"),
                    false,
                )),
                None => {}
            }
            if !text.is_empty() {
                sections.push(section("Reply", text, false));
            }
            for call in tool_calls {
                let input = serde_json::to_string_pretty(&call.input).unwrap_or_default();
                let mut title = format!("Call {} ({})", call.name, call.tool_use_id);
                if let Some(child) = &call.child_run {
                    title.push_str(&format!(" → run {child}"));
                    spawned.extend(below.get(child.as_str()).copied());
                }
                sections.push(section(&title, input, true));
            }
            let label = if let Some(purpose) = purpose {
                format!("gate {purpose}")
            } else if !tool_calls.is_empty() {
                let calls: Vec<String> = tool_calls
                    .iter()
                    .map(|call| summary::call_label(&call.name, &call.input))
                    .collect();
                calls.join(" · ")
            } else if text.trim().is_empty() {
                "no reply".to_string()
            } else {
                headline(text, 160)
            };
            tokens_used = format!(
                "{} tok",
                tokens(usage.input + usage.output + usage.cache_read)
            );
            cost = cost_usd.map(money).unwrap_or_default();
            if let Some(error) = error {
                Row::new("model", label, headline(error, 160), LogStatus::Error)
            } else if stop_reason == "max_tokens" {
                Row::new(
                    "model",
                    label,
                    "hit its output cap".to_string(),
                    LogStatus::Warn,
                )
            } else {
                Row::new("model", label, String::new(), LogStatus::Ok)
            }
        }
        LogEvent::ToolResult {
            tool_use_id,
            tool,
            is_error,
            source,
            content_chars,
            output,
            output_complete,
            files,
        } => {
            fields.push(field("tool", tool));
            fields.push(field("call", tool_use_id));
            fields.push(field("from", source));
            fields.push(field("output", format!("{content_chars} characters")));
            for file in files {
                fields.push(field(
                    "file",
                    format!(
                        "{} {} (+{} -{})",
                        file.kind, file.path, file.added, file.removed
                    ),
                ));
            }
            let title = if *output_complete {
                "Output"
            } else {
                "Output (start; AURA_TRACE=full keeps it whole)"
            };
            sections.push(section(title, output, true));
            let label = calls.get(tool_use_id.as_str()).map_or_else(
                || tool.clone(),
                |call| summary::call_label(&call.name, &call.input),
            );
            let said = summary::result_summary(&summary::Output {
                tool,
                text: output,
                complete: *output_complete,
                chars: *content_chars,
                is_error: *is_error,
                source,
                files,
            });
            let status = if *is_error {
                LogStatus::Error
            } else if source == "blocked" {
                LogStatus::Warn
            } else {
                LogStatus::Ok
            };
            Row::new("tool", label, said, status)
        }
        LogEvent::Decision {
            source,
            name,
            action,
            alternatives,
            trigger,
            features,
        } => {
            fields.push(field("source", source));
            fields.push(field("decision", name));
            fields.push(field("action", action.label()));
            if !alternatives.is_empty() {
                fields.push(field("alternatives", alternatives.join(", ")));
            }
            sections.push(section(
                "Action",
                serde_json::to_string_pretty(action).unwrap_or_default(),
                true,
            ));
            if !trigger.is_null() {
                sections.push(section("Trigger", pretty(trigger), true));
            }
            if !features.is_null() {
                sections.push(section("Features", pretty(features), true));
            }
            let status = if matches!(
                action,
                HarnessAction::Block { .. }
                    | HarnessAction::Stop { .. }
                    | HarnessAction::Retry { .. }
            ) {
                LogStatus::Warn
            } else {
                LogStatus::Ok
            };
            Row::new(
                "decision",
                format!("{source}/{name} {}", action.label()),
                String::new(),
                status,
            )
        }
        LogEvent::TurnEnd {
            stop_reason,
            iterations,
            input_tokens,
            output_tokens,
            files_changed,
            error,
        } => {
            fields.push(field("stop reason", stop_reason));
            fields.push(field("model calls", iterations.to_string()));
            fields.push(field(
                "tokens",
                format!(
                    "{} in, {} out",
                    tokens(*input_tokens),
                    tokens(*output_tokens)
                ),
            ));
            fields.push(field("files changed", files_changed.to_string()));
            if let Some(error) = error {
                sections.push(section("Error", error, true));
            }
            let label = format!("turn ended: {stop_reason}");
            match error {
                Some(error) => Row::new(
                    "end",
                    label,
                    format!("error: {}", headline(error, 72)),
                    LogStatus::Error,
                ),
                None if *files_changed > 0 => Row::new(
                    "end",
                    label,
                    count(*files_changed, "file") + " changed",
                    LogStatus::Ok,
                ),
                None => Row::new("end", label, String::new(), LogStatus::Ok),
            }
        }
    };
    if !step.record_seqs.is_empty() {
        let seqs: Vec<String> = step.record_seqs.iter().map(u64::to_string).collect();
        fields.push(field("kernel record", seqs.join(", ")));
    }
    if !step.caused_by.is_empty() {
        let seqs: Vec<String> = step.caused_by.iter().map(u64::to_string).collect();
        fields.push(field("caused by step", seqs.join(", ")));
    }
    LogStepView {
        seq: step.seq,
        kind: row.kind.to_string(),
        label: row.label,
        summary: row.summary,
        parent: step.parent.filter(|_| row.kind == "tool"),
        tokens: tokens_used,
        cost,
        took: step.duration_ms.map(duration).unwrap_or_default(),
        status: row.status,
        fields,
        sections,
        spawned,
        json: pretty(step),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_agent_trace::bundle::{RunBundle, RunSource, STEPS_FILE};
    use aura_agent_trace::run_log;
    use aura_agent_trace::RunMeta;
    use aura_core_types::trace::{
        Actor, ModelAction, Observation, PolicyRef, Step, StepAction, StepLinks, ToolCallAction,
        ToolIntent,
    };
    use serde_json::json;

    fn step(run: &str, seq: u64, actor: Actor, action: StepAction) -> Step {
        Step {
            run_id: run.into(),
            seq,
            parent: None,
            turn: 1,
            iteration: Some(0),
            at_ms: 1_000 + seq,
            actor,
            observation: Observation::None,
            action,
            policy: PolicyRef::default(),
            links: StepLinks::default(),
            usage: None,
            duration_ms: None,
        }
    }

    fn save(home: &Path, meta: &RunMeta, steps: &[Step]) -> RunBundle {
        let bundle = RunBundle::create(home, meta).unwrap();
        let lines: Vec<String> = steps
            .iter()
            .map(|s| serde_json::to_string(s).unwrap())
            .collect();
        std::fs::write(bundle.path(STEPS_FILE), lines.join("\n") + "\n").unwrap();
        bundle
    }

    #[test]
    fn a_run_and_its_subagent_are_laid_out_with_their_files() {
        let home = tempfile::tempdir().unwrap();
        let parent = RunMeta::new("claude/claude-sonnet-5-5", RunSource::Test);
        let pid = parent.run_id.clone();
        let input = json!({"prompt": "count files"});
        let bundle = save(
            home.path(),
            &parent,
            &[
                step(
                    &pid,
                    1,
                    Actor::User,
                    StepAction::UserPrompt {
                        text: "delegate it".into(),
                    },
                ),
                step(
                    &pid,
                    2,
                    Actor::Model,
                    StepAction::Model(ModelAction {
                        thinking_text: "a subagent can count".into(),
                        thinking_chars: 20,
                        tool_calls: vec![ToolCallAction {
                            tool_use_id: "t1".into(),
                            name: "task".into(),
                            intent: ToolIntent::classify("task", &input),
                            input,
                        }],
                        stop_reason: "tool_use".into(),
                        ..ModelAction::default()
                    }),
                ),
            ],
        );
        let mut child = RunMeta::new("m", RunSource::Test);
        child.title = "count files".into();
        child.spawned_by = Some(pid.clone());
        save(home.path(), &child, &[]);

        let log = run_log::of_bundle(home.path(), &bundle).unwrap();
        let view = view("ab", &log, &bundle.path(run_log::LOG_FILE));
        assert_eq!(view.runs.len(), 2);
        let root = view.root().unwrap();
        assert_eq!(root.depth, 0);
        assert_eq!(view.runs[1].depth, 1);
        assert!(root
            .files
            .iter()
            .any(|(label, path)| label == "log" && path.ends_with("log.json")));
        let call = &root.turns[0].steps[1];
        assert_eq!(call.spawned, vec![1], "the task call names its subagent");
        assert!(call
            .sections
            .iter()
            .any(|s| s.title == "Thinking" && s.body == "a subagent can count"));
        assert_eq!(root.status, LogStatus::Running, "the turn has not ended");
        assert!(view.json.contains("\"schema\": \"aura.run_log/v1\""));
    }

    #[test]
    fn a_tool_result_reads_as_the_call_it_answers_and_what_came_of_it() {
        let home = tempfile::tempdir().unwrap();
        let mut meta = RunMeta::new("m", RunSource::Test);
        meta.title = "list the crates".into();
        let id = meta.run_id.clone();
        let input = json!({"path": "crates"});
        let listing = "dir\t4096\tagent\ndir\t4096\tui";
        let mut result = step(
            &id,
            3,
            Actor::Environment,
            StepAction::ToolResult {
                tool_use_id: "t1".into(),
                tool: "list_files".into(),
                is_error: false,
                source: "executor".into(),
                content_chars: listing.len(),
                excerpt: listing.into(),
                content: None,
                files: Vec::new(),
            },
        );
        result.parent = Some(2);
        result.duration_ms = Some(19);
        let mut call = step(
            &id,
            2,
            Actor::Model,
            StepAction::Model(ModelAction {
                tool_calls: vec![ToolCallAction {
                    tool_use_id: "t1".into(),
                    name: "list_files".into(),
                    intent: ToolIntent::classify("list_files", &input),
                    input,
                }],
                stop_reason: "tool_use".into(),
                ..ModelAction::default()
            }),
        );
        call.duration_ms = Some(6_000);
        let prompt = StepAction::UserPrompt {
            text: "list the crates".into(),
        };
        let bundle = save(
            home.path(),
            &meta,
            &[step(&id, 1, Actor::User, prompt), call, result],
        );

        let log = run_log::of_bundle(home.path(), &bundle).unwrap();
        let view = view("ab", &log, &bundle.path(run_log::LOG_FILE));
        let turn = &view.root().unwrap().turns[0];
        assert_eq!(turn.prompt, "", "the run's title says it");
        let (call, result) = (&turn.steps[1], &turn.steps[2]);
        assert_eq!(call.label, "list_files crates");
        assert_eq!((call.took.as_str(), call.parent), ("6s", None));
        assert!(call.tokens.ends_with(" tok"), "{:?}", call.tokens);
        assert_eq!(result.label, "list_files crates");
        assert_eq!(result.summary, "2 entries");
        assert_eq!((result.took.as_str(), result.parent), ("19ms", Some(2)));
        assert!(
            result.sections.iter().any(|s| s.body == listing),
            "kept whole"
        );
    }
}
