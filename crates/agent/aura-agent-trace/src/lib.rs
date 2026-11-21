//! Layer: agent
//!
//! Normalized run trajectories.
//!
//! Every coding run is recorded as an ordered list of
//! [`aura_core_types::trace::Step`]s plus a reward log, in a bundle
//! under `AURA_HOME/runs/<run_id>/`. The same data feeds:
//!
//! - [`RunScorecard`]: efficiency, waste, harness-behavior and outcome
//!   metrics for one run
//! - [`RunGraph`]: the linked run → turn → iteration → tool-call graph
//!   rendered by the terminal UI
//! - [`RunLog`]: `log.json`, the run and the runs it spawned as one
//!   document to read or hand to a model
//! - [`dataset`]: SFT, preference, RL and harness-bandit exports
//! - [`replay`]: re-running a recorded prefix against a different model
//!   or harness parameters
//!
//! The recorder is a sidecar: it never appends to the kernel record
//! log, it joins to it by tool-use id and file path.

pub mod bundle;
pub mod dataset;
pub mod db;
pub mod graph;
pub mod health;
pub mod normalize;
pub mod recorder;
pub mod records;
pub mod replay;
pub mod report;
pub mod rewards;
pub mod run_log;
pub mod scorecard;

pub use bundle::{kernel_snapshot_store, list_runs, RunBundle, RunMeta, RunSource};
pub use db::DbSink;
pub use graph::{EdgeKind, GraphEdge, GraphNode, NodeKind, RunGraph};
pub use health::RunHealth;
pub use normalize::RunTrace;
pub use recorder::{RunRecorder, StepBuilder, TraceSink};
pub use records::{RecordRouter, RecordRow};
pub use replay::ReplayProvider;
pub use run_log::RunLog;
pub use scorecard::RunScorecard;

/// Errors reading or writing run bundles.
#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    /// Filesystem failure.
    #[error("trace io: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed JSON.
    #[error("trace json: {0}")]
    Json(#[from] serde_json::Error),
    /// Blob store failure.
    #[error("trace blobs: {0}")]
    Blobs(#[from] aura_store_snapshot::SnapshotError),
    /// No run matches the id.
    #[error("no run matches `{0}`")]
    NotFound(String),
    /// More than one run matches the id prefix.
    #[error("`{0}` matches {1} runs; use more characters")]
    Ambiguous(String, usize),
    /// Bad input to an export or replay.
    #[error("{0}")]
    Invalid(String),
    /// Trace database failure.
    #[error("trace database: {0}")]
    Store(#[from] aura_store_db::TraceStoreError),
}
