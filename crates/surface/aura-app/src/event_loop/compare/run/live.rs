//! The runs under way, as the BENCH tab shows them: each run's place
//! among them from its start to its end, how the prompt it is on was
//! routed, and what its model is doing.

use super::context::{elapsed_ms, Context, RunSpec};
use crate::event_loop::bench;
use crate::event_loop::subagents::{lock, AgentHub};
use aura_agent_trace::{Activity, Doing};
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, CompareArm, ComparePayload, Difficulty, PromptCategory};
use aura_ui_terminal::events::{run_order, BenchDoing, BenchLiveRun};
use aura_ui_terminal::UiCommand;
use std::time::Instant;

/// A run under way.
#[derive(Clone)]
pub(super) struct Live {
    name: String,
    arm: CompareArm,
    task: Option<String>,
    /// Its agent, and when it started working; `None` while it waits for
    /// its task's files
    working: Option<(AgentId, Instant)>,
    /// How its prompt under way was classified and routed; `None` until
    /// it was
    routed: Option<LiveRoute>,
}

/// How the prompt a run is on was classified and where it went.
#[derive(Clone)]
pub(super) struct LiveRoute {
    pub model: String,
    /// `None` for a pinned arm's run its classifier gave no label
    pub category: Option<PromptCategory>,
    pub difficulty: Option<Difficulty>,
}

impl Context {
    /// The prompt the run `name` is on goes to `route`.
    pub(super) fn routed(&self, name: &str, route: LiveRoute) {
        if let Some(live) = lock(&self.live).iter_mut().find(|l| l.name == name) {
            live.routed = Some(route);
        }
    }

    /// The run `name` is no longer under way.
    pub(super) fn ended(&self, name: &str) {
        lock(&self.live).retain(|l| l.name != name);
    }
}

/// A run's place in [`Context::live`], given up when it ends however it
/// ends, unless kept: a run that ends with a result stays listed until
/// [`run_all`](super::schedule::run_all) has put it among the finished
/// runs, so the BENCH tab never shows it in neither.
pub(super) struct LiveSlot<'a> {
    ctx: &'a Context,
    name: String,
    kept: bool,
}

impl<'a> LiveSlot<'a> {
    /// `run`'s place among the runs under way.
    pub(super) fn take(ctx: &'a Context, run: &RunSpec) -> Self {
        lock(&ctx.live).push(Live {
            name: run.name.clone(),
            arm: run.arm,
            task: run.example.map(|task| task.id.clone()),
            working: None,
            routed: None,
        });
        Self {
            ctx,
            name: run.name.clone(),
            kept: false,
        }
    }

    /// The run works as `agent`, since it first started to.
    pub(super) fn working(&self, agent: AgentId) {
        if let Some(live) = lock(&self.ctx.live)
            .iter_mut()
            .find(|l| l.name == self.name)
        {
            let since = live.working.map_or_else(Instant::now, |(_, since)| since);
            live.working = Some((agent, since));
        }
    }

    /// Keep the run listed: it ended with a result.
    pub(super) fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for LiveSlot<'_> {
    fn drop(&mut self) {
        if !self.kept {
            self.ctx.ended(&self.name);
        }
    }
}

/// Show `payload` in the BENCH tab with where the example tasks' files
/// stand and the runs under way.
pub(super) async fn show_live(ctx: &Context, payload: &ComparePayload) {
    let host = ctx.host.agent_id();
    let root = lock(&ctx.hub.registry).root(host);
    let mut view = bench::view(host, root, &ctx.host.name, payload);
    {
        let setup = ctx.setup.borrow();
        for row in &mut view.tasks {
            if let Some(i) = ctx.plan.examples.iter().position(|t| t.id == row.id) {
                row.setup = setup.get(i).cloned().unwrap_or_default();
            }
        }
    }
    let live = lock(&ctx.live).clone();
    view.live = live.into_iter().map(|l| live_run(&ctx.hub, l)).collect();
    view.live
        .sort_by_key(|live| run_order(&live.name, live.arm));
    let _ = ctx
        .commands
        .send(UiCommand::BenchUpsert(Box::new(view)))
        .await;
}

/// How the BENCH tab shows `live`, a run under way, with what its agent
/// has spent so far.
fn live_run(hub: &AgentHub, live: Live) -> BenchLiveRun {
    let Live {
        name,
        arm,
        task,
        working,
        routed,
    } = live;
    let used = working.map_or_else(Vec::new, |(agent, _)| hub.tokens_by_model(agent));
    let category = (routed.as_ref())
        .and_then(|r| r.category)
        .map(|c| c.as_str().to_string());
    let difficulty = (routed.as_ref())
        .and_then(|r| r.difficulty)
        .map(|d| d.as_str().to_string());
    BenchLiveRun {
        name,
        arm: arm.index(),
        task,
        model: routed.map(|r| r.model),
        category,
        difficulty,
        wall_ms: working.map(|(_, since)| elapsed_ms(since)),
        cost_usd: used.iter().map(|row| row.cost_usd).sum(),
        unpriced_calls: used.iter().map(|row| row.unpriced_calls).sum(),
        tokens: bench::tokens(&used),
        agent_id: working.map(|(agent, _)| agent.to_hex()),
        doing: working.and_then(|(agent, _)| doing(hub, agent, now_ms())),
    }
}

/// What the run of `agent` is doing at `now_ms`, with what its subtask is
/// doing while one runs. `None` when it has no open run.
fn doing(hub: &AgentHub, agent: AgentId, now_ms: u64) -> Option<BenchDoing> {
    let (activity, _) = hub.traces.activity(agent)?;
    let mut doing = bench_doing(&activity, now_ms);
    if matches!(&activity.doing, Doing::Tools(tools) if tools.iter().any(|t| t == "task")) {
        let below = lock(&hub.registry).descendants(agent);
        doing.subtask = below
            .into_iter()
            .rev()
            .filter_map(|agent| hub.traces.activity(agent))
            .find(|(_, in_turn)| *in_turn)
            .map(|(sub, _)| Box::new(bench_doing(&sub, now_ms)));
    }
    Some(doing)
}

/// What `activity` says its model is doing at `now_ms`, and since when.
pub(super) fn bench_doing(activity: &Activity, now_ms: u64) -> BenchDoing {
    let (what, call) = match &activity.doing {
        Doing::Waiting => ("waiting for the model".to_string(), activity.calls + 1),
        Doing::Thinking => ("thinking".to_string(), activity.calls + 1),
        Doing::Writing => ("writing".to_string(), activity.calls + 1),
        Doing::Tools(tools) => (format!("running {}", tools.join(", ")), 0),
    };
    BenchDoing {
        what,
        for_ms: now_ms.saturating_sub(activity.since_ms),
        call,
        restarts: activity.restarts,
        subtask: None,
    }
}
