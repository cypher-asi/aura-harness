//! What the runs of one comparison share, which run each is, and what
//! each gives back once it ends.

use super::live::Live;
use super::plan::ComparePlan;
use crate::agent_registry::RegisteredAgent;
use crate::bench_pack::ExampleTask;
use crate::event_loop::subagents::{AgentHub, ArmRouting};
use aura_agent_kernel::Kernel;
use aura_core_types::{AgentId, CompareArm, CompareCopy, CompareRun, OutcomePayload};
use aura_gate::compare::{run_name, PlanReading};
use aura_ui_terminal::events::BenchTaskSetup;
use aura_ui_terminal::UiCommand;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

/// Why a run the user stopped ended.
pub(super) const STOPPED: &str = "stopped by the user";

/// What the runs of one comparison share.
pub(super) struct Context {
    pub hub: Arc<AgentHub>,
    pub commands: mpsc::Sender<UiCommand>,
    pub plan: ComparePlan,
    pub host: RegisteredAgent,
    /// ZNS ids of the agents each arm's runs are registered under, A first
    pub arms: [String; 2],
    /// How the turns and subtasks of each arm's runs are routed, A first
    pub routing: [ArmRouting; 2],
    /// Where this comparison's copies go
    pub dir: PathBuf,
    /// Root of the user's git repository, when the workspace is in one
    pub repo: Option<PathBuf>,
    pub cancel: CancellationToken,
    /// Where each example task's files stand, in `plan.examples` order
    pub setup: watch::Receiver<Vec<BenchTaskSetup>>,
    /// The runs' agents, registered before setting up so the Agents panel
    /// lists them waiting, by run name
    pub agents: HashMap<String, RegisteredAgent>,
    /// Runs under way, in the order they started
    pub live: Mutex<Vec<Live>>,
}

/// Run `n` of `arm`: its name, and the example task it works on when the
/// work is example tasks.
pub(super) struct RunSpec {
    pub arm: CompareArm,
    pub n: u32,
    pub name: String,
    pub example: Option<&'static ExampleTask>,
}

impl RunSpec {
    /// Run `n` of `arm` in the comparison `plan`.
    pub(super) fn new(plan: &ComparePlan, arm: CompareArm, n: u32) -> Self {
        Self {
            arm,
            n,
            name: run_name(arm, n),
            example: plan.example(n),
        }
    }
}

/// A run that ended, with what the judge needs from it.
pub(super) struct Ran {
    pub run: CompareRun,
    /// Its order: run number, then arm
    pub order: (u32, CompareArm),
    pub reply: String,
    pub patch: String,
    pub note: Option<String>,
    /// Plan usage its model calls reported
    pub readings: Vec<PlanReading>,
    /// Its last turn's `Outcome` as its check judged it, recorded on its
    /// kernel once every run has ended, so no run is routed by another's
    /// hidden checks
    pub checked: Option<(Arc<Kernel>, OutcomePayload)>,
}

impl Ran {
    /// `run`, which never got going because of `error`, under the
    /// comparison's agent `host`.
    pub(super) fn not_run(host: AgentId, run: &RunSpec, error: String) -> Self {
        Self {
            run: CompareRun {
                name: run.name.clone(),
                arm: run.arm,
                agent_id: host,
                prompt_tx: None,
                model: String::new(),
                copy: CompareCopy {
                    root: PathBuf::new(),
                    workdir: PathBuf::new(),
                    git_dir: None,
                    baseline: String::new(),
                },
                wall_ms: 0,
                cost_usd: 0.0,
                tokens: Vec::new(),
                plan_usage: Vec::new(),
                turns: Vec::new(),
                check_passed: None,
                files_changed: 0,
                error: Some(error),
                task: run.example.map(|task| task.id.clone()),
            },
            order: (run.n, run.arm),
            reply: String::new(),
            patch: String::new(),
            note: None,
            readings: Vec::new(),
            checked: None,
        }
    }
}

/// Milliseconds since `since`.
pub(super) fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}
