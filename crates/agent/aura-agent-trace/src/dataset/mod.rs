//! Training-data export from run bundles.
//!
//! Formats:
//!
//! - `sft`: successful trajectories as provider-neutral chat with tool
//!   calls (`role` / `content` / `tool_calls` / `tool_call_id`)
//! - `preference`: chosen / rejected trajectory pairs for the same task
//!   (including branches of one run) with different outcomes
//! - `rl`: one record per step with observation, action, reward,
//!   discounted return, done flag, policy and propensity
//! - `harness-bandit`: one record per harness decision with features,
//!   action, params, propensity and measured effect
//!
//! Every export redacts secrets, skips runs without consent and eval
//! runs (unless asked), splits train / eval by task so a task never
//! appears on both sides, deduplicates identical records, and writes a
//! `dataset_card.json` describing what went in.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aura_core_types::trace::TRACE_SCHEMA_VERSION;
use aura_store_snapshot::FsSnapshotStore;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bundle::{blobs_dir, list_run_dirs, RunBundle, RunSource};
use crate::normalize::RunTrace;
use crate::TraceError;

mod records;

use records::{preference_pairs, records_for, task_key};

/// Export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    /// Supervised fine-tuning chats.
    Sft,
    /// Chosen / rejected pairs.
    Preference,
    /// Per-step reinforcement-learning records.
    Rl,
    /// Per-decision harness records.
    HarnessBandit,
}

impl ExportFormat {
    /// Parse `sft`, `preference`, `rl` or `harness-bandit`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "sft" => Some(Self::Sft),
            "preference" | "pref" | "dpo" => Some(Self::Preference),
            "rl" => Some(Self::Rl),
            "harness-bandit" | "bandit" => Some(Self::HarnessBandit),
            _ => None,
        }
    }

    /// Stable kebab-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sft => "sft",
            Self::Preference => "preference",
            Self::Rl => "rl",
            Self::HarnessBandit => "harness-bandit",
        }
    }
}

/// Which runs to export.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportFilter {
    /// Minimum run outcome (`sft` defaults to requiring `> 0`).
    pub min_outcome: Option<f64>,
    /// Only runs of this model.
    pub model: Option<String>,
    /// Only runs from this source.
    pub source: Option<RunSource>,
    /// Include eval-suite and benchmark runs (excluded by default so the
    /// suites stay held out).
    pub include_eval: bool,
    /// Share of tasks assigned to the eval split, in `[0, 1]`.
    pub eval_fraction: f64,
}

impl ExportFilter {
    /// Parse `key=value` pairs separated by commas: `outcome>=0.5`,
    /// `model=claude-opus`, `source=tui`, `eval=include`,
    /// `eval_fraction=0.1`.
    ///
    /// # Errors
    ///
    /// Returns [`TraceError::Invalid`] for unknown keys or bad values.
    pub fn parse(expr: &str) -> Result<Self, TraceError> {
        let mut filter = Self {
            eval_fraction: 0.1,
            ..Self::default()
        };
        for part in expr.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let bad = || TraceError::Invalid(format!("bad filter `{part}`"));
            if let Some(v) = part.strip_prefix("outcome>=") {
                filter.min_outcome = Some(v.parse().map_err(|_| bad())?);
            } else if let Some((k, v)) = part.split_once('=') {
                match k.trim() {
                    "model" => filter.model = Some(v.trim().to_string()),
                    "source" => {
                        filter.source = Some(
                            serde_json::from_value(Value::String(v.trim().to_string()))
                                .map_err(|_| bad())?,
                        );
                    }
                    "eval" => filter.include_eval = v.trim() == "include",
                    "eval_fraction" => filter.eval_fraction = v.parse().map_err(|_| bad())?,
                    _ => return Err(bad()),
                }
            } else {
                return Err(bad());
            }
        }
        Ok(filter)
    }

    fn admits(&self, trace: &RunTrace) -> bool {
        let meta = &trace.meta;
        let evaluated = matches!(meta.source, RunSource::Eval | RunSource::Bench);
        if !meta.consent || (evaluated && !self.include_eval) {
            return false;
        }
        if self.model.as_ref().is_some_and(|m| &meta.model != m) {
            return false;
        }
        if self.source.is_some_and(|s| meta.source != s) {
            return false;
        }
        match self.min_outcome {
            Some(min) => trace.outcome().is_some_and(|o| o >= min),
            None => true,
        }
    }
}

/// Provenance written next to an export.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatasetCard {
    /// Export format.
    pub format: String,
    /// Trajectory schema version of the inputs.
    pub schema_version: u32,
    /// Harness version that exported.
    pub exporter: String,
    /// Filter in effect.
    pub filter: Value,
    /// Runs considered.
    pub runs_seen: usize,
    /// Runs exported.
    pub runs_used: usize,
    /// Records per split.
    pub records: BTreeMap<String, usize>,
    /// Records dropped as duplicates.
    pub duplicates: usize,
    /// Secrets replaced by the redactor.
    pub redactions: usize,
    /// Run ids per split.
    pub runs: BTreeMap<String, Vec<String>>,
}

/// File name of the dataset card.
pub const CARD_FILE: &str = "dataset_card.json";

fn secret_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"sk-(?:ant-|proj-)?[A-Za-z0-9_\-]{16,}",
            r"gh[pousr]_[A-Za-z0-9]{20,}",
            r"github_pat_[A-Za-z0-9_]{20,}",
            r"AKIA[0-9A-Z]{16}",
            r"xox[abprs]-[A-Za-z0-9\-]{10,}",
            r"AIza[0-9A-Za-z_\-]{30,}",
            r"(?i)bearer\s+[A-Za-z0-9._\-]{16,}",
            r"eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
            r#"(?i)(api[_-]?key|secret|password|passwd|token)(["'\s:=]+)[^\s"']{8,}"#,
            r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}",
        ]
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

/// Replace secrets and email addresses with `[REDACTED]`. Returns the
/// redacted text and the number of replacements.
#[must_use]
pub fn redact(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut count = 0;
    for re in secret_patterns() {
        let hits = re.find_iter(&out).count();
        if hits > 0 {
            count += hits;
            out = re.replace_all(&out, "[REDACTED]").into_owned();
        }
    }
    (out, count)
}

pub(crate) fn redact_value(value: &mut Value, count: &mut usize) {
    match value {
        Value::String(s) => {
            let (r, n) = redact(s);
            if n > 0 {
                *s = r;
                *count += n;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| redact_value(v, count)),
        Value::Object(map) => map.values_mut().for_each(|v| redact_value(v, count)),
        _ => {}
    }
}

/// `train` or `eval`, stable per task key.
#[must_use]
pub fn split_for(task_key: &str, eval_fraction: f64) -> &'static str {
    let hash = blake3::hash(task_key.as_bytes());
    let bucket = f64::from(u16::from_le_bytes([hash.as_bytes()[0], hash.as_bytes()[1]]))
        / f64::from(u16::MAX);
    if bucket < eval_fraction {
        "eval"
    } else {
        "train"
    }
}

/// Load every run under `home`.
#[must_use]
pub fn load_all(home: &Path) -> Vec<RunTrace> {
    list_run_dirs(home)
        .into_iter()
        .filter_map(|d| RunTrace::load(&RunBundle::at(d)).ok())
        .collect()
}

/// Export `traces` in `format` to `out_dir` as `<split>.jsonl` plus a
/// dataset card.
///
/// # Errors
///
/// Returns an I/O or JSON error.
pub fn export(
    traces: &[RunTrace],
    blob_home: Option<&Path>,
    format: ExportFormat,
    filter: &ExportFilter,
    out_dir: &Path,
) -> Result<DatasetCard, TraceError> {
    fs::create_dir_all(out_dir)?;
    let blobs = blob_home.and_then(|h| FsSnapshotStore::open(blobs_dir(h)).ok());
    let mut filter = filter.clone();
    if format == ExportFormat::Sft && filter.min_outcome.is_none() {
        filter.min_outcome = Some(f64::MIN_POSITIVE);
    }
    let admitted = admitted_traces(traces, format, &filter);

    let mut card = DatasetCard {
        format: format.as_str().to_string(),
        schema_version: TRACE_SCHEMA_VERSION,
        exporter: crate::bundle::harness_version(),
        filter: serde_json::to_value(&filter)?,
        runs_seen: traces.len(),
        runs_used: 0,
        ..DatasetCard::default()
    };
    let rows = export_rows(format, &admitted, blobs.as_ref());
    let used = write_rows(rows, filter.eval_fraction, out_dir, &mut card)?;
    card.runs = used
        .into_iter()
        .map(|(k, v)| {
            let mut ids: Vec<String> = v.into_iter().collect();
            ids.sort();
            (k, ids)
        })
        .collect();
    card.runs_used = card.runs.values().map(Vec::len).sum();
    crate::bundle::write_json_atomic(&out_dir.join(CARD_FILE), &card)?;
    Ok(card)
}

/// The traces `filter` admits; preference pairs ignore `min_outcome`.
fn admitted_traces<'a>(
    traces: &'a [RunTrace],
    format: ExportFormat,
    filter: &ExportFilter,
) -> Vec<&'a RunTrace> {
    traces
        .iter()
        .filter(|t| {
            if format == ExportFormat::Preference {
                ExportFilter {
                    min_outcome: None,
                    ..filter.clone()
                }
                .admits(t)
            } else {
                filter.admits(t)
            }
        })
        .collect()
}

/// `(task, run, record)` rows to write, before redaction and dedup.
fn export_rows(
    format: ExportFormat,
    admitted: &[&RunTrace],
    blobs: Option<&FsSnapshotStore>,
) -> Vec<(String, String, Value)> {
    let mut rows: Vec<(String, String, Value)> = Vec::new();
    if format == ExportFormat::Preference {
        for (task, record) in preference_pairs(admitted, blobs) {
            rows.push((task.clone(), task, record));
        }
    } else {
        for trace in admitted {
            for record in records_for(format, trace, blobs) {
                rows.push((task_key(trace), trace.meta.run_id.clone(), record));
            }
        }
    }
    rows
}

/// Redact, dedup and write `rows` to `<split>.jsonl` files, counting into
/// `card`. Returns the run ids used per split.
fn write_rows(
    rows: Vec<(String, String, Value)>,
    eval_fraction: f64,
    out_dir: &Path,
    card: &mut DatasetCard,
) -> Result<BTreeMap<String, HashSet<String>>, TraceError> {
    let mut seen: HashSet<blake3::Hash> = HashSet::new();
    let mut writers: BTreeMap<&'static str, fs::File> = BTreeMap::new();
    let mut used: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for (task, run, mut record) in rows {
        redact_value(&mut record, &mut card.redactions);
        let line = serde_json::to_string(&record)?;
        if !seen.insert(blake3::hash(line.as_bytes())) {
            card.duplicates += 1;
            continue;
        }
        let split = split_for(&task, eval_fraction);
        let file = match writers.entry(split) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(fs::File::create(out_dir.join(format!("{split}.jsonl")))?)
            }
        };
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        *card.records.entry(split.to_string()).or_default() += 1;
        used.entry(split.to_string()).or_default().insert(run);
    }
    Ok(used)
}

/// Default output directory for an export.
#[must_use]
pub fn default_out_dir(home: &Path, format: ExportFormat) -> PathBuf {
    home.join("datasets").join(format.as_str())
}

#[cfg(test)]
mod tests;
