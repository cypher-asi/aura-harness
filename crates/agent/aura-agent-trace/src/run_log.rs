//! The run log: one run and the runs it spawned, as one JSON document,
//! `log.json` in the run's bundle, for a person or a model to read when
//! looking for where a session's reasoning went wrong or could be better.
//!
//! Steps are grouped by turn. A model call carries its thinking, its
//! visible text and the tool calls it made; a tool result its output; a
//! harness decision what triggered it. Runs of agents the run spawned
//! are nested under `subagents`, each linked to the call that started
//! it, so a run's log holds every agent below it and a spawned run's log
//! only itself and what it spawned. Runs no call started, such as a
//! benchmark's, can be [`adopt`]ed into a log, and an agent with no run of
//! its own gets a [`group`] of them. Secrets are redacted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aura_core_types::trace::{
    Actor, FileTouch, HarnessAction, Observation, Step, StepAction, TokenUsage, ToolIntent,
};
use aura_store_snapshot::{FsSnapshotStore, SnapshotStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bundle::{
    self, blobs_dir, child_runs, RunBundle, RunMeta, META_FILE, RECORD_FILE, REWARDS_FILE,
    STEPS_FILE, SUMMARY_FILE,
};
use crate::health::{self, RunHealth};
use crate::normalize::RunTrace;
use crate::scorecard::{step_cost_usd, RunScorecard};
use crate::TraceError;

/// Log file name in a run bundle.
pub const LOG_FILE: &str = "log.json";
/// `schema` of the document.
pub const LOG_SCHEMA: &str = "aura.run_log/v1";

/// What the document is, for a reader that has only the file.
const ABOUT: &str = "Log of one AURA agent run and the runs of the agents it spawned. \
`turns` holds each user turn in order; each step is a user prompt, a model call (its thinking, \
visible text and tool calls), a tool result, a harness decision (nudges, blocks, compactions, \
retries) or the turn's end. `subagents` nests the logs of spawned agents, linked by \
`run.spawned_by` to the tool call that started them, and the runs of benchmarks the agent \
holds, which no call started. An agent with no run of its own (`run.source` is `agent`) has \
no turns, only `subagents`. Times are Unix milliseconds; costs are \
estimated list prices in USD. Secrets are replaced by [REDACTED].";

/// Waits on an approval longer than this count as problems in `health`.
const SLOW_APPROVAL_MS: u64 = 30_000;

/// Deepest nesting of spawned runs a log follows.
const MAX_DEPTH: usize = 8;

/// A run loaded for the log, with the bundle directory it is stored in.
pub type LoadedRun = (RunTrace, PathBuf);

/// The whole log of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunLog {
    /// [`LOG_SCHEMA`]; empty on nested logs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema: String,
    /// What the document holds; empty on nested logs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// When the log was written.
    #[serde(default)]
    pub generated_at_ms: u64,
    /// Who ran what.
    pub run: LogRun,
    /// Where the run is stored.
    pub files: LogFiles,
    /// What the run did and cost, its subagents' cost included where said.
    pub totals: LogTotals,
    /// The run's scorecard, as in `summary.json`.
    pub scorecard: RunScorecard,
    /// Calls that never finished, slow approvals and other problems.
    pub health: RunHealth,
    /// Every turn in order.
    pub turns: Vec<LogTurn>,
    /// Logs of the runs this run spawned, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<RunLog>,
    /// Secrets replaced in the whole document.
    #[serde(default)]
    pub redactions: usize,
}

/// Metadata of the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogRun {
    pub run_id: String,
    /// Hex id of the agent whose conversation the run records.
    pub agent_id: Option<String>,
    pub title: String,
    /// Model requested for the run; each call names the model that served it.
    pub model: String,
    pub mode: String,
    /// `tui`, `gateway`, `eval`, ...
    pub source: String,
    pub task_id: Option<String>,
    pub workspace: String,
    /// `decisions` or `full`; at `full`, tool outputs are complete.
    pub trace_level: String,
    pub harness_version: String,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// The run and call that spawned this one.
    pub spawned_by: Option<LogSpawn>,
    /// Run this one was branched from, and the step it starts after.
    pub parent_run: Option<String>,
    pub branch_step: Option<u64>,
    /// Eval arm or experiment label.
    pub arm: Option<String>,
    /// Harness thresholds in effect.
    pub params: Value,
}

/// Where a spawned run came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogSpawn {
    /// The spawning run.
    pub run_id: String,
    /// The spawning call, when it could be told.
    pub tool_use_id: Option<String>,
    /// Turn and step of the spawning run that made the call.
    pub turn: Option<u32>,
    pub step: Option<u64>,
}

/// Files of the run's bundle, as absolute paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFiles {
    /// The bundle directory.
    pub bundle: String,
    /// This document.
    pub log: String,
    pub meta: String,
    /// The raw steps, one JSON object per line.
    pub steps: String,
    /// Kernel record rows joined to the steps.
    pub record: String,
    pub rewards: String,
    pub summary: String,
    /// Blobs of full requests and outputs, when the run kept them.
    pub blobs: Option<String>,
}

/// Counts and cost.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogTotals {
    pub turns: usize,
    pub model_calls: usize,
    pub tool_calls: usize,
    pub tool_errors: usize,
    pub decisions: usize,
    pub usage: TokenUsage,
    /// Estimated cost of this run's model calls.
    pub cost_usd: Option<f64>,
    /// Runs below this one, at any depth.
    pub subagents: usize,
    /// Estimated cost of this run and every run below it.
    pub cost_usd_with_subagents: Option<f64>,
}

/// One user turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogTurn {
    /// 1-based.
    pub turn: u32,
    /// What the user asked; `None` for a turn the log has no prompt of.
    pub prompt: Option<String>,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub model_calls: usize,
    pub tool_calls: usize,
    pub tool_errors: usize,
    pub decisions: usize,
    pub usage: TokenUsage,
    pub cost_usd: Option<f64>,
    /// Why the turn stopped; `None` while it runs.
    pub stop_reason: Option<String>,
    /// Why its last model call failed, when one did.
    pub error: Option<String>,
    pub files_changed: Option<usize>,
    /// Runs spawned in the turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<String>,
    pub steps: Vec<LogStep>,
}

/// One step of a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogStep {
    /// Position in the run, from 1.
    pub seq: u64,
    /// The model call a tool result answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    /// Zero-based model call within the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u32>,
    pub at_ms: u64,
    /// `user`, `model`, `environment` or `harness/<source>`.
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(flatten)]
    pub event: LogEvent,
    /// Rows of the kernel record log the step matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub record_seqs: Vec<u64>,
    /// Steps this one was caused by, as the nudge before a call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caused_by: Vec<u64>,
}

/// What happened at a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogEvent {
    UserPrompt {
        text: String,
    },
    ModelCall {
        /// Model that served the call.
        model: String,
        /// What the harness made the call for (`classify`, `criteria`,
        /// `judge`); `None` for the agent's own calls.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<String>,
        /// The model's reasoning; `None` when it showed none or the run
        /// did not keep it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
        thinking_chars: usize,
        /// The model reasoned but the run kept only how much.
        #[serde(default, skip_serializing_if = "is_false")]
        thinking_omitted: bool,
        /// Visible reply text.
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<LogToolCall>,
        stop_reason: String,
        /// Why the call failed, when it did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        usage: TokenUsage,
        cost_usd: Option<f64>,
    },
    ToolResult {
        tool_use_id: String,
        tool: String,
        is_error: bool,
        /// `executor`, `cache` or `blocked`.
        source: String,
        /// Length of the whole output.
        content_chars: usize,
        /// The output: whole when `output_complete`, else its start.
        output: String,
        output_complete: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<FileTouch>,
    },
    Decision {
        /// Harness component (`steering`, `compaction`, `retry`, ...).
        source: String,
        name: String,
        action: HarnessAction,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        alternatives: Vec<String>,
        /// The condition that fired.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        trigger: Value,
        /// The state the heuristic looked at.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        features: Value,
    },
    TurnEnd {
        stop_reason: String,
        iterations: usize,
        input_tokens: u64,
        output_tokens: u64,
        files_changed: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// A tool call a model made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogToolCall {
    pub tool_use_id: String,
    pub name: String,
    pub intent: ToolIntent,
    pub input: Value,
    /// Run of the agent the call spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_run: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(b: &bool) -> bool {
    !*b
}

impl RunLog {
    /// The run's turn `turn`.
    #[must_use]
    pub fn turn(&self, turn: u32) -> Option<&LogTurn> {
        self.turns.iter().find(|t| t.turn == turn)
    }

    /// This log or one nested in it, by run id.
    #[must_use]
    pub fn find(&self, run_id: &str) -> Option<&Self> {
        if self.run.run_id == run_id {
            return Some(self);
        }
        self.subagents.iter().find_map(|child| child.find(run_id))
    }
}

/// Loads runs from their bundles under `home`.
pub fn bundle_loader(home: &Path) -> impl Fn(&str) -> Option<LoadedRun> + '_ {
    move |run_id| {
        let bundle = RunBundle::find(home, run_id).ok()?;
        let trace = RunTrace::load(&bundle).ok()?;
        Some((trace, bundle.dir().to_path_buf()))
    }
}

/// The log of `trace`, stored in `dir`, with the runs it spawned under
/// `home` loaded by `load`. Without a home only the run itself is in it.
#[must_use]
pub fn build(
    home: Option<&Path>,
    trace: &RunTrace,
    dir: &Path,
    load: &dyn Fn(&str) -> Option<LoadedRun>,
) -> RunLog {
    let blobs = home
        .map(blobs_dir)
        .filter(|d| d.is_dir())
        .and_then(|d| FsSnapshotStore::open(d).ok());
    let ctx = Ctx { home, blobs, load };
    let mut seen = HashSet::from([trace.meta.run_id.clone()]);
    let mut log = ctx.run(trace, dir, &mut seen, 0);
    log.schema = LOG_SCHEMA.to_string();
    log.about = ABOUT.to_string();
    log.generated_at_ms = bundle::now_ms();
    redacted(log)
}

/// The log of the run in `bundle`, its spawned runs read from `home`.
///
/// # Errors
///
/// Returns an error when the run can't be read.
pub fn of_bundle(home: &Path, bundle: &RunBundle) -> Result<RunLog, TraceError> {
    let trace = RunTrace::load(bundle)?;
    Ok(build(
        Some(home),
        &trace,
        bundle.dir(),
        &bundle_loader(home),
    ))
}

/// The log of the agent with hex id `agent`, named `title`, that has no
/// run of its own, such as the agent holding a benchmark: `runs` (the
/// logs of runs that belong to it) under `subagents`, the whole stored in
/// `dir`.
#[must_use]
pub fn group(agent: &str, title: &str, dir: &Path, runs: Vec<RunLog>) -> RunLog {
    let started_at_ms = runs.iter().map(|r| r.run.started_at_ms).min().unwrap_or(0);
    let mut log = RunLog {
        schema: LOG_SCHEMA.to_string(),
        about: ABOUT.to_string(),
        generated_at_ms: bundle::now_ms(),
        run: LogRun {
            run_id: format!("agent-{agent}"),
            agent_id: Some(agent.to_string()),
            title: title.to_string(),
            model: String::new(),
            mode: String::new(),
            source: "agent".to_string(),
            task_id: None,
            workspace: String::new(),
            trace_level: String::new(),
            harness_version: String::new(),
            started_at_ms,
            ended_at_ms: None,
            duration_ms: None,
            spawned_by: None,
            parent_run: None,
            branch_step: None,
            arm: None,
            params: Value::Null,
        },
        files: LogFiles {
            bundle: dir.display().to_string(),
            log: dir.join(LOG_FILE).display().to_string(),
            meta: String::new(),
            steps: String::new(),
            record: String::new(),
            rewards: String::new(),
            summary: String::new(),
            blobs: None,
        },
        totals: LogTotals::default(),
        scorecard: RunScorecard::default(),
        health: RunHealth::default(),
        turns: Vec::new(),
        subagents: Vec::new(),
        redactions: 0,
    };
    adopt(&mut log, runs);
    log
}

/// Nest `runs` under `log`, beside the runs it spawned: runs that belong
/// to the same agent without a call of its run starting them, such as a
/// benchmark's. Runs already in `log` are skipped.
pub fn adopt(log: &mut RunLog, runs: Vec<RunLog>) {
    for mut run in runs {
        if log.find(&run.run.run_id).is_some() {
            continue;
        }
        run.schema.clear();
        run.about.clear();
        log.redactions += std::mem::take(&mut run.redactions);
        log.totals.subagents += 1 + run.totals.subagents;
        log.totals.cost_usd_with_subagents = sum(
            log.totals.cost_usd_with_subagents,
            run.totals.cost_usd_with_subagents,
        );
        log.subagents.push(run);
    }
}

/// Write `log` to `log.json` in `dir`, returning its path.
///
/// # Errors
///
/// Returns an I/O or JSON error.
pub fn write(dir: &Path, log: &RunLog) -> Result<PathBuf, TraceError> {
    let path = dir.join(LOG_FILE);
    bundle::write_json_atomic(&path, log)?;
    Ok(path)
}

/// `AURA_HOME` of a bundle in its `runs` directory.
#[must_use]
pub fn home_of(dir: &Path) -> Option<&Path> {
    let runs = dir.parent()?;
    (runs.file_name()? == "runs")
        .then(|| runs.parent())
        .flatten()
}

struct Ctx<'a> {
    home: Option<&'a Path>,
    blobs: Option<FsSnapshotStore>,
    load: &'a dyn Fn(&str) -> Option<LoadedRun>,
}

impl Ctx<'_> {
    fn run(
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
        let mut subagents = Vec::new();
        for child in &children {
            if !seen.insert(child.run_id.clone()) {
                continue;
            }
            let Some((child_trace, child_dir)) = (self.load)(&child.run_id) else {
                continue;
            };
            let mut log = self.run(&child_trace, &child_dir, seen, depth + 1);
            let call = spawns.get(&child.run_id).cloned();
            let (turn, step) = call
                .as_deref()
                .and_then(|call| step_of_call(&trace.steps, call))
                .map_or_else(
                    || (turn_at(&trace.steps, child.started_at_ms), None),
                    |(turn, seq)| (Some(turn), Some(seq)),
                );
            if let Some(t) = turn.and_then(|n| turns.iter_mut().find(|t| t.turn == n)) {
                t.subagents.push(child.run_id.clone());
            }
            log.run.spawned_by = Some(LogSpawn {
                run_id: meta.run_id.clone(),
                tool_use_id: call,
                turn,
                step,
            });
            subagents.push(log);
        }
        let mut totals = LogTotals {
            turns: turns.len(),
            ..LogTotals::default()
        };
        for turn in &turns {
            totals.model_calls += turn.model_calls;
            totals.tool_calls += turn.tool_calls;
            totals.tool_errors += turn.tool_errors;
            totals.decisions += turn.decisions;
            totals.usage = add(totals.usage, turn.usage);
            totals.cost_usd = sum(totals.cost_usd, turn.cost_usd);
        }
        totals.cost_usd_with_subagents = totals.cost_usd;
        for child in &subagents {
            totals.subagents += 1 + child.totals.subagents;
            totals.cost_usd_with_subagents = sum(
                totals.cost_usd_with_subagents,
                child.totals.cost_usd_with_subagents,
            );
        }
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

    fn turns(&self, steps: &[Step], call_of: &HashMap<&str, &str>) -> Vec<LogTurn> {
        let mut turns: Vec<LogTurn> = Vec::new();
        for step in steps {
            if turns.last().is_none_or(|t| t.turn != step.turn) {
                turns.push(LogTurn {
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
                });
            }
            let Some(turn) = turns.last_mut() else {
                continue;
            };
            let event = self.event(step, call_of);
            match &event {
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
            turn.steps.push(LogStep {
                seq: step.seq,
                parent: step.parent,
                iteration: step.iteration,
                at_ms: step.at_ms,
                actor: actor_label(&step.actor),
                duration_ms: step.duration_ms,
                event,
                record_seqs: step.links.record_seqs.clone(),
                caused_by: step.links.caused_by.clone(),
            });
        }
        turns
    }

    fn event(&self, step: &Step, call_of: &HashMap<&str, &str>) -> LogEvent {
        match &step.action {
            StepAction::UserPrompt { text } => LogEvent::UserPrompt { text: text.clone() },
            StepAction::Model(action) => {
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
                    tool_calls: action
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
                        .collect(),
                    stop_reason: action.stop_reason.clone(),
                    error: action.error.clone(),
                    usage,
                    cost_usd: step_cost_usd(&step.policy.id, usage),
                }
            }
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
            } => {
                let (trigger, features) = match &step.observation {
                    Observation::Harness { features, trigger } => {
                        (trigger.clone(), features.clone())
                    }
                    _ => (Value::Null, Value::Null),
                };
                LogEvent::Decision {
                    source: match step.actor {
                        Actor::Harness { source } => source.as_str().to_string(),
                        _ => "harness".to_string(),
                    },
                    name: name.clone(),
                    action: action.clone(),
                    alternatives: alternatives.clone(),
                    trigger,
                    features,
                }
            }
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

    fn blob(&self, id: &str) -> Option<String> {
        let bytes = self.blobs.as_ref()?.get(id).ok().flatten()?;
        String::from_utf8(bytes.to_vec()).ok()
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

const fn add(a: TokenUsage, b: TokenUsage) -> TokenUsage {
    TokenUsage {
        input: a.input + b.input,
        output: a.output + b.output,
        cache_write: a.cache_write + b.cache_write,
        cache_read: a.cache_read + b.cache_read,
    }
}

fn sum(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}

/// `log` with secrets replaced in every string.
fn redacted(log: RunLog) -> RunLog {
    let Ok(mut value) = serde_json::to_value(&log) else {
        return log;
    };
    let mut count = 0;
    crate::dataset::redact_value(&mut value, &mut count);
    if count == 0 {
        return log;
    }
    match serde_json::from_value::<RunLog>(value) {
        Ok(mut redacted) => {
            redacted.redactions = count;
            redacted
        }
        Err(e) => {
            tracing::warn!(error = %e, "run log redaction failed");
            log
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::RunSource;
    use aura_core_types::trace::{
        DecisionSource, ModelAction, PolicyKind, PolicyRef, StepLinks, ToolCallAction,
    };
    use serde_json::json;

    fn step(run: &str, seq: u64, turn: u32, actor: Actor, action: StepAction) -> Step {
        Step {
            run_id: run.into(),
            seq,
            parent: None,
            turn,
            iteration: Some(0),
            at_ms: 1_000 + seq * 10,
            actor,
            observation: Observation::None,
            action,
            policy: PolicyRef {
                kind: PolicyKind::Model,
                id: "claude/claude-sonnet-5-5".into(),
                ..PolicyRef::default()
            },
            links: StepLinks::default(),
            usage: None,
            duration_ms: None,
        }
    }

    fn prompt(run: &str, seq: u64, turn: u32, text: &str) -> Step {
        step(
            run,
            seq,
            turn,
            Actor::User,
            StepAction::UserPrompt { text: text.into() },
        )
    }

    fn call(run: &str, seq: u64, turn: u32, id: &str, tool: &str, input: Value) -> Step {
        let mut s = step(
            run,
            seq,
            turn,
            Actor::Model,
            StepAction::Model(ModelAction {
                thinking_chars: 12,
                thinking_text: "check the file".into(),
                text: "Reading it.".into(),
                tool_calls: vec![ToolCallAction {
                    tool_use_id: id.into(),
                    name: tool.into(),
                    intent: ToolIntent::classify(tool, &input),
                    input,
                }],
                stop_reason: "tool_use".into(),
                ..ModelAction::default()
            }),
        );
        s.usage = Some(TokenUsage {
            input: 100,
            output: 20,
            ..TokenUsage::default()
        });
        s
    }

    fn end(run: &str, seq: u64, turn: u32) -> Step {
        step(
            run,
            seq,
            turn,
            Actor::Environment,
            StepAction::TurnEnd {
                stop_reason: "end_turn".into(),
                iterations: 1,
                input_tokens: 100,
                output_tokens: 20,
                files_changed: 0,
                error: None,
            },
        )
    }

    fn save(home: &Path, meta: &RunMeta, steps: &[Step]) -> RunBundle {
        let bundle = RunBundle::create(home, meta).unwrap();
        bundle::append_jsonl(&bundle.path(STEPS_FILE), steps).unwrap();
        bundle
    }

    #[test]
    fn groups_steps_by_turn_with_thinking_and_totals() {
        let home = tempfile::tempdir().unwrap();
        let meta = RunMeta::new("claude/claude-sonnet-5-5", RunSource::Test);
        let run = meta.run_id.clone();
        let mut old = call(&run, 5, 2, "t2", "read_file", json!({"path": "b.rs"}));
        if let StepAction::Model(action) = &mut old.action {
            action.thinking_text.clear();
        }
        let mut nudge = step(
            &run,
            6,
            2,
            Actor::Harness {
                source: DecisionSource::Steering,
            },
            StepAction::Harness {
                name: "implement_now".into(),
                action: HarnessAction::Nudge {
                    kind: "implement".into(),
                },
                alternatives: Vec::new(),
            },
        );
        nudge.observation = Observation::Harness {
            features: json!({"reads": 9}),
            trigger: json!({"threshold": 8}),
        };
        let bundle = save(
            home.path(),
            &meta,
            &[
                prompt(&run, 1, 1, "fix the bug"),
                call(&run, 2, 1, "t1", "read_file", json!({"path": "a.rs"})),
                end(&run, 3, 1),
                prompt(&run, 4, 2, "and the other"),
                old,
                nudge,
                end(&run, 7, 2),
            ],
        );
        let log = of_bundle(home.path(), &bundle).unwrap();
        assert_eq!(log.schema, LOG_SCHEMA);
        assert_eq!(log.turns.len(), 2);
        let first = &log.turns[0];
        assert_eq!(first.prompt.as_deref(), Some("fix the bug"));
        assert_eq!(first.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!((first.model_calls, first.tool_calls), (1, 1));
        match &first.steps[1].event {
            LogEvent::ModelCall {
                thinking,
                thinking_omitted,
                ..
            } => {
                assert_eq!(thinking.as_deref(), Some("check the file"));
                assert!(!thinking_omitted);
            }
            other => panic!("not a model call: {other:?}"),
        }
        match &log.turns[1].steps[1].event {
            LogEvent::ModelCall {
                thinking,
                thinking_omitted,
                ..
            } => {
                assert!(thinking.is_none(), "an older run kept no thinking text");
                assert!(thinking_omitted);
            }
            other => panic!("not a model call: {other:?}"),
        }
        match &log.turns[1].steps[2].event {
            LogEvent::Decision {
                source, trigger, ..
            } => {
                assert_eq!(source, "steering");
                assert_eq!(trigger["threshold"], 8);
            }
            other => panic!("not a decision: {other:?}"),
        }
        assert_eq!(log.turns[1].decisions, 1);
        assert_eq!(log.totals.model_calls, 2);
        assert_eq!(log.totals.usage.input, 200);
        assert!(log.totals.cost_usd.is_some_and(|c| c > 0.0));
        assert_eq!(log.files.bundle, bundle.dir().display().to_string());
        assert!(log.files.log.ends_with(LOG_FILE));

        let path = write(bundle.dir(), &log).unwrap();
        let written = std::fs::read_to_string(path).unwrap();
        assert!(written.starts_with("{\n  \"schema\""), "pretty-printed");
        let back: RunLog = serde_json::from_str(&written).unwrap();
        assert_eq!(back, log);
    }

    #[test]
    fn an_agent_without_a_run_groups_the_runs_that_belong_to_it() {
        let home = tempfile::tempdir().unwrap();
        let logs: Vec<RunLog> = (0..2)
            .map(|n| {
                let mut meta = RunMeta::new("m", RunSource::Bench);
                meta.title = format!("a-{n}");
                let id = meta.run_id.clone();
                let bundle = save(
                    home.path(),
                    &meta,
                    &[
                        prompt(&id, 1, 1, "write it"),
                        call(&id, 2, 1, "t1", "write_file", json!({"path": "a"})),
                        end(&id, 3, 1),
                    ],
                );
                of_bundle(home.path(), &bundle).unwrap()
            })
            .collect();
        let again = logs[0].clone();
        let dir = home.path().join("logs/abc");
        let mut log = group("abc", "Compare #1", &dir, logs);
        adopt(&mut log, vec![again]);

        assert_eq!(log.schema, LOG_SCHEMA);
        assert_eq!(log.run.title, "Compare #1");
        assert_eq!(log.run.source, "agent");
        assert!(log.turns.is_empty());
        assert_eq!(log.files.log, dir.join(LOG_FILE).display().to_string());
        let titles: Vec<&str> = log.subagents.iter().map(|r| r.run.title.as_str()).collect();
        assert_eq!(titles, ["a-0", "a-1"], "each run once");
        assert!(log.subagents.iter().all(|r| r.schema.is_empty()));
        assert_eq!(log.totals.subagents, 2);
        let each = log.subagents[0].totals.cost_usd_with_subagents.unwrap();
        assert!((log.totals.cost_usd_with_subagents.unwrap() - 2.0 * each).abs() < 1e-9);
    }

    #[test]
    fn nests_spawned_runs_under_the_call_that_started_them() {
        let home = tempfile::tempdir().unwrap();
        let parent = RunMeta::new("m", RunSource::Test);
        let pid = parent.run_id.clone();
        let task = json!({"prompt": "count files"});
        let bundle = save(
            home.path(),
            &parent,
            &[
                prompt(&pid, 1, 1, "delegate"),
                call(&pid, 2, 1, "t1", "task", task),
                end(&pid, 3, 1),
            ],
        );
        let mut child = RunMeta::new("m", RunSource::Test);
        child.title = "count files".into();
        child.spawned_by = Some(pid.clone());
        child.started_at_ms = parent.started_at_ms + 1;
        let cid = child.run_id.clone();
        let child_bundle = save(
            home.path(),
            &child,
            &[prompt(&cid, 1, 1, "count files"), end(&cid, 2, 1)],
        );
        let mut grandchild = RunMeta::new("m", RunSource::Test);
        grandchild.spawned_by = Some(cid.clone());
        save(
            home.path(),
            &grandchild,
            &[prompt(&grandchild.run_id, 1, 1, "x")],
        );

        let log = of_bundle(home.path(), &bundle).unwrap();
        assert_eq!(log.subagents.len(), 1);
        assert_eq!(log.totals.subagents, 2, "the grandchild counts too");
        let sub = &log.subagents[0];
        assert_eq!(sub.run.run_id, cid);
        let spawn = sub.run.spawned_by.as_ref().unwrap();
        assert_eq!(spawn.tool_use_id.as_deref(), Some("t1"));
        assert_eq!((spawn.turn, spawn.step), (Some(1), Some(2)));
        assert_eq!(log.turns[0].subagents, vec![cid.clone()]);
        assert!(log.find(&grandchild.run_id).is_some());
        match &log.turns[0].steps[1].event {
            LogEvent::ModelCall { tool_calls, .. } => {
                assert_eq!(tool_calls[0].child_run.as_deref(), Some(cid.as_str()));
            }
            other => panic!("not a model call: {other:?}"),
        }

        let only_child = of_bundle(home.path(), &child_bundle).unwrap();
        assert_eq!(only_child.run.run_id, cid);
        assert_eq!(only_child.subagents.len(), 1, "only what it spawned");
        assert_eq!(home_of(child_bundle.dir()), Some(home.path()));
    }

    #[test]
    fn secrets_are_redacted() {
        let home = tempfile::tempdir().unwrap();
        let meta = RunMeta::new("m", RunSource::Test);
        let run = meta.run_id.clone();
        let bundle = save(
            home.path(),
            &meta,
            &[prompt(&run, 1, 1, "use api_key=sk-abcdefghijklmnop please")],
        );
        let log = of_bundle(home.path(), &bundle).unwrap();
        assert!(log.redactions > 0);
        let text = log.turns[0].prompt.as_deref().unwrap();
        assert!(text.contains("[REDACTED]") && !text.contains("sk-abcdefghijklmnop"));
    }
}
