//! On-disk run bundles.
//!
//! One directory per run under `AURA_HOME/runs/<run_id>/`:
//!
//! - `meta.json`: who ran what, with which model and harness params
//! - `steps.jsonl`: one [`Step`] per line, append-only
//! - `rewards.jsonl`: one [`Reward`] per line, append-only
//! - `summary.json`: the [`RunScorecard`], written when the run ends
//!
//! Blobs are shared across runs in `AURA_HOME/blobs/` so identical
//! prompts and tool schemas are stored once.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aura_config::HarnessParams;
use aura_core_types::trace::{Reward, Step, TRACE_SCHEMA_VERSION};
use aura_store_snapshot::{FsSnapshotStore, SnapshotStore};
use serde::{Deserialize, Serialize};

use crate::scorecard::RunScorecard;
use crate::TraceError;

/// Metadata file name.
pub const META_FILE: &str = "meta.json";
/// Step log file name.
pub const STEPS_FILE: &str = "steps.jsonl";
/// Reward log file name.
pub const REWARDS_FILE: &str = "rewards.jsonl";
/// Scorecard file name.
pub const SUMMARY_FILE: &str = "summary.json";
/// Kernel record rows file.
pub const RECORD_FILE: &str = "record.jsonl";

/// Where a run came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunSource {
    /// Interactive terminal session.
    #[default]
    Tui,
    /// Gateway chat session.
    Gateway,
    /// Automaton task (dev loop, task run).
    Automaton,
    /// Eval suite.
    Eval,
    /// Counterfactual branch of another run.
    Branch,
    /// A run of a benchmark (`/compare`, the BENCH tab), on one of its arms.
    Bench,
    /// Test fixture.
    Test,
}

impl RunSource {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Gateway => "gateway",
            Self::Automaton => "automaton",
            Self::Eval => "eval",
            Self::Branch => "branch",
            Self::Bench => "bench",
            Self::Test => "test",
        }
    }
}

/// Contents of `meta.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunMeta {
    /// Trajectory format version.
    pub schema_version: u32,
    /// Unique run id.
    pub run_id: String,
    /// Unix milliseconds at start.
    pub started_at_ms: u64,
    /// Unix milliseconds at finish, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    /// Harness version and, when known, git sha.
    pub harness_version: String,
    /// Model requested for the run.
    pub model: String,
    /// Agent mode (`agent`, `plan`, ...).
    #[serde(default)]
    pub mode: String,
    /// Where the run came from.
    #[serde(default)]
    pub source: RunSource,
    /// Task id for automaton and eval runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Short task title or first prompt line.
    #[serde(default)]
    pub title: String,
    /// Workspace root.
    #[serde(default)]
    pub workspace: String,
    /// Capture level (`decisions` or `full`).
    #[serde(default)]
    pub trace_level: String,
    /// Harness thresholds in effect.
    #[serde(default)]
    pub params: HarnessParams,
    /// Whether the run may be used for training exports.
    #[serde(default = "default_true")]
    pub consent: bool,
    /// Run this one was branched from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run: Option<String>,
    /// Step of the parent run the branch starts after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_step: Option<u64>,
    /// Eval arm or experiment label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    /// Probability the arm was chosen with, for randomized evals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm_propensity: Option<f64>,
    /// Hex id of the agent whose conversation this run records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Run of the agent that spawned this one, for spawned agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<String>,
}

const fn default_true() -> bool {
    true
}

impl RunMeta {
    /// New metadata with a fresh run id and the current params.
    #[must_use]
    pub fn new(model: impl Into<String>, source: RunSource) -> Self {
        Self {
            schema_version: TRACE_SCHEMA_VERSION,
            run_id: new_run_id(),
            started_at_ms: now_ms(),
            ended_at_ms: None,
            harness_version: harness_version(),
            model: model.into(),
            mode: String::new(),
            source,
            task_id: None,
            title: String::new(),
            workspace: String::new(),
            trace_level: aura_config::trace().level.as_str().to_string(),
            params: HarnessParams::current(),
            consent: true,
            parent_run: None,
            branch_step: None,
            arm: None,
            arm_propensity: None,
            agent_id: None,
            spawned_by: None,
        }
    }
}

/// Harness version string recorded in every bundle.
#[must_use]
pub fn harness_version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match option_env!("AURA_GIT_SHA") {
        Some(sha) if !sha.is_empty() => format!("{version}+{sha}"),
        _ => version.to_string(),
    }
}

/// Sortable, unique run id: start time in hex plus random suffix.
#[must_use]
pub fn new_run_id() -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{:011x}-{}", now_ms(), &suffix[..8])
}

/// Current unix time in milliseconds.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// Directory holding every run bundle.
#[must_use]
pub fn runs_dir(home: &Path) -> PathBuf {
    home.join("runs")
}

/// Directory holding shared trace blobs.
#[must_use]
pub fn blobs_dir(home: &Path) -> PathBuf {
    home.join("blobs")
}

/// Kernel snapshot store over the shared blob directory, so full
/// AuditedLite effect payloads dedupe with trace blobs.
pub fn kernel_snapshot_store(home: &Path) -> Result<Arc<dyn SnapshotStore>, TraceError> {
    Ok(Arc::new(FsSnapshotStore::open(blobs_dir(home))?))
}

/// A run directory on disk.
#[derive(Debug, Clone)]
pub struct RunBundle {
    dir: PathBuf,
}

impl RunBundle {
    /// Bundle at `dir` (not checked).
    #[must_use]
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Create the bundle directory for `meta` under `home` and write
    /// `meta.json`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory or file can't be written.
    pub fn create(home: &Path, meta: &RunMeta) -> Result<Self, TraceError> {
        let dir = runs_dir(home).join(&meta.run_id);
        fs::create_dir_all(&dir)?;
        let bundle = Self { dir };
        bundle.write_meta(meta)?;
        Ok(bundle)
    }

    /// Find a run by exact id or unique id prefix.
    ///
    /// # Errors
    ///
    /// Returns [`TraceError::NotFound`] or [`TraceError::Ambiguous`].
    pub fn find(home: &Path, id: &str) -> Result<Self, TraceError> {
        let exact = runs_dir(home).join(id);
        if exact.join(META_FILE).is_file() {
            return Ok(Self::at(exact));
        }
        let matches: Vec<PathBuf> = list_run_dirs(home)
            .into_iter()
            .filter(|d| {
                d.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(id) || n.ends_with(id))
            })
            .collect();
        match matches.len() {
            0 => Err(TraceError::NotFound(id.to_string())),
            1 => Ok(Self::at(matches.into_iter().next().unwrap_or_default())),
            n => Err(TraceError::Ambiguous(id.to_string(), n)),
        }
    }

    /// Bundle directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of a file in the bundle.
    #[must_use]
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Overwrite `meta.json`.
    ///
    /// # Errors
    ///
    /// Returns an I/O or JSON error.
    pub fn write_meta(&self, meta: &RunMeta) -> Result<(), TraceError> {
        write_json_atomic(&self.path(META_FILE), meta)
    }

    /// Read `meta.json`.
    ///
    /// # Errors
    ///
    /// Returns an I/O or JSON error.
    pub fn read_meta(&self) -> Result<RunMeta, TraceError> {
        Ok(serde_json::from_slice(&fs::read(self.path(META_FILE))?)?)
    }

    /// Read every step. Lines that fail to parse (for example a line
    /// cut off by a crash) are skipped.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but can't be read.
    pub fn read_steps(&self) -> Result<Vec<Step>, TraceError> {
        read_jsonl(&self.path(STEPS_FILE))
    }

    /// Read every stored reward.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but can't be read.
    pub fn read_rewards(&self) -> Result<Vec<Reward>, TraceError> {
        read_jsonl(&self.path(REWARDS_FILE))
    }

    /// Read `record.jsonl` (missing file → empty).
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but can't be read.
    pub fn read_records(&self) -> Result<Vec<crate::records::RecordRow>, TraceError> {
        read_jsonl(&self.path(RECORD_FILE))
    }

    /// Append kernel record rows to `record.jsonl`.
    ///
    /// # Errors
    ///
    /// Returns an I/O or JSON error.
    pub fn append_records(&self, rows: &[crate::records::RecordRow]) -> Result<(), TraceError> {
        append_jsonl(&self.path(RECORD_FILE), rows)
    }

    /// Append rewards to `rewards.jsonl`.
    ///
    /// # Errors
    ///
    /// Returns an I/O or JSON error.
    pub fn append_rewards(&self, rewards: &[Reward]) -> Result<(), TraceError> {
        append_jsonl(&self.path(REWARDS_FILE), rewards)
    }

    /// Read `summary.json`, if the run has finished.
    #[must_use]
    pub fn read_summary(&self) -> Option<RunScorecard> {
        let bytes = fs::read(self.path(SUMMARY_FILE)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write `summary.json`.
    ///
    /// # Errors
    ///
    /// Returns an I/O or JSON error.
    pub fn write_summary(&self, card: &RunScorecard) -> Result<(), TraceError> {
        write_json_atomic(&self.path(SUMMARY_FILE), card)
    }
}

/// Every run directory under `home`, oldest first.
#[must_use]
pub fn list_run_dirs(home: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(runs_dir(home)) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join(META_FILE).is_file())
        .collect();
    dirs.sort();
    dirs
}

/// Metadata and scorecard (when finished) of every run, oldest first.
#[must_use]
pub fn list_runs(home: &Path) -> Vec<(RunMeta, Option<RunScorecard>)> {
    list_run_dirs(home)
        .into_iter()
        .filter_map(|dir| {
            let bundle = RunBundle::at(dir);
            let meta = bundle.read_meta().ok()?;
            Some((meta, bundle.read_summary()))
        })
        .collect()
}

/// The newest `limit` runs under `home` that belong to the agent with hex
/// id `agent` ([`crate::db::owner_of`]), newest first.
#[must_use]
pub fn runs_of(home: &Path, agent: &str, limit: usize) -> Vec<RunBundle> {
    list_run_dirs(home)
        .into_iter()
        .rev()
        .map(RunBundle::at)
        .filter(|bundle| {
            bundle
                .read_meta()
                .ok()
                .and_then(|meta| crate::db::owner_of(&meta))
                .as_deref()
                == Some(agent)
        })
        .take(limit)
        .collect()
}

/// Metadata of the runs spawned by run `run_id`, oldest first. Run ids
/// sort by start time, so only later runs are read.
#[must_use]
pub fn child_runs(home: &Path, run_id: &str) -> Vec<RunMeta> {
    let started = run_id.split('-').next().unwrap_or(run_id);
    list_run_dirs(home)
        .into_iter()
        .filter(|dir| dir.file_name().is_some_and(|name| *name >= *started))
        .filter_map(|dir| RunBundle::at(dir).read_meta().ok())
        .filter(|meta| meta.spawned_by.as_deref() == Some(run_id))
        .collect()
}

pub(crate) fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>, TraceError> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(v) => out.push(v),
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "skipping bad trace line");
            }
        }
    }
    Ok(out)
}

pub(crate) fn append_jsonl<T: Serialize>(path: &Path, items: &[T]) -> Result<(), TraceError> {
    if items.is_empty() {
        return Ok(());
    }
    let mut buf = Vec::new();
    for item in items {
        serde_json::to_writer(&mut buf, item)?;
        buf.push(b'\n');
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&buf)?;
    Ok(())
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), TraceError> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_find_and_list() {
        let home = tempfile::tempdir().unwrap();
        let meta = RunMeta::new("m", RunSource::Test);
        let bundle = RunBundle::create(home.path(), &meta).unwrap();
        assert_eq!(bundle.read_meta().unwrap(), meta);
        let found = RunBundle::find(home.path(), &meta.run_id[..8]).unwrap();
        assert_eq!(found.dir(), bundle.dir());
        assert_eq!(list_runs(home.path()).len(), 1);
        assert!(matches!(
            RunBundle::find(home.path(), "zzz"),
            Err(TraceError::NotFound(_))
        ));
    }

    #[test]
    fn jsonl_skips_truncated_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.jsonl");
        fs::write(&path, "{\"a\":1}\n{\"a\":").unwrap();
        let rows: Vec<serde_json::Value> = read_jsonl(&path).unwrap();
        assert_eq!(rows.len(), 1);
    }
}
