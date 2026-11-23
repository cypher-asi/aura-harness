//! A comparison as the user confirmed it: its work, its arms, and what
//! its runs are routed among.

use crate::bench_pack::ExampleTask;
use crate::event_loop::bench_source::StartFile;
use aura_core_types::{AgentId, ArmSpec, BenchSource};
use aura_gate::route::Candidate;
use std::path::PathBuf;
use std::sync::Arc;

/// A comparison as the user confirmed it.
#[derive(Debug, Clone)]
pub(crate) struct ComparePlan {
    /// What it is about: its prompt, or a replayed session's first one
    pub task: String,
    /// The prompts each run sends in order (the build prompt for a plan);
    /// empty when the work is example tasks
    pub prompts: Vec<String>,
    /// Example tasks: run `n` of either arm works on task
    /// `(n - 1) % examples.len()`, in a copy of its own files, with its
    /// prompts and its check
    pub examples: Vec<&'static ExampleTask>,
    /// Plan file each run builds, relative to the workspace
    pub plan: Option<String>,
    /// What arm A and arm B run on
    pub arms: [ArmSpec; 2],
    /// Where the work comes from
    pub source: BenchSource,
    /// Files put in each copy before its run: a replayed session's files
    /// as they were when it started
    pub start: Arc<Vec<StartFile>>,
    /// What setting it up found out, for the record
    pub notes: Vec<String>,
    /// Model that judges
    pub judge: String,
    pub repeats: u32,
    pub check: Option<String>,
    pub budget_usd: Option<f64>,
    /// What Gate routes a Gate arm among
    pub candidates: Vec<Candidate>,
    /// Every model the user can run on any route, which a by-difficulty
    /// arm's models are looked up among
    pub runnable: Vec<Candidate>,
    /// Model that classifies the runs' prompts: the one the benchmark
    /// named, else Gate's automatic choice
    pub classifier: String,
    /// The user's folder the copies are made from
    pub workspace: PathBuf,
    /// The user's agent that asked
    pub user: AgentId,
    /// Where comparisons keep their copies (`AURA_HOME/compare`)
    pub root: PathBuf,
    /// Runs that may work at once
    pub concurrency: usize,
    /// Whether Gate arms explore, to learn
    pub learn: bool,
}

impl ComparePlan {
    /// Runs each arm gets: `repeats` of each example task, else `repeats`.
    pub(crate) fn runs_per_arm(&self) -> u32 {
        let tasks = u32::try_from(self.examples.len().max(1)).unwrap_or(u32::MAX);
        self.repeats.saturating_mul(tasks)
    }

    /// The example task run `n` works on, if the work is example tasks.
    pub(super) fn example(&self, n: u32) -> Option<&'static ExampleTask> {
        let i = usize::try_from(n.max(1) - 1).ok()?;
        (!self.examples.is_empty()).then(|| self.examples[i % self.examples.len()])
    }
}
