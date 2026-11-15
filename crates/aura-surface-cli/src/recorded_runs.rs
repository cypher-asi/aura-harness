//! Headless recorded runs: prompts driven through the agent loop against
//! a workspace without the terminal UI. Backs `aura runs branch` and
//! `aura eval run`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use aura_agent::{
    AgentLoop, AgentLoopConfig, AgentLoopResult, KernelModelGateway, KernelToolGateway,
};
use aura_agent_gate::{RunBlocksTool, GATE_BLOCK_AGENT};
use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_agent_trace::{
    child_cost_reward, RecordRouter, ReplayProvider, RunMeta, RunRecorder, RunScorecard, RunSource,
    RunTrace,
};
use aura_config::trace::TraceLevel;
use aura_core_types::trace::StepAction;
use aura_core_types::{AgentId, AgentPermissions, SubagentDispatchRequest, SubagentResult};
use aura_model_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_store_snapshot::SnapshotStore;
use aura_tools::{SubagentDispatchHook, ToolConfig, ToolExecutor};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{gate, session_helpers};

/// How long a turn waits for its event forwarder before recording the
/// turn end.
const RECORDER_DRAIN: Duration = Duration::from_secs(2);

/// One headless run.
pub struct HeadlessRun {
    /// `AURA_HOME`; the bundle lands under `runs/`.
    pub home: PathBuf,
    /// Directory the tools operate in.
    pub workspace: PathBuf,
    /// Model provider (live, mock, or a replay prefix).
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Loop configuration; `trace_blobs` is filled in by the runner.
    pub config: AgentLoopConfig,
    /// One user prompt per turn.
    pub prompts: Vec<String>,
    /// Run metadata (source, task, arm, parent run).
    pub meta: RunMeta,
    /// Trace level; `off` is raised to `decisions`.
    pub level: TraceLevel,
    /// Tool configuration.
    pub tool_config: ToolConfig,
}

/// What a headless run produced.
#[derive(Debug)]
pub struct HeadlessOutcome {
    /// Recorded run id.
    pub run_id: String,
    /// Bundle directory.
    pub bundle_dir: PathBuf,
    /// Result of each completed turn.
    pub turns: Vec<AgentLoopResult>,
    /// Loop error that ended the run early.
    pub error: Option<String>,
    /// Scorecard written at the end of the run.
    pub scorecard: Option<RunScorecard>,
}

/// Run every prompt in order, recording one bundle. The kernel's record
/// log is kept in the bundle's `record/` directory.
///
/// # Errors
///
/// Fails when the bundle, record store or kernel can't be created.
/// Loop errors end the run and are returned in
/// [`HeadlessOutcome::error`].
pub async fn run(spec: HeadlessRun) -> anyhow::Result<HeadlessOutcome> {
    let level = match spec.level {
        TraceLevel::Off => TraceLevel::Decisions,
        level => level,
    };
    let mut meta = spec.meta;
    meta.model.clone_from(&spec.config.model);
    meta.workspace = spec.workspace.display().to_string();
    let rec = RunRecorder::start(&spec.home, meta, level)?.context("tracing is off")?;

    let store: Arc<dyn aura_store_db::Store> =
        session_helpers::open_store(&rec.dir().join("record"))?;
    let agent_id = AgentId::generate();
    let records = Arc::new(RecordRouter::new());
    records.attach(agent_id, &rec);
    let snapshot_store = aura_agent_trace::kernel_snapshot_store(&spec.home)?;
    let (_, mut tools) = session_helpers::build_executor_router_with_config(&spec.tool_config);
    let mut executor = ToolExecutor::new(spec.tool_config.clone())
        .with_caller_permissions(AgentPermissions::full_access());
    let mut config = spec.config;
    let gate_runner = gate::frontier_of(&config.model)
        .is_some()
        .then(|| gate::runner(Some(&spec.home), Some(rec.run_id())));
    if let Some(runner) = &gate_runner {
        let workers = HeadlessWorkers {
            store: Arc::clone(&store),
            provider: Arc::clone(&spec.provider),
            tool_config: spec.tool_config.clone(),
            workspace: spec.workspace.clone(),
            snapshot_store: Arc::clone(&snapshot_store),
            config: config.clone(),
            tools: tools.clone(),
            rec: Arc::clone(&rec),
        };
        executor = executor.with_subagent_dispatch_hook(Arc::new(workers));
        executor.register(Box::new(RunBlocksTool::new(Arc::clone(runner))));
        tools.push(RunBlocksTool::definition());
        tools = gate::coordinate(runner, agent_id, &mut config, tools, None);
    }
    let mut router = ExecutorRouter::new();
    router.add_executor(Arc::new(executor));
    let kernel = Arc::new(Kernel::new(
        store,
        Arc::clone(&spec.provider),
        router,
        KernelConfig {
            workspace_base: spec.workspace.clone(),
            use_workspace_base_as_root: true,
            record_observer: Some(records),
            snapshot_store,
            ..KernelConfig::default()
        },
        agent_id,
    )?);
    let model_gateway = KernelModelGateway::new(Arc::clone(&kernel));
    let tool_gateway = KernelToolGateway::new(kernel);

    config.trace_blobs = rec.blob_store();
    let agent_loop = AgentLoop::new(config);

    let mut messages: Vec<Message> = Vec::new();
    let mut turns = Vec::new();
    let mut error = None;
    for prompt in &spec.prompts {
        rec.begin_turn(prompt);
        messages.push(Message::user(prompt.clone()));
        let (tx, mut rx) = mpsc::channel(256);
        let sink = Arc::clone(&rec);
        let forwarder = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                sink.on_event(&event);
            }
        });
        let result = agent_loop
            .run_with_events(
                &model_gateway,
                &tool_gateway,
                messages.clone(),
                tools.clone(),
                Some(tx),
                None,
            )
            .await;
        let _ = tokio::time::timeout(RECORDER_DRAIN, forwarder).await;
        match result {
            Ok(result) => {
                rec.end_turn(&result);
                messages.clone_from(&result.messages);
                let failed = result.llm_error.clone();
                turns.push(result);
                if let Some(e) = failed {
                    error = Some(e);
                    break;
                }
            }
            Err(e) => {
                rec.end_turn(&AgentLoopResult {
                    llm_error: Some(e.to_string()),
                    ..AgentLoopResult::default()
                });
                error = Some(e.to_string());
                break;
            }
        }
    }
    let scorecard = rec.finish();
    Ok(HeadlessOutcome {
        run_id: rec.run_id(),
        bundle_dir: rec.dir().to_path_buf(),
        turns,
        error,
        scorecard,
    })
}

/// Runs gate workers of a headless run as fresh agents on the run's
/// record store, adding what they cost to the run's rewards.
struct HeadlessWorkers {
    store: Arc<dyn aura_store_db::Store>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    tool_config: ToolConfig,
    workspace: PathBuf,
    snapshot_store: Arc<dyn SnapshotStore>,
    config: AgentLoopConfig,
    tools: Vec<ToolDefinition>,
    rec: Arc<RunRecorder>,
}

#[async_trait::async_trait]
impl SubagentDispatchHook for HeadlessWorkers {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String> {
        if request.subagent_type != GATE_BLOCK_AGENT {
            return Ok(SubagentResult::rejected(
                "headless runs only dispatch gate workers",
            ));
        }
        let child = AgentId::generate();
        let mut chain = request.parent_chain.clone();
        chain.push(request.parent_agent_id);
        let executor = ToolExecutor::new(self.tool_config.clone())
            .with_caller_permissions(AgentPermissions::full_access())
            .with_parent_chain(chain);
        let mut router = ExecutorRouter::new();
        router.add_executor(Arc::new(executor));
        let kernel = Kernel::new(
            Arc::clone(&self.store),
            Arc::clone(&self.provider),
            router,
            KernelConfig {
                workspace_base: self.workspace.clone(),
                use_workspace_base_as_root: true,
                snapshot_store: Arc::clone(&self.snapshot_store),
                ..KernelConfig::default()
            },
            child,
        )
        .map_err(|e| format!("could not start a gate worker: {e}"))?;
        let result = gate::run_worker(
            Arc::new(kernel),
            self.config.clone(),
            self.tools.clone(),
            &request,
            CancellationToken::new(),
        )
        .await;
        if let Some(micros) = result.cost_micros {
            #[allow(clippy::cast_precision_loss)]
            let usd = micros as f64 / 1_000_000.0;
            self.rec.add_rewards(&[child_cost_reward(
                usd,
                "gate",
                serde_json::json!({
                    "agent": child.to_string(),
                    "model": request.model_override,
                }),
            )]);
        }
        Ok(result)
    }
}

/// Inputs for re-running `source` from step `from_step`.
#[derive(Debug)]
pub struct BranchPlan {
    /// Prompts of every turn up to the one containing `from_step`.
    pub prompts: Vec<String>,
    /// Metadata for the branch run.
    pub meta: RunMeta,
}

/// Plan a branch: the prompts to replay and the branch's metadata.
///
/// # Errors
///
/// Fails when `from_step` isn't a step of the run.
pub fn plan_branch(source: &RunTrace, from_step: u64) -> anyhow::Result<BranchPlan> {
    let Some(step) = source.step(from_step) else {
        bail!("run {} has no step {from_step}", source.meta.run_id);
    };
    let prompts: Vec<String> = source
        .steps
        .iter()
        .filter(|s| s.turn <= step.turn)
        .filter_map(|s| match &s.action {
            StepAction::UserPrompt { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    if prompts.is_empty() {
        bail!(
            "run {} has no user prompt before step {from_step}",
            source.meta.run_id
        );
    }
    let mut meta = RunMeta::new(source.meta.model.clone(), RunSource::Branch);
    meta.task_id = source
        .meta
        .task_id
        .clone()
        .or_else(|| Some(source.meta.run_id.clone()));
    meta.title.clone_from(&source.meta.title);
    meta.mode.clone_from(&source.meta.mode);
    meta.parent_run = Some(source.meta.run_id.clone());
    meta.branch_step = Some(from_step);
    Ok(BranchPlan { prompts, meta })
}

/// Provider that serves `source`'s model responses recorded before
/// `from_step`, then hands off to `live`, so the decision at
/// `from_step` is made again.
///
/// # Errors
///
/// Fails when the blob store can't be opened.
pub fn replay_provider(
    home: &Path,
    source: &RunTrace,
    from_step: u64,
    live: Arc<dyn ModelProvider + Send + Sync>,
) -> anyhow::Result<Arc<dyn ModelProvider + Send + Sync>> {
    let has_prefix = source
        .steps
        .iter()
        .any(|s| s.seq < from_step && matches!(s.action, StepAction::Model(_)));
    if !has_prefix {
        return Ok(live);
    }
    let replay = ReplayProvider::from_steps(home, &source.steps, from_step - 1, Some(live))?;
    Ok(Arc::new(replay))
}
