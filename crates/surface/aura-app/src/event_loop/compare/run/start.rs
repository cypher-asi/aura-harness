//! One run getting going: once its task's files are in place, its copy
//! is made, its agent is pointed at the copy, its session opened, and its
//! prompts run.

use super::context::{Context, Ran, RunSpec};
use super::copy::{prepare_copy, scratch_dir};
use super::live::LiveSlot;
use super::setup::{files_of, fixture};
use super::turn::run_turns;
use crate::agent_registry::RegisteredAgent;
use crate::event_loop::subagents::lock;
use aura_core_types::{AgentId, CompareArm, CompareCopy};
use aura_exec_tools::RunScratch;
use std::path::PathBuf;
use std::sync::Arc;

/// Run `n` of `arm` from its copy to its check, once its task's files are
/// in place, and hand its agent back. `None` when the user stopped the
/// comparison before it started.
pub(super) async fn run_one(ctx: Arc<Context>, arm: CompareArm, n: u32) -> Option<Ran> {
    let run = RunSpec::new(&ctx.plan, arm, n);
    let slot = LiveSlot::take(&ctx, &run);
    let ran = run_in(&ctx, &slot, &run).await;
    if ran.is_some() {
        // [`run_all`](super::schedule::run_all) drops it from the live
        // runs once it is among the finished ones.
        slot.keep();
    }
    ran
}

/// [`run_one`]'s work, as the run holding `slot`.
async fn run_in(ctx: &Context, slot: &LiveSlot<'_>, run: &RunSpec) -> Option<Ran> {
    let not_run = |problem| Some(Ran::not_run(ctx.host.agent_id(), run, problem));
    if let Some(task) = run.example {
        match files_of(ctx, task).await {
            Ok(()) => {}
            Err(Some(problem)) => return not_run(problem),
            Err(None) => return None,
        }
    }
    if ctx.cancel.is_cancelled() {
        return None;
    }
    let agent = ctx.agents.get(&run.name).cloned();
    if let Some(agent) = &agent {
        slot.working(agent.agent_id());
    }
    let (copy, note) = match copy_for(ctx, run).await {
        Ok(prepared) => prepared,
        Err(problem) => return not_run(problem),
    };
    let folder = copy.workdir.clone();
    let agent = match registered(ctx, run, agent, folder.clone()) {
        Ok(agent) => agent,
        Err(e) => return not_run(format!("registering the run: {e:#}")),
    };
    slot.working(agent.agent_id());
    let salt = compare_cache_salt(ctx.host.agent_id(), run.arm);
    ctx.hub.factory.scope_cache(agent.agent_id(), salt);
    match scratch_for(ctx, run).await {
        Ok(scratch) => ctx.hub.factory.give_scratch(agent.agent_id(), scratch),
        Err(problem) => return not_run(problem),
    }
    let session = match ctx.hub.factory.open(&agent, Vec::new(), Some(folder)) {
        Ok(session) => session,
        Err(e) => return not_run(format!("opening the run: {e:#}")),
    };
    let ran = run_turns(ctx, session, run, copy).await;
    Some(Ran { note, ..ran })
}

/// Prompt-cache salt of `arm`'s runs in the comparison on agent
/// `compare`: the first 16 hex chars of a BLAKE3 hash of
/// `"<compare>:<arm>"`. Opaque, since the providers show it to the model;
/// the same for every run of one arm, different between arms and
/// comparisons.
pub(super) fn compare_cache_salt(compare: AgentId, arm: CompareArm) -> String {
    let arm = match arm {
        CompareArm::A => "A",
        CompareArm::B => "B",
    };
    let hash = blake3::hash(format!("{}:{arm}", compare.to_hex()).as_bytes());
    hash.to_hex().as_str().chars().take(16).collect()
}

/// Make the scratch folder of `run`'s commands, beside its copy (see
/// [`scratch_dir`]), off the async runtime, so no two runs share temp
/// files, caches or build output.
async fn scratch_for(ctx: &Context, run: &RunSpec) -> Result<RunScratch, String> {
    let scratch = RunScratch::new(scratch_dir(&ctx.dir, &run.name));
    let made = scratch.clone();
    tokio::task::spawn_blocking(move || made.create())
        .await
        .map_err(|e| format!("preparing the scratch folder: {e}"))?
        .map_err(|e| format!("preparing the scratch folder: {e}"))?;
    Ok(scratch)
}

/// Make the copy `run` works in (see [`prepare_copy`]) off the async
/// runtime: of its example task's files when it works on one, else of the
/// user's workspace.
async fn copy_for(ctx: &Context, run: &RunSpec) -> Result<(CompareCopy, Option<String>), String> {
    let dir = ctx.dir.clone();
    let (workspace, repo) = match run.example {
        Some(task) => (fixture(&ctx.dir, task), None),
        None => (ctx.plan.workspace.clone(), ctx.repo.clone()),
    };
    let (plan_file, start, name) = (
        ctx.plan.plan.clone(),
        Arc::clone(&ctx.plan.start),
        run.name.clone(),
    );
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_copy(
            &dir,
            &workspace,
            repo.as_deref(),
            plan_file.as_deref(),
            &start,
            &name,
        )
    });
    prepared
        .await
        .map_err(|e| format!("preparing the copy: {e}"))?
}

/// `run`'s agent with its folder set to `folder`, the copy it works in:
/// `agent`, registered up front, else one registered now.
fn registered(
    ctx: &Context,
    run: &RunSpec,
    agent: Option<RegisteredAgent>,
    folder: PathBuf,
) -> anyhow::Result<RegisteredAgent> {
    match agent {
        Some(mut agent) => lock(&ctx.hub.registry)
            .set_folder(agent.agent_id(), folder.clone())
            .map(|()| {
                agent.folder = Some(folder);
                agent
            }),
        None => lock(&ctx.hub.registry).create_child(
            &run.name,
            &ctx.arms[run.arm.index()],
            Some(folder),
        ),
    }
}

#[cfg(test)]
mod salt_tests {
    use super::*;

    #[test]
    fn compare_cache_salt_is_opaque_per_arm_and_comparison() {
        let (one, two) = (AgentId::generate(), AgentId::generate());
        let a = compare_cache_salt(one, CompareArm::A);
        assert_eq!(a, compare_cache_salt(one, CompareArm::A));
        assert_ne!(a, compare_cache_salt(one, CompareArm::B));
        assert_ne!(a, compare_cache_salt(two, CompareArm::A));
        for salt in [a, compare_cache_salt(one, CompareArm::B)] {
            assert_eq!(salt.len(), 16);
            assert!(salt
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }
    }
}
