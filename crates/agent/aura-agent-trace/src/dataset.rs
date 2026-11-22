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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aura_core_types::trace::{Actor, RewardKind, RewardTarget, StepAction, TRACE_SCHEMA_VERSION};
use aura_store_snapshot::{FsSnapshotStore, SnapshotStore};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::bundle::{blobs_dir, list_run_dirs, RunBundle, RunSource};
use crate::normalize::{RunTrace, DEFAULT_GAMMA};
use crate::TraceError;

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

fn task_key(trace: &RunTrace) -> String {
    trace
        .meta
        .task_id
        .clone()
        .or_else(|| trace.meta.parent_run.clone())
        .unwrap_or_else(|| {
            if trace.meta.title.is_empty() {
                trace.meta.run_id.clone()
            } else {
                trace.meta.title.to_lowercase()
            }
        })
}

/// Chat transcript of a run. Tool outputs come from blobs when the run
/// was recorded at `full`, otherwise from the stored excerpt.
fn chat(trace: &RunTrace, blobs: Option<&FsSnapshotStore>) -> Vec<Value> {
    let mut messages = Vec::new();
    for step in &trace.steps {
        match &step.action {
            StepAction::UserPrompt { text } => {
                messages.push(json!({"role": "user", "content": text}));
            }
            StepAction::Model(action) if step.actor == Actor::Model => {
                let tool_calls: Vec<Value> = action
                    .tool_calls
                    .iter()
                    .map(|c| json!({"id": c.tool_use_id, "name": c.name, "arguments": c.input}))
                    .collect();
                let mut msg = json!({"role": "assistant", "content": action.text});
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = Value::Array(tool_calls);
                }
                messages.push(msg);
            }
            StepAction::ToolResult {
                tool_use_id,
                excerpt,
                content,
                is_error,
                ..
            } => {
                let full = content
                    .as_ref()
                    .zip(blobs)
                    .and_then(|(id, store)| store.get(id).ok().flatten())
                    .and_then(|b| String::from_utf8(b.to_vec()).ok());
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": full.unwrap_or_else(|| excerpt.clone()),
                    "is_error": is_error,
                }));
            }
            StepAction::Model(_) | StepAction::Harness { .. } | StepAction::TurnEnd { .. } => {}
        }
    }
    messages
}

fn records_for(
    format: ExportFormat,
    trace: &RunTrace,
    blobs: Option<&FsSnapshotStore>,
) -> Vec<Value> {
    let run_id = &trace.meta.run_id;
    match format {
        ExportFormat::Sft => vec![json!({
            "run_id": run_id,
            "model": trace.meta.model,
            "outcome": trace.outcome(),
            "messages": chat(trace, blobs),
        })],
        ExportFormat::Rl => {
            let returns: HashMap<u64, (f64, f64)> = trace
                .returns(DEFAULT_GAMMA)
                .into_iter()
                .map(|(seq, r, g)| (seq, (r, g)))
                .collect();
            let last = trace.steps.last().map(|s| s.seq);
            trace
                .steps
                .iter()
                .map(|s| {
                    let (reward, ret) = returns.get(&s.seq).copied().unwrap_or_default();
                    json!({
                        "run_id": run_id,
                        "seq": s.seq,
                        "turn": s.turn,
                        "actor": s.actor,
                        "observation": s.observation,
                        "action": s.action,
                        "reward": reward,
                        "return": ret,
                        "done": Some(s.seq) == last,
                        "policy": s.policy,
                        "propensity": s.policy.propensity,
                    })
                })
                .collect()
        }
        ExportFormat::HarnessBandit => {
            let effects: HashMap<u64, f64> = trace
                .derived_rewards
                .iter()
                .filter(|r| r.kind == RewardKind::DecisionEffect)
                .filter_map(|r| match r.target {
                    RewardTarget::Step { seq } => Some((seq, r.value)),
                    RewardTarget::Run => None,
                })
                .collect();
            trace
                .steps
                .iter()
                .filter_map(|s| {
                    let Actor::Harness { source } = s.actor else {
                        return None;
                    };
                    let StepAction::Harness {
                        name,
                        action,
                        alternatives,
                    } = &s.action
                    else {
                        return None;
                    };
                    let (features, trigger) = match &s.observation {
                        aura_core_types::trace::Observation::Harness { features, trigger } => {
                            (features.clone(), trigger.clone())
                        }
                        _ => (Value::Null, Value::Null),
                    };
                    Some(json!({
                        "run_id": run_id,
                        "seq": s.seq,
                        "source": source,
                        "name": name,
                        "version": s.policy.version,
                        "features": features,
                        "trigger": trigger,
                        "action": action,
                        "alternatives": alternatives,
                        "params": s.policy.params,
                        "propensity": s.policy.propensity,
                        "effect": effects.get(&s.seq),
                        "outcome": trace.outcome(),
                    }))
                })
                .collect()
        }
        ExportFormat::Preference => Vec::new(),
    }
}

fn preference_pairs(traces: &[&RunTrace], blobs: Option<&FsSnapshotStore>) -> Vec<(String, Value)> {
    let mut by_task: BTreeMap<String, Vec<&RunTrace>> = BTreeMap::new();
    for t in traces {
        if t.outcome().is_some() {
            by_task.entry(task_key(t)).or_default().push(t);
        }
    }
    let mut out = Vec::new();
    for (task, mut runs) in by_task {
        runs.sort_by(|a, b| {
            b.outcome()
                .partial_cmp(&a.outcome())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (Some(best), Some(worst)) = (runs.first(), runs.last()) else {
            continue;
        };
        if best.outcome() <= worst.outcome() {
            continue;
        }
        let shared_prefix = match (&worst.meta.parent_run, worst.meta.branch_step) {
            (Some(p), Some(step)) if *p == best.meta.run_id => Some(step),
            _ => match (&best.meta.parent_run, best.meta.branch_step) {
                (Some(p), Some(step)) if *p == worst.meta.run_id => Some(step),
                _ => None,
            },
        };
        out.push((
            task.clone(),
            json!({
                "task": task,
                "chosen_run": best.meta.run_id,
                "rejected_run": worst.meta.run_id,
                "chosen_outcome": best.outcome(),
                "rejected_outcome": worst.outcome(),
                "shared_prefix_steps": shared_prefix,
                "chosen": chat(best, blobs),
                "rejected": chat(worst, blobs),
            }),
        ));
    }
    out
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
    let admitted: Vec<&RunTrace> = traces
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
        .collect();

    let mut card = DatasetCard {
        format: format.as_str().to_string(),
        schema_version: TRACE_SCHEMA_VERSION,
        exporter: crate::bundle::harness_version(),
        filter: serde_json::to_value(&filter)?,
        runs_seen: traces.len(),
        runs_used: 0,
        ..DatasetCard::default()
    };
    let mut rows: Vec<(String, String, Value)> = Vec::new();
    if format == ExportFormat::Preference {
        for (task, record) in preference_pairs(&admitted, blobs.as_ref()) {
            rows.push((task.clone(), task, record));
        }
    } else {
        for trace in &admitted {
            for record in records_for(format, trace, blobs.as_ref()) {
                rows.push((task_key(trace), trace.meta.run_id.clone(), record));
            }
        }
    }

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
        let split = split_for(&task, filter.eval_fraction);
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

/// Default output directory for an export.
#[must_use]
pub fn default_out_dir(home: &Path, format: ExportFormat) -> PathBuf {
    home.join("datasets").join(format.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::RunMeta;
    use crate::rewards::tests::{base, model, nudge, result};
    use aura_core_types::trace::Reward;

    fn run(title: &str, outcome: Option<f64>, source: RunSource) -> RunTrace {
        let mut meta = RunMeta::new("m", source);
        meta.title = title.into();
        let steps = vec![
            base(
                1,
                Actor::User,
                StepAction::UserPrompt {
                    text: "fix it, key sk-ant-abcdefghijklmnopqrstu".into(),
                },
            ),
            nudge(2, "implement_now"),
            model(3, &[("a", "edit_file", serde_json::json!({"path": "x"}))]),
            result(4, 3, "a", "edit_file", "x", "modify", false),
        ];
        let rewards = outcome
            .map(|v| Reward {
                target: RewardTarget::Run,
                kind: RewardKind::Outcome,
                name: "verify".into(),
                value: v,
                source: "verify".into(),
                at_ms: 1,
                evidence: Value::Null,
            })
            .into_iter()
            .collect();
        RunTrace::from_parts(meta, steps, rewards)
    }

    #[test]
    fn redacts_common_secrets() {
        let (out, n) = redact("token=abcd1234efgh and ghp_abcdefghijklmnopqrstuvwx me@x.io");
        assert_eq!(n, 3);
        assert!(!out.contains("ghp_"));
        assert!(!out.contains("me@x.io"));
    }

    #[test]
    fn splits_are_stable_per_task() {
        assert_eq!(split_for("task-a", 0.5), split_for("task-a", 0.5));
        assert_eq!(split_for("anything", 0.0), "train");
        assert_eq!(split_for("anything", 1.1), "eval");
    }

    #[test]
    fn sft_keeps_successful_non_eval_runs_and_redacts() {
        let dir = tempfile::tempdir().unwrap();
        let traces = vec![
            run("a", Some(1.0), RunSource::Tui),
            run("b", Some(0.0), RunSource::Tui),
            run("c", Some(1.0), RunSource::Eval),
        ];
        let filter = ExportFilter::parse("eval_fraction=0").unwrap();
        let card = export(&traces, None, ExportFormat::Sft, &filter, dir.path()).unwrap();
        assert_eq!(card.runs_used, 1);
        assert_eq!(card.records.get("train"), Some(&1));
        assert!(card.redactions >= 1);
        let text = fs::read_to_string(dir.path().join("train.jsonl")).unwrap();
        assert!(text.contains("[REDACTED]"));
        assert!(dir.path().join(CARD_FILE).is_file());
    }

    #[test]
    fn preference_pairs_same_task_by_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let traces = vec![
            run("same task", Some(1.0), RunSource::Tui),
            run("same task", Some(0.0), RunSource::Tui),
        ];
        let filter = ExportFilter::parse("eval_fraction=0").unwrap();
        let card = export(&traces, None, ExportFormat::Preference, &filter, dir.path()).unwrap();
        assert_eq!(card.records.get("train"), Some(&1));
    }

    #[test]
    fn bandit_rows_carry_effects() {
        let dir = tempfile::tempdir().unwrap();
        let traces = vec![run("a", None, RunSource::Tui)];
        let filter = ExportFilter::parse("eval_fraction=0").unwrap();
        export(
            &traces,
            None,
            ExportFormat::HarnessBandit,
            &filter,
            dir.path(),
        )
        .unwrap();
        let text = fs::read_to_string(dir.path().join("train.jsonl")).unwrap();
        let row: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(row["name"], "implement_now");
        assert_eq!(row["effect"], 1.0);
    }

    #[test]
    fn filter_parses_and_rejects_unknown_keys() {
        let f = ExportFilter::parse("outcome>=0.5,model=x,source=tui").unwrap();
        assert_eq!(f.min_outcome, Some(0.5));
        assert_eq!(f.source, Some(RunSource::Tui));
        assert!(ExportFilter::parse("nope=1").is_err());
    }
}
