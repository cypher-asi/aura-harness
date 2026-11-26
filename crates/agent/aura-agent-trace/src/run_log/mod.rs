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

mod assemble;
mod types;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use aura_core_types::trace::TokenUsage;
use aura_store_snapshot::FsSnapshotStore;
use serde_json::Value;

use crate::bundle::{self, blobs_dir, RunBundle};
use crate::health::RunHealth;
use crate::normalize::RunTrace;
use crate::scorecard::RunScorecard;
use crate::TraceError;

use assemble::Ctx;
pub use types::{
    LogEvent, LogFiles, LogRun, LogSpawn, LogStep, LogToolCall, LogTotals, LogTurn, RunLog,
};

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
mod tests;
