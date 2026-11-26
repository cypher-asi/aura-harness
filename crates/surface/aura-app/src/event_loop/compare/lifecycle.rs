//! A comparison from `/compare go` to its end: its agent beside the one
//! that asked, its task and what is shown when that task fails, and
//! `/compare cancel`.

use super::record::{interrupted, latest_payload};
use super::run::{self, ComparePlan};
use super::view::plural;
use super::Running;
use crate::agent_registry::RegisteredAgent;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::subagents::{lock, AgentHub};
use crate::event_loop::{bench, titles, LoopState};
use aura_ui::UiCommand;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// `/compare go`: start the comparison waiting for it.
pub(crate) async fn start(state: &mut LoopState<'_>) -> Result<String, String> {
    if let Some(running) = state.compare.running() {
        return Err(format!("{} is still running", running.name));
    }
    let plan = state
        .compare
        .pending
        .take()
        .ok_or("Nothing to start: /compare [prompt] prepares a comparison first")?;
    let host = register(state, &plan)?;
    crate::event_loop::channels::join_or_warn(&state.hub, &host);
    state.hub.send_agent_list().await;
    info!(
        compare = %host.name,
        repeats = plan.repeats,
        prompts = plan.prompts.len(),
        examples = plan.examples.len(),
        a = %plan.arms[0].label(),
        b = %plan.arms[1].label(),
        "Starting a comparison"
    );
    let runs = plan.runs_per_arm();
    let text = format!(
        "{} started: {runs} run{} per arm, up to {} at a time; they show in the Agents panel. \
         /compare cancel stops it.",
        host.name,
        plural(runs),
        plan.concurrency,
    );
    let cancel = CancellationToken::new();
    let task = tokio::spawn(supervise(
        Arc::clone(&state.hub),
        state.commands.clone(),
        plan,
        host.clone(),
        cancel.clone(),
    ));
    state.compare.running = Some(Running {
        host: host.agent_id(),
        name: host.name,
        cancel,
        task,
    });
    Ok(text)
}

/// The agent the comparison `plan` runs under, registered beside the
/// agent that asked for it; that agent is named after the benchmark when
/// it has no title yet.
fn register(state: &LoopState<'_>, plan: &ComparePlan) -> Result<RegisteredAgent, String> {
    let mut registry = lock(&state.hub.registry);
    // Asked from a comparison, or from a run of one, it goes beside
    // that comparison rather than inside it.
    let (parent_id, untitled, parent) = registry
        .ancestry(plan.user)
        .into_iter()
        .filter_map(|id| registry.find(id))
        .find(|agent| {
            !agent.is_compare()
                && !agent.is_compare_arm()
                && !registry.under_compare(agent.agent_id())
        })
        .map(|agent| (agent.agent_id(), agent.needs_title(), agent.zns_id.clone()))
        .ok_or("The agent that asked for the comparison is gone")?;
    let host = registry
        .create_compare(&parent)
        .map_err(anyhow::Error::from)
        .map_err(|e| format!("Could not register the comparison: {e:#}"))?;
    // An agent opened only to run a benchmark never runs a typed turn,
    // so it would keep its placeholder name in the Agents panel: name
    // it after the benchmark instead. Names the user gave are kept.
    if untitled {
        if let Err(e) =
            registry.set_title(parent_id, &titles::bench_title(&plan.source, &plan.task))
        {
            warn!(agent = %parent_id, error = %e, "Could not save the benchmark title");
        }
    }
    Ok(host)
}

/// Run the comparison `plan` on `host`; when its task fails, which leaves
/// its record saying it runs, show it stopped.
async fn supervise(
    hub: Arc<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
    plan: ComparePlan,
    host: RegisteredAgent,
    cancel: CancellationToken,
) {
    let user = plan.user;
    let ran = tokio::spawn(run::run(
        Arc::clone(&hub),
        commands.clone(),
        plan,
        host.clone(),
        cancel,
    ))
    .await;
    let Err(e) = ran else { return };
    warn!(compare = %host.name, error = %e, "A comparison's task failed");
    let id = host.agent_id();
    if let Some(mut payload) = latest_payload(hub.factory.store.as_ref(), id) {
        interrupted(&mut payload, "its task failed");
        let root = lock(&hub.registry).root(id);
        let view = bench::view(id, root, &host.name, &payload);
        let _ = commands.send(UiCommand::BenchUpsert(Box::new(view))).await;
    }
    AgentUi::new(commands, user)
        .send(UiCommand::ShowError(format!("{} stopped: {e}", host.name)))
        .await;
}

/// `/compare cancel`: stop the comparison running, else drop the one
/// waiting to start.
pub(crate) fn cancel(state: &mut LoopState<'_>) -> String {
    if let Some(running) = state.compare.running() {
        running.cancel.cancel();
        return format!(
            "Stopping {}; runs under way end where they are",
            running.name
        );
    }
    if state.compare.pending.take().is_some() {
        return "Dropped the comparison".to_string();
    }
    "No comparison to cancel".to_string()
}
