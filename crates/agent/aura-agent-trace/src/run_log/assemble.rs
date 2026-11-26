//! Building a [`RunLog`] from a run's trace and the runs it spawned.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use aura_core_types::trace::{
    Actor, HarnessAction, ModelAction, Observation, Step, StepAction, TokenUsage,
};
use aura_store_snapshot::{FsSnapshotStore, SnapshotStore};
use serde_json::Value;

use super::{
    add, sum, LoadedRun, LogEvent, LogFiles, LogRun, LogSpawn, LogStep, LogToolCall, LogTotals,
    LogTurn, RunLog, LOG_FILE, MAX_DEPTH, SLOW_APPROVAL_MS,
};
use crate::bundle::{
    self, blobs_dir, child_runs, RunMeta, META_FILE, RECORD_FILE, REWARDS_FILE, STEPS_FILE,
    SUMMARY_FILE,
};
use crate::health;
use crate::normalize::RunTrace;
use crate::scorecard::step_cost_usd;

pub(super) struct Ctx<'a> {
    pub(super) home: Option<&'a Path>,
    pub(super) blobs: Option<FsSnapshotStore>,
    pub(super) load: &'a dyn Fn(&str) -> Option<LoadedRun>,
}

/// The run whose children are being nested, and what links them to it.
struct Parent<'a> {
    trace: &'a RunTrace,
    spawns: &'a HashMap<String, String>,
}

impl Ctx<'_> {
    pub(super) fn run(
        &self,
        trace: &RunTrace,
        dir: &Path,
        seen: &mut HashSet<String>,
        depth: usize,
    ) -> RunLog {
        let meta = &trace.meta;
        let children = match self.home {
            Some(home) if depth < MAX_DEPTH => child_runs(home, &meta.run_id),
            _ => Vec::new(),
        };
        let spawns = spawning_calls(trace, &children);
        let call_of: HashMap<&str, &str> = spawns
            .iter()
            .map(|(child, call)| (call.as_str(), child.as_str()))
            .collect();
        let mut turns = self.turns(&trace.steps, &call_of);
        let parent = Parent {
            trace,
            spawns: &spawns,
        };
        let subagents = self.subagents(&parent, &children, &mut turns, seen, depth);
        let totals = totals_of(&turns, &subagents);
        RunLog {
            schema: String::new(),
            about: String::new(),
            generated_at_ms: 0,
            run: run_of(meta),
            files: files_of(dir, self.home),
            totals,
            scorecard: trace.scorecard(),
            health: health::check(trace, bundle::now_ms(), SLOW_APPROVAL_MS),
            turns,
            subagents,
            redactions: 0,
        }
    }

    /// Logs of `children` not yet `seen`, each linked to the call and turn
    /// of `parent` that spawned it.
    fn subagents(
        &self,
        parent: &Parent<'_>,
        children: &[RunMeta],
        turns: &mut [LogTurn],
        seen: &mut HashSet<String>,
        depth: usize,
    ) -> Vec<RunLog> {
        let mut subagents = Vec::new();
        for child in children {
            if !seen.insert(child.run_id.clone()) {
                continue;
            }
            let Some((child_trace, child_dir)) = (self.load)(&child.run_id) else {
                continue;
            };
            let mut log = self.run(&child_trace, &child_dir, seen, depth + 1);
            log.run.spawned_by = Some(spawn_of(parent, child, turns));
            subagents.push(log);
        }
        subagents
    }

    fn turns(&self, steps: &[Step], call_of: &HashMap<&str, &str>) -> Vec<LogTurn> {
        let mut turns: Vec<LogTurn> = Vec::new();
        for step in steps {
            if turns.last().is_none_or(|t| t.turn != step.turn) {
                turns.push(new_turn(step));
            }
            let Some(turn) = turns.last_mut() else {
                continue;
            };
            let event = self.event(step, call_of);
            tally(turn, step, &event);
            turn.steps.push(log_step(step, event));
        }
        turns
    }

    fn event(&self, step: &Step, call_of: &HashMap<&str, &str>) -> LogEvent {
        match &step.action {
            StepAction::UserPrompt { text } => LogEvent::UserPrompt { text: text.clone() },
            StepAction::Model(action) => self.model_call(step, action, call_of),
            StepAction::ToolResult {
                tool_use_id,
                tool,
                is_error,
                source,
                content_chars,
                excerpt,
                content,
                files,
            } => {
                let full = content.as_ref().and_then(|id| self.blob(id));
                let output_complete = full.is_some() || excerpt.chars().count() >= *content_chars;
                LogEvent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    tool: tool.clone(),
                    is_error: *is_error,
                    source: source.clone(),
                    content_chars: *content_chars,
                    output: full.unwrap_or_else(|| excerpt.clone()),
                    output_complete,
                    files: files.clone(),
                }
            }
            StepAction::Harness {
                name,
                action,
                alternatives,
            } => decision(step, name, action, alternatives),
            StepAction::TurnEnd {
                stop_reason,
                iterations,
                input_tokens,
                output_tokens,
                files_changed,
                error,
            } => LogEvent::TurnEnd {
                stop_reason: stop_reason.clone(),
                iterations: *iterations,
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                files_changed: *files_changed,
                error: error.clone(),
            },
        }
    }

    fn model_call(
        &self,
        step: &Step,
        action: &ModelAction,
        call_of: &HashMap<&str, &str>,
    ) -> LogEvent {
        let thinking = if action.thinking_text.is_empty() {
            action.thinking.as_ref().and_then(|id| self.blob(id))
        } else {
            Some(action.thinking_text.clone())
        };
        let usage = step.usage.unwrap_or_default();
        LogEvent::ModelCall {
            model: step.policy.id.clone(),
            purpose: action.purpose.clone(),
            thinking_omitted: thinking.is_none() && action.thinking_chars > 0,
            thinking,
            thinking_chars: action.thinking_chars,
            text: action.text.clone(),
            tool_calls: tool_calls_of(action, call_of),
            stop_reason: action.stop_reason.clone(),
            error: action.error.clone(),
            usage,
            cost_usd: step_cost_usd(&step.policy.id, usage),
        }
    }

    fn blob(&self, id: &str) -> Option<String> {
        let bytes = self.blobs.as_ref()?.get(id).ok().flatten()?;
        String::from_utf8(bytes.to_vec()).ok()
    }
}

/// Where `child` came from in `parent`, noting it on the turn that
/// spawned it.
fn spawn_of(parent: &Parent<'_>, child: &RunMeta, turns: &mut [LogTurn]) -> LogSpawn {
    let steps = &parent.trace.steps;
    let call = parent.spawns.get(&child.run_id).cloned();
    let (turn, step) = call
        .as_deref()
        .and_then(|call| step_of_call(steps, call))
        .map_or_else(
            || (turn_at(steps, child.started_at_ms), None),
            |(turn, seq)| (Some(turn), Some(seq)),
        );
    if let Some(t) = turn.and_then(|n| turns.iter_mut().find(|t| t.turn == n)) {
        t.subagents.push(child.run_id.clone());
    }
    LogSpawn {
        run_id: parent.trace.meta.run_id.clone(),
        tool_use_id: call,
        turn,
        step,
    }
}

/// Totals of `turns`, with the runs and cost of `subagents` added.
fn totals_of(turns: &[LogTurn], subagents: &[RunLog]) -> LogTotals {
    let mut totals = LogTotals {
        turns: turns.len(),
        ..LogTotals::default()
    };
    for turn in turns {
        totals.model_calls += turn.model_calls;
        totals.tool_calls += turn.tool_calls;
        totals.tool_errors += turn.tool_errors;
        totals.decisions += turn.decisions;
        totals.usage = add(totals.usage, turn.usage);
        totals.cost_usd = sum(totals.cost_usd, turn.cost_usd);
    }
    totals.cost_usd_with_subagents = totals.cost_usd;
    for child in subagents {
        totals.subagents += 1 + child.totals.subagents;
        totals.cost_usd_with_subagents = sum(
            totals.cost_usd_with_subagents,
            child.totals.cost_usd_with_subagents,
        );
    }
    totals
}

/// An empty turn starting at `step`.
fn new_turn(step: &Step) -> LogTurn {
    LogTurn {
        turn: step.turn,
        prompt: None,
        started_at_ms: step.at_ms,
        ended_at_ms: None,
        duration_ms: None,
        model_calls: 0,
        tool_calls: 0,
        tool_errors: 0,
        decisions: 0,
        usage: TokenUsage::default(),
        cost_usd: None,
        stop_reason: None,
        error: None,
        files_changed: None,
        subagents: Vec::new(),
        steps: Vec::new(),
    }
}

/// Count `event`, seen at `step`, into `turn`.
fn tally(turn: &mut LogTurn, step: &Step, event: &LogEvent) {
    match event {
        LogEvent::UserPrompt { text } => {
            turn.prompt.get_or_insert_with(|| text.clone());
        }
        LogEvent::ModelCall {
            tool_calls,
            usage,
            cost_usd,
            ..
        } => {
            turn.model_calls += 1;
            turn.tool_calls += tool_calls.len();
            turn.usage = add(turn.usage, *usage);
            turn.cost_usd = sum(turn.cost_usd, *cost_usd);
        }
        LogEvent::ToolResult { is_error, .. } => {
            turn.tool_errors += usize::from(*is_error);
        }
        LogEvent::Decision { .. } => turn.decisions += 1,
        LogEvent::TurnEnd {
            stop_reason,
            files_changed,
            error,
            ..
        } => {
            turn.ended_at_ms = Some(step.at_ms);
            turn.duration_ms = Some(step.at_ms.saturating_sub(turn.started_at_ms));
            turn.stop_reason = Some(stop_reason.clone());
            turn.files_changed = Some(*files_changed);
            turn.error.clone_from(error);
        }
    }
}

fn log_step(step: &Step, event: LogEvent) -> LogStep {
    LogStep {
        seq: step.seq,
        parent: step.parent,
        iteration: step.iteration,
        at_ms: step.at_ms,
        actor: actor_label(&step.actor),
        duration_ms: step.duration_ms,
        event,
        record_seqs: step.links.record_seqs.clone(),
        caused_by: step.links.caused_by.clone(),
    }
}

/// The tool calls of `action`, each with the run it spawned.
fn tool_calls_of(action: &ModelAction, call_of: &HashMap<&str, &str>) -> Vec<LogToolCall> {
    action
        .tool_calls
        .iter()
        .map(|call| LogToolCall {
            tool_use_id: call.tool_use_id.clone(),
            name: call.name.clone(),
            intent: call.intent,
            input: call.input.clone(),
            child_run: call_of
                .get(call.tool_use_id.as_str())
                .map(|run| (*run).to_string()),
        })
        .collect()
}

fn decision(step: &Step, name: &str, action: &HarnessAction, alternatives: &[String]) -> LogEvent {
    let (trigger, features) = match &step.observation {
        Observation::Harness { features, trigger } => (trigger.clone(), features.clone()),
        _ => (Value::Null, Value::Null),
    };
    LogEvent::Decision {
        source: match step.actor {
            Actor::Harness { source } => source.as_str().to_string(),
            _ => "harness".to_string(),
        },
        name: name.to_string(),
        action: action.clone(),
        alternatives: alternatives.to_vec(),
        trigger,
        features,
    }
}

/// The call of `trace` each of `children` was spawned by, by child run id.
fn spawning_calls(trace: &RunTrace, children: &[RunMeta]) -> HashMap<String, String> {
    if children.is_empty() {
        return HashMap::new();
    }
    let mut graph = trace.graph();
    graph.link_child_runs(children);
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            let call = node.id.strip_prefix("tc:")?;
            let child = node.meta.get("child_run")?.as_str()?;
            Some((child.to_string(), call.to_string()))
        })
        .collect()
}

/// Turn and step of the model call that made tool call `call`.
fn step_of_call(steps: &[Step], call: &str) -> Option<(u32, u64)> {
    steps.iter().find_map(|step| match &step.action {
        StepAction::Model(action) if action.tool_calls.iter().any(|c| c.tool_use_id == call) => {
            Some((step.turn, step.seq))
        }
        _ => None,
    })
}

/// The turn under way at `at_ms`.
fn turn_at(steps: &[Step], at_ms: u64) -> Option<u32> {
    steps
        .iter()
        .take_while(|step| step.at_ms <= at_ms)
        .last()
        .map(|step| step.turn)
}

fn actor_label(actor: &Actor) -> String {
    match actor {
        Actor::Harness { source } => format!("harness/{}", source.as_str()),
        other => other.label().to_string(),
    }
}

fn run_of(meta: &RunMeta) -> LogRun {
    LogRun {
        run_id: meta.run_id.clone(),
        agent_id: meta.agent_id.clone(),
        title: meta.title.clone(),
        model: meta.model.clone(),
        mode: meta.mode.clone(),
        source: meta.source.as_str().to_string(),
        task_id: meta.task_id.clone(),
        workspace: meta.workspace.clone(),
        trace_level: meta.trace_level.clone(),
        harness_version: meta.harness_version.clone(),
        started_at_ms: meta.started_at_ms,
        ended_at_ms: meta.ended_at_ms,
        duration_ms: meta
            .ended_at_ms
            .map(|end| end.saturating_sub(meta.started_at_ms)),
        spawned_by: meta.spawned_by.as_ref().map(|run_id| LogSpawn {
            run_id: run_id.clone(),
            tool_use_id: None,
            turn: None,
            step: None,
        }),
        parent_run: meta.parent_run.clone(),
        branch_step: meta.branch_step,
        arm: meta.arm.clone(),
        params: serde_json::to_value(meta.params).unwrap_or_default(),
    }
}

fn files_of(dir: &Path, home: Option<&Path>) -> LogFiles {
    let path = |name: &str| dir.join(name).display().to_string();
    LogFiles {
        bundle: dir.display().to_string(),
        log: path(LOG_FILE),
        meta: path(META_FILE),
        steps: path(STEPS_FILE),
        record: path(RECORD_FILE),
        rewards: path(REWARDS_FILE),
        summary: path(SUMMARY_FILE),
        blobs: home
            .map(blobs_dir)
            .filter(|d| d.is_dir())
            .map(|d| d.display().to_string()),
    }
}
