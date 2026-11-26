//! Runs in the agent-state database.
//!
//! [`DbSink`] mirrors a [`RunRecorder`](crate::RunRecorder) into an
//! [`aura_store_db::TraceStore`]; [`load`] and [`latest_of`] read a run
//! back as a [`RunTrace`] without its bundle, and [`import`] copies a
//! bundle recorded before the mirror existed.

use std::sync::Arc;

use aura_core_types::trace::{Reward, Step};
use aura_store_db::TraceStore;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::bundle::{RunBundle, RunMeta};
use crate::recorder::TraceSink;
use crate::records::RecordRow;
use crate::scorecard::RunScorecard;
use crate::{RunTrace, TraceError};

/// The agent a run belongs to: its `agent_id`, else the agent named by a
/// terminal run's `tui:<hex>` task id.
#[must_use]
pub fn owner_of(meta: &RunMeta) -> Option<String> {
    meta.agent_id.clone().or_else(|| {
        meta.task_id
            .as_deref()
            .and_then(|id| id.strip_prefix("tui:"))
            .map(str::to_string)
    })
}

/// Mirrors a recorder into a [`TraceStore`].
#[derive(Debug)]
pub struct DbSink {
    store: Arc<TraceStore>,
}

impl DbSink {
    /// Sink writing to `store`.
    #[must_use]
    pub fn new(store: Arc<TraceStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }
}

fn encode_all<T: Serialize>(items: &[T]) -> Vec<Vec<u8>> {
    items
        .iter()
        .filter_map(|item| serde_json::to_vec(item).ok())
        .collect()
}

fn decode_all<T: DeserializeOwned>(values: &[Vec<u8>]) -> Vec<T> {
    values
        .iter()
        .filter_map(|bytes| match serde_json::from_slice(bytes) {
            Ok(item) => Some(item),
            Err(e) => {
                tracing::debug!(error = %e, "skipping unreadable stored trace entry");
                None
            }
        })
        .collect()
}

fn logged<E: std::fmt::Display>(what: &str, result: Result<(), E>) {
    if let Err(e) = result {
        tracing::warn!(error = %e, "{what} not stored in the trace database");
    }
}

impl TraceSink for DbSink {
    fn meta(&self, meta: &RunMeta) {
        let result = serde_json::to_vec(meta)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                self.store
                    .put_meta(&meta.run_id, owner_of(meta).as_deref(), &bytes)
                    .map_err(|e| e.to_string())
            });
        logged("run metadata", result);
    }

    fn steps(&self, run_id: &str, steps: &[Step]) {
        let encoded: Vec<(u64, Vec<u8>)> = steps
            .iter()
            .filter_map(|step| Some((step.seq, serde_json::to_vec(step).ok()?)))
            .collect();
        logged(
            "run steps",
            self.store.put_steps(
                run_id,
                encoded.iter().map(|(seq, bytes)| (*seq, bytes.as_slice())),
            ),
        );
    }

    fn rewards(&self, run_id: &str, rewards: &[Reward]) {
        logged(
            "run rewards",
            self.store.append_rewards(run_id, &encode_all(rewards)),
        );
    }

    fn records(&self, run_id: &str, rows: &[RecordRow]) {
        logged(
            "run record rows",
            self.store.append_records(run_id, &encode_all(rows)),
        );
    }

    fn summary(&self, run_id: &str, card: &RunScorecard) {
        let result = serde_json::to_vec(card)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                self.store
                    .put_summary(run_id, &bytes)
                    .map_err(|e| e.to_string())
            });
        logged("run scorecard", result);
    }
}

/// Load `run_id` from `store`. `None` when it isn't stored.
///
/// # Errors
///
/// Store or unsealing failures, or malformed metadata.
pub fn load(store: &TraceStore, run_id: &str) -> Result<Option<RunTrace>, TraceError> {
    let Some(meta) = store.meta(run_id)? else {
        return Ok(None);
    };
    let meta: RunMeta = serde_json::from_slice(&meta)?;
    let mut steps: Vec<Step> = decode_all(&store.steps(run_id)?);
    let records: Vec<RecordRow> = decode_all(&store.records(run_id)?);
    crate::records::join(&mut steps, &records);
    let rewards: Vec<Reward> = decode_all(&store.rewards(run_id)?);
    let mut trace = RunTrace::from_parts(meta, steps, rewards);
    trace.records = records;
    Ok(Some(trace))
}

/// The newest stored run of the agent with hex id `agent`.
///
/// # Errors
///
/// Store or unsealing failures, or malformed metadata.
pub fn latest_of(store: &TraceStore, agent: &str) -> Result<Option<RunTrace>, TraceError> {
    match store.latest_run_of(agent)? {
        Some(run_id) => load(store, &run_id),
        None => Ok(None),
    }
}

/// The scorecard stored for `run_id`, if the run finished.
///
/// # Errors
///
/// Store or unsealing failures.
pub fn summary(store: &TraceStore, run_id: &str) -> Result<Option<RunScorecard>, TraceError> {
    Ok(store
        .summary(run_id)?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
}

/// A whole run as one machine hands it to another (a cloud agent's
/// machine to the hub): what [`load`] reads, plus the scorecard.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct RunCopy {
    pub meta: RunMeta,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub rewards: Vec<Reward>,
    #[serde(default)]
    pub records: Vec<RecordRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RunScorecard>,
}

impl RunCopy {
    /// Read `bundle` whole.
    ///
    /// # Errors
    ///
    /// The bundle's files can't be read.
    pub fn read(bundle: &RunBundle) -> Result<Self, TraceError> {
        Ok(Self {
            meta: bundle.read_meta()?,
            steps: bundle.read_steps()?,
            rewards: bundle.read_rewards()?,
            records: bundle.read_records()?,
            summary: bundle.read_summary(),
        })
    }
}

/// Make `copy` what `store` holds for its run: an earlier copy of the
/// same run is replaced, not added to.
///
/// # Errors
///
/// The earlier copy can't be removed.
pub fn replace(store: &Arc<TraceStore>, copy: &RunCopy) -> Result<(), TraceError> {
    let run_id = copy.meta.run_id.as_str();
    store.delete_run(run_id)?;
    let sink = DbSink::new(Arc::clone(store));
    sink.meta(&copy.meta);
    if !copy.steps.is_empty() {
        sink.steps(run_id, &copy.steps);
    }
    if !copy.rewards.is_empty() {
        sink.rewards(run_id, &copy.rewards);
    }
    if !copy.records.is_empty() {
        sink.records(run_id, &copy.records);
    }
    if let Some(card) = &copy.summary {
        sink.summary(run_id, card);
    }
    Ok(())
}

/// Copy `bundle` into `store`, returning the run as loaded.
///
/// # Errors
///
/// The bundle can't be read.
pub fn import(store: &Arc<TraceStore>, bundle: &RunBundle) -> Result<RunTrace, TraceError> {
    let trace = RunTrace::load(bundle)?;
    let sink = DbSink::new(Arc::clone(store));
    let run_id = trace.meta.run_id.as_str();
    sink.meta(&trace.meta);
    sink.steps(run_id, &trace.steps);
    if !trace.stored_rewards.is_empty() {
        sink.rewards(run_id, &trace.stored_rewards);
    }
    if !trace.records.is_empty() {
        sink.records(run_id, &trace.records);
    }
    if let Some(card) = bundle.read_summary() {
        sink.summary(run_id, &card);
    }
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::RunSource;
    use crate::RunRecorder;
    use aura_agent::AgentLoopResult;
    use aura_config::TraceLevel;
    use aura_core_types::trace::{RewardKind, RewardTarget};

    fn store(dir: &std::path::Path) -> Arc<TraceStore> {
        let rocks = aura_store_db::RocksStore::open(dir, false).unwrap();
        Arc::new(TraceStore::for_store(&rocks))
    }

    #[test]
    fn recorder_mirrors_into_the_database() {
        let home = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let store = store(db.path());
        let mut meta = RunMeta::new("m", RunSource::Test);
        meta.agent_id = Some("abc".into());
        let rec = RunRecorder::start(home.path(), meta, TraceLevel::Decisions)
            .unwrap()
            .unwrap();
        rec.begin_turn("before the sink");
        rec.add_sink(DbSink::new(Arc::clone(&store)));
        rec.end_turn(&AgentLoopResult::default());
        rec.begin_turn("second");
        rec.add_rewards(&[Reward {
            target: RewardTarget::Run,
            kind: RewardKind::HumanExplicit,
            name: "human".into(),
            value: 1.0,
            source: "test".into(),
            at_ms: 1,
            evidence: serde_json::Value::Null,
        }]);
        rec.finish();

        let loaded = latest_of(&store, "abc").unwrap().unwrap();
        let from_bundle = RunTrace::load(rec.bundle()).unwrap();
        assert_eq!(loaded.meta.title, "before the sink");
        assert!(loaded.meta.ended_at_ms.is_some());
        assert_eq!(loaded.steps.len(), 3);
        assert_eq!(loaded.steps, from_bundle.steps);
        assert_eq!(loaded.stored_rewards.len(), 1);
        assert!(summary(&store, &rec.run_id()).unwrap().is_some());
    }

    #[test]
    fn imported_bundles_are_found_by_their_terminal_agent() {
        let home = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let store = store(db.path());
        let mut meta = RunMeta::new("m", RunSource::Tui);
        meta.task_id = Some("tui:feed".into());
        let rec = RunRecorder::start(home.path(), meta, TraceLevel::Decisions)
            .unwrap()
            .unwrap();
        rec.begin_turn("old run");
        rec.finish();

        import(&store, rec.bundle()).unwrap();
        let loaded = latest_of(&store, "feed").unwrap().unwrap();
        assert_eq!(loaded.meta.run_id, rec.run_id());
        assert_eq!(loaded.steps.len(), 1);
    }

    #[test]
    fn a_copied_run_replaces_its_earlier_copy() {
        let home = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let store = store(db.path());
        let mut meta = RunMeta::new("m", RunSource::Gateway);
        meta.agent_id = Some("cloudy".into());
        let rec = RunRecorder::start(home.path(), meta, TraceLevel::Decisions)
            .unwrap()
            .unwrap();
        rec.begin_turn("first");
        rec.on_record(&RecordRow {
            seq: 1,
            tx_type: "agent_msg".into(),
            at_ms: 1,
            tool_use_id: None,
            tool: None,
            verdict: None,
            approver: None,
            rule: None,
            reason: None,
            call: None,
            route: None,
        });
        rec.end_turn(&AgentLoopResult::default());
        rec.finish();

        let wire = |copy: &RunCopy| -> RunCopy {
            serde_json::from_value(serde_json::to_value(copy).unwrap()).unwrap()
        };
        replace(&store, &wire(&RunCopy::read(rec.bundle()).unwrap())).unwrap();
        rec.begin_turn("second");
        rec.end_turn(&AgentLoopResult::default());
        rec.finish();
        let copy = wire(&RunCopy::read(rec.bundle()).unwrap());
        replace(&store, &copy).unwrap();

        let loaded = latest_of(&store, "cloudy").unwrap().unwrap();
        assert_eq!(loaded.meta.run_id, rec.run_id());
        assert_eq!(loaded.steps.len(), 4);
        assert_eq!(loaded.records.len(), 1, "rows are not added twice");
        assert!(summary(&store, &rec.run_id()).unwrap().is_some());
    }
}
