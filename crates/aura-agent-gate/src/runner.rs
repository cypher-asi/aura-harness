//! Running blocks: choose a tier, dispatch a worker, verify, climb the
//! ladder, roll back what failed, and log every attempt.
//!
//! Workers are child agents started through the calling agent's
//! [`aura_tools::SubagentDispatchHook`] with subagent type
//! [`GATE_BLOCK_AGENT`]; the surface that owns the kernels runs them on
//! the model, effort and tool subset the request names. Every model
//! call they make is recorded by their own kernels.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use aura_core_types::{
    AgentId, SubagentBudget, SubagentDispatchRequest, SubagentExit, SubagentResult,
};
use aura_tools::ToolContext;
use tracing::{info, warn};

use crate::block::{
    Attempt, BlockKind, BlockResult, BlockSpec, BlockStatus, RunBlocksOutput, VerifyLevel,
};
use crate::ladder::{self, Next};
use crate::ledger::{Ledger, LedgerEntry};
use crate::policy::GatePolicy;
use crate::prompts;
use crate::tier::{frontier_of, Tier, Tiers};
use crate::undo::{Caps, Snapshot};
use crate::verify::{self, run_check, run_checks};

/// Subagent type of gate workers and reviewers.
pub const GATE_BLOCK_AGENT: &str = "gate_block";

/// Tools that only read the workspace.
pub const READ_TOOLS: &[&str] = &[
    "read_file",
    "list_files",
    "find_files",
    "search_code",
    "stat_file",
];

/// Tools that change files.
pub const WRITE_TOOLS: &[&str] = &["write_file", "edit_file", "delete_file"];

/// Longest diff a block result carries back to the coordinator.
const MAX_DIFF_BYTES: usize = 6 * 1024;

/// Tools a worker of `kind` gets.
#[must_use]
pub fn worker_tools(kind: BlockKind) -> Vec<String> {
    let mut tools: Vec<String> = READ_TOOLS.iter().map(ToString::to_string).collect();
    if kind.writes() {
        tools.extend(WRITE_TOOLS.iter().map(ToString::to_string));
    }
    tools.push("run_command".into());
    tools
}

/// Runner settings.
#[derive(Debug, Clone)]
pub struct GateSettings {
    /// Routing policy.
    pub policy: GatePolicy,
    /// `fast=<id>,standard=<id>,frontier=<id>` tier overrides.
    pub tier_overrides: Option<String>,
    /// Ledger file; `None` keeps no ledger.
    pub ledger: Option<PathBuf>,
    /// Recorded run the attempts belong to.
    pub run_id: Option<String>,
    /// Most USD exploration may spend in this process.
    pub explore_budget_usd: f64,
    /// Timeout of each check command.
    pub check_timeout_ms: u64,
    /// Wall-clock budget of each worker.
    pub worker_timeout_ms: u64,
    /// Agent-loop iterations of each worker.
    pub worker_iterations: u32,
    /// Most blocks in one `run_blocks` call.
    pub max_blocks: usize,
}

impl Default for GateSettings {
    fn default() -> Self {
        Self {
            policy: GatePolicy::default(),
            tier_overrides: None,
            ledger: None,
            run_id: None,
            explore_budget_usd: 1.0,
            check_timeout_ms: 600_000,
            worker_timeout_ms: 900_000,
            worker_iterations: 60,
            max_blocks: 16,
        }
    }
}

/// Runs `run_blocks` calls for every agent of one surface.
#[derive(Debug)]
pub struct BlockRunner {
    settings: GateSettings,
    ledger: Option<Ledger>,
    frontiers: Mutex<HashMap<AgentId, String>>,
    explore_spent: Mutex<f64>,
    rng: Mutex<u64>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl BlockRunner {
    /// Runner with `settings`.
    #[must_use]
    pub fn new(settings: GateSettings) -> Self {
        let seed = now_ms() ^ 0x9E37_79B9_7F4A_7C15;
        Self {
            ledger: settings.ledger.clone().map(Ledger::new),
            settings,
            frontiers: Mutex::new(HashMap::new()),
            explore_spent: Mutex::new(0.0),
            rng: Mutex::new(seed.max(1)),
        }
    }

    /// Settings in force.
    #[must_use]
    pub const fn settings(&self) -> &GateSettings {
        &self.settings
    }

    /// Note that `agent` coordinates with `frontier` (a model id or a
    /// gate id) from now on.
    pub fn set_frontier(&self, agent: AgentId, frontier: &str) {
        let frontier = frontier_of(frontier).unwrap_or(frontier).to_string();
        lock(&self.frontiers).insert(agent, frontier);
    }

    /// Tiers for a coordinator on `frontier`.
    ///
    /// # Errors
    /// Fails when the tier overrides don't parse.
    pub fn tiers_for(&self, frontier: &str) -> Result<Tiers, String> {
        let frontier = frontier_of(frontier).unwrap_or(frontier);
        let tiers = Tiers::for_frontier(frontier);
        match &self.settings.tier_overrides {
            Some(spec) => tiers.with_overrides(spec),
            None => Ok(tiers),
        }
    }

    fn roll(&self) -> f64 {
        let mut s = lock(&self.rng);
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        #[allow(clippy::cast_precision_loss)]
        let v = (*s >> 11) as f64 / (1u64 << 53) as f64;
        v
    }

    fn explore_budget_left(&self) -> bool {
        *lock(&self.explore_spent) < self.settings.explore_budget_usd
    }

    /// Run `blocks` in order for the agent calling with `ctx`. Blocks
    /// after the first failure are skipped.
    ///
    /// # Errors
    /// Fails before running anything when the context lacks what
    /// workers need or the input is unusable.
    pub async fn run(
        &self,
        ctx: &ToolContext,
        blocks: &[BlockSpec],
    ) -> Result<RunBlocksOutput, String> {
        if blocks.is_empty() {
            return Err("run_blocks needs at least one block".into());
        }
        if blocks.len() > self.settings.max_blocks {
            return Err(format!(
                "run_blocks takes at most {} blocks per call",
                self.settings.max_blocks
            ));
        }
        let mut titles = BTreeSet::new();
        for spec in blocks {
            spec.validate()?;
            if !titles.insert(spec.title.trim()) {
                return Err(format!("two blocks are titled `{}`", spec.title));
            }
        }
        let agent = ctx
            .caller_agent_id
            .ok_or("run_blocks needs the calling agent on the tool context")?;
        let frontier = lock(&self.frontiers)
            .get(&agent)
            .cloned()
            .or_else(|| ctx.caller_model_id.clone())
            .ok_or("run_blocks: no coordinator model is known for this agent")?;
        let tiers = self.tiers_for(&frontier)?;
        if blocks.iter().any(|b| b.kind != BlockKind::Command) && ctx.subagent_dispatch.is_none() {
            return Err("run_blocks: this surface cannot start block workers".into());
        }

        let mut results = Vec::with_capacity(blocks.len());
        let mut failed = false;
        for spec in blocks {
            if failed {
                results.push(BlockResult::skipped(spec));
                continue;
            }
            let result = self.run_block(ctx, &tiers, spec).await;
            failed = result.status != BlockStatus::Passed;
            info!(
                block = %result.title,
                status = ?result.status,
                tier = ?result.tier,
                attempts = result.attempts.len(),
                cost_usd = result.cost_usd,
                "Gate block finished"
            );
            if let Some(ledger) = &self.ledger {
                let entries = LedgerEntry::for_block(
                    &result,
                    &self.settings.policy.version,
                    self.settings.run_id.as_deref(),
                    now_ms(),
                );
                if let Err(e) = ledger.append(&entries) {
                    warn!(error = %e, path = %ledger.path().display(), "Gate ledger write failed");
                }
            }
            results.push(result);
        }
        Ok(RunBlocksOutput::new(results))
    }

    async fn run_block(&self, ctx: &ToolContext, tiers: &Tiers, spec: &BlockSpec) -> BlockResult {
        if spec.kind == BlockKind::Command {
            return self.run_command_block(ctx, spec).await;
        }
        let root = ctx.sandbox.root().to_path_buf();
        let snapshot = if spec.kind.writes() {
            tokio::task::spawn_blocking(move || Snapshot::take(&root, Caps::default()))
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let undoable = snapshot.is_some() || !spec.kind.writes();
        let choice = self.settings.policy.choose(
            spec,
            tiers,
            self.roll(),
            undoable && self.explore_budget_left(),
        );

        let mut attempts: Vec<Attempt> = Vec::new();
        let mut tier = choice.tier;
        let mut reason = choice.reason;
        let mut propensity = choice.propensity;
        let mut explored = choice.explored;
        let mut previous: Option<(Attempt, bool)> = None;
        loop {
            let n = u32::try_from(attempts.len() + 1).unwrap_or(u32::MAX);
            let attempt = self
                .attempt(
                    ctx,
                    tiers,
                    spec,
                    Pick {
                        n,
                        tier,
                        reason: &reason,
                        explored,
                        propensity,
                    },
                    previous.as_ref().map(|(a, r)| (a, *r)),
                )
                .await;
            if explored {
                *lock(&self.explore_spent) += attempt.cost_usd;
            }
            attempts.push(attempt);
            let last = attempts.last().cloned();
            if spec.draft {
                break;
            }
            match ladder::next(&attempts, tiers, self.settings.policy.max_attempts) {
                Next::Stop => break,
                Next::Fix(t) => {
                    previous = last.map(|a| (a, false));
                    tier = t;
                    reason = "fix".into();
                }
                Next::Escalate(t) => {
                    let restored = self.settings.policy.restart_on_escalate
                        && restore(snapshot.as_ref()).await;
                    previous = last.map(|a| (a, restored));
                    tier = t;
                    reason = "escalate".into();
                }
            }
            propensity = 1.0;
            explored = false;
        }

        let passed = attempts.last().is_some_and(|a| a.passed);
        let restored = !passed && restore(snapshot.as_ref()).await;
        let diff = match snapshot {
            Some(snapshot) if passed => {
                tokio::task::spawn_blocking(move || snapshot.diff(MAX_DIFF_BYTES))
                    .await
                    .unwrap_or_default()
            }
            _ => String::new(),
        };
        let files_changed: Vec<String> = if restored {
            Vec::new()
        } else {
            attempts
                .iter()
                .flat_map(|a| a.files_changed.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        let last = attempts.last();
        BlockResult {
            title: spec.title.clone(),
            kind: spec.kind,
            bucket: spec.bucket(),
            status: if passed {
                BlockStatus::Passed
            } else {
                BlockStatus::Failed
            },
            level: last.map(|a| a.level).unwrap_or_default(),
            required: spec.required_level(),
            tier: last.map(|a| a.tier),
            model: last.map(|a| a.model.clone()),
            summary: last.map(|a| a.summary.clone()).unwrap_or_default(),
            files_changed,
            diff,
            failure: if passed {
                None
            } else {
                last.and_then(acceptance_failure)
                    .or_else(|| last.and_then(|a| a.failure.clone()))
            },
            restored,
            cost_usd: attempts.iter().map(|a| a.cost_usd).sum(),
            attempts,
        }
    }

    async fn run_command_block(&self, ctx: &ToolContext, spec: &BlockSpec) -> BlockResult {
        let command = spec.command.clone().unwrap_or_default();
        let outcome = run_check(ctx, "command", &command, self.settings.check_timeout_ms).await;
        let mut checks = vec![outcome.clone()];
        let mut level = VerifyLevel::Ran;
        let mut failure = (!outcome.passed).then(|| format!("command failed: {command}"));
        if failure.is_none() {
            let v = run_checks(ctx, spec, self.settings.check_timeout_ms).await;
            level = v.level;
            failure = v.failure;
            checks.extend(v.checks);
        }
        let passed = failure.is_none();
        BlockResult {
            title: spec.title.clone(),
            kind: spec.kind,
            bucket: spec.bucket(),
            status: if passed {
                BlockStatus::Passed
            } else {
                BlockStatus::Failed
            },
            level,
            required: spec.required_level(),
            tier: None,
            model: None,
            summary: checks
                .iter()
                .map(|c| format!("$ {}\n{}", c.command, c.output))
                .collect::<Vec<_>>()
                .join("\n"),
            files_changed: Vec::new(),
            diff: String::new(),
            failure,
            restored: false,
            cost_usd: 0.0,
            attempts: Vec::new(),
        }
    }

    async fn attempt(
        &self,
        ctx: &ToolContext,
        tiers: &Tiers,
        spec: &BlockSpec,
        pick: Pick<'_>,
        previous: Option<(&Attempt, bool)>,
    ) -> Attempt {
        let started = Instant::now();
        let model = tiers.model(pick.tier).to_string();
        let effort = prompts::effort(pick.tier, spec.kind);
        let mut attempt = Attempt {
            n: pick.n,
            tier: pick.tier,
            model: model.clone(),
            effort: effort.to_string(),
            reason: pick.reason.to_string(),
            explored: pick.explored,
            propensity: pick.propensity,
            passed: false,
            level: VerifyLevel::Ran,
            failure: None,
            checks: Vec::new(),
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            files_changed: Vec::new(),
            summary: String::new(),
            agent: None,
            wall_ms: 0,
        };
        let worker = self
            .dispatch(
                ctx,
                &model,
                effort,
                prompts::worker_addendum(spec.kind),
                prompts::block_prompt(spec, previous),
                worker_tools(spec.kind),
            )
            .await;
        let result = match worker {
            Ok(result) => result,
            Err(e) => {
                attempt.failure = Some(format!("worker could not start: {e}"));
                attempt.wall_ms = elapsed_ms(started);
                return attempt;
            }
        };
        attempt.cost_usd = cost_usd(&model, &result);
        attempt.input_tokens = result.total_input_tokens;
        attempt.output_tokens = result.total_output_tokens;
        attempt.files_changed.clone_from(&result.files_changed);
        attempt.summary = verify::tail(&result.final_message, 4_000);
        attempt.agent = result.child_agent_id.map(|a| a.to_hex());
        if let Some(reason) = exit_failure(&result.exit) {
            attempt.failure = Some(format!("worker {reason}"));
            attempt.wall_ms = elapsed_ms(started);
            return attempt;
        }
        if spec.kind == BlockKind::Explore && result.final_message.trim().is_empty() {
            attempt.failure = Some("worker reported nothing".into());
            attempt.wall_ms = elapsed_ms(started);
            return attempt;
        }

        let verification = run_checks(ctx, spec, self.settings.check_timeout_ms).await;
        attempt.level = verification.level;
        attempt.checks = verification.checks;
        attempt.failure = verification.failure;
        if attempt.failure.is_none() && spec.review {
            match self.review(ctx, tiers, spec, &attempt).await {
                Ok((cost, verdict)) => {
                    attempt.cost_usd += cost;
                    match verdict {
                        Ok(()) => attempt.level = VerifyLevel::Reviewed,
                        Err(reason) => attempt.failure = Some(reason),
                    }
                }
                Err(e) => attempt.failure = Some(format!("reviewer could not run: {e}")),
            }
        }
        attempt.passed = attempt.failure.is_none() && attempt.level >= spec.required_level();
        if !attempt.passed && attempt.failure.is_none() {
            attempt.failure = Some("did not reach the required verification level".into());
        }
        attempt.wall_ms = elapsed_ms(started);
        attempt
    }

    async fn review(
        &self,
        ctx: &ToolContext,
        tiers: &Tiers,
        spec: &BlockSpec,
        attempt: &Attempt,
    ) -> Result<(f64, Result<(), String>), String> {
        let tier = if attempt.tier <= Tier::Standard {
            Tier::Standard
        } else {
            Tier::Frontier
        };
        let model = tiers.model(tier).to_string();
        let result = self
            .dispatch(
                ctx,
                &model,
                "medium",
                prompts::reviewer_addendum(),
                prompts::review_prompt(spec, attempt),
                READ_TOOLS.iter().map(ToString::to_string).collect(),
            )
            .await?;
        let cost = cost_usd(&model, &result);
        if let Some(reason) = exit_failure(&result.exit) {
            return Ok((cost, Err(format!("reviewer {reason}"))));
        }
        Ok((cost, prompts::parse_review(&result.final_message)))
    }

    async fn dispatch(
        &self,
        ctx: &ToolContext,
        model: &str,
        effort: &str,
        addendum: String,
        prompt: String,
        tools: Vec<String>,
    ) -> Result<SubagentResult, String> {
        let dispatch = ctx
            .subagent_dispatch
            .as_ref()
            .ok_or("no worker dispatcher")?;
        let parent = ctx.caller_agent_id.ok_or("no calling agent")?;
        let permissions = ctx
            .caller_permissions
            .clone()
            .ok_or("no caller permissions")?;
        let mut parent_chain = Vec::with_capacity(ctx.parent_chain.len() + 1);
        parent_chain.push(parent);
        parent_chain.extend(ctx.parent_chain.iter().copied());
        dispatch
            .dispatch(SubagentDispatchRequest {
                parent_agent_id: parent,
                subagent_type: GATE_BLOCK_AGENT.into(),
                prompt,
                originating_user_id: ctx.originating_user_id.clone(),
                parent_chain,
                model_override: Some(model.to_string()),
                reasoning_effort_override: Some(effort.to_string()),
                system_prompt_addendum: Some(addendum),
                parent_permissions: permissions,
                parent_tool_permissions: ctx.caller_tool_permissions.clone(),
                user_tool_defaults: ctx.user_tool_defaults.clone(),
                tool_call_id: None,
                parent_mode: ctx.caller_mode,
                parent_kernel_mode: ctx.caller_kernel_mode,
                parent_model_id: ctx.caller_model_id.clone(),
                override_mode: None,
                override_permissions: None,
                override_tool_subset: Some(tools),
                override_isolation_id: None,
                override_budget: Some(SubagentBudget {
                    max_iterations: self.settings.worker_iterations,
                    max_tokens: None,
                    timeout_ms: self.settings.worker_timeout_ms,
                }),
                spawn_mode: None,
                council_index: None,
                council_parent_tool_use_id: None,
            })
            .await
    }
}

/// The tier choice for one attempt.
struct Pick<'a> {
    n: u32,
    tier: Tier,
    reason: &'a str,
    explored: bool,
    propensity: f64,
}

/// The coordinator's view of a failed acceptance check, which workers
/// never see.
fn acceptance_failure(attempt: &Attempt) -> Option<String> {
    if !ladder::failed_acceptance(attempt) {
        return None;
    }
    let check = attempt.checks.last()?;
    Some(format!(
        "passed the worker's checks but failed your acceptance check `{}` (exit {}); its \
         output is in the attempts. Fix the check if it is wrong, otherwise re-plan the block",
        check.command,
        check
            .exit_code
            .map_or_else(|| "without a status".to_string(), |c| c.to_string()),
    ))
}

/// Why a worker's run doesn't count, `None` when it completed.
fn exit_failure(exit: &SubagentExit) -> Option<String> {
    match exit {
        SubagentExit::Completed => None,
        SubagentExit::Failed { reason } => Some(format!("failed: {reason}")),
        SubagentExit::Cancelled => Some("was cancelled".into()),
        SubagentExit::Timeout => Some("ran out of time".into()),
        SubagentExit::Rejected { reason } => Some(format!("was rejected: {reason}")),
    }
}

/// List price of a worker's run: the dispatcher's figure, else an
/// estimate from its token totals.
#[allow(clippy::cast_precision_loss)]
fn cost_usd(model: &str, result: &SubagentResult) -> f64 {
    result.cost_micros.map_or_else(
        || {
            aura_model_reasoner::model_catalog::estimate_cost_usd(
                model,
                &aura_model_reasoner::Usage::new(
                    result.total_input_tokens,
                    result.total_output_tokens,
                ),
            )
            .unwrap_or_default()
        },
        |micros| micros as f64 / 1_000_000.0,
    )
}

async fn restore(snapshot: Option<&Snapshot>) -> bool {
    let Some(snapshot) = snapshot else {
        return false;
    };
    match snapshot.restore() {
        Ok(_) => true,
        Err(e) => {
            warn!(error = %e, "Gate could not roll back a failed block");
            false
        }
    }
}

#[cfg(test)]
mod tests;
