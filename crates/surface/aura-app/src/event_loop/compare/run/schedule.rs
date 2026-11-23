//! The runs started as the comparison's schedule allows: as many at once
//! as may work, until the budget is spent, each recorded and shown as it
//! ends while the BENCH tab hears how the others are doing.

use super::context::{Context, Ran};
use super::host::record;
use super::live::show_live;
use super::start::run_one;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::compare::view::{duration, money, plural};
use crate::event_loop::handlers::system_message;
use aura_core_types::{ComparePayload, CompareRun};
use aura_gate::compare::Schedule;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;
use tracing::warn;

/// How often the BENCH tab hears how the runs under way are doing.
const LIVE_EVERY: Duration = Duration::from_secs(2);

/// Start the runs as the schedule allows and wait for them all,
/// recording each as it ends.
pub(super) async fn run_all(
    ctx: &Arc<Context>,
    session: &AgentSession,
    user_ui: &AgentUi,
    payload: &mut ComparePayload,
) -> Vec<Ran> {
    let plan = &ctx.plan;
    let mut schedule = Schedule::new(plan.runs_per_arm(), plan.concurrency, plan.budget_usd);
    let mut tasks = JoinSet::new();
    let mut ran = Vec::new();
    let mut tick = tokio::time::interval(LIVE_EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut setup = ctx.setup.clone();
    let mut setting_up = !plan.examples.is_empty();
    loop {
        if !ctx.cancel.is_cancelled() {
            while let Some((arm, n)) = schedule.start_next() {
                tasks.spawn(run_one(Arc::clone(ctx), arm, n));
            }
        }
        let joined = tokio::select! {
            joined = tasks.join_next() => joined,
            _ = tick.tick() => {
                show_live(ctx, payload).await;
                continue;
            }
            changed = setup.changed(), if setting_up => {
                setting_up = changed.is_ok();
                show_live(ctx, payload).await;
                continue;
            }
        };
        let Some(joined) = joined else {
            break;
        };
        let done = joined.unwrap_or_else(|_| {
            warn!("A compare run panicked");
            None
        });
        let Some(done) = done else {
            schedule.finished(0.0);
            continue;
        };
        schedule.finished(done.run.cost_usd);
        ended(ctx, session, user_ui, payload, &done).await;
        ran.push(done);
    }
    payload.notes.extend(budget_note(ctx, &mut schedule));
    ran
}

/// Tell the user's chat that `done` ended, put it among the finished
/// runs of `payload`, and record and show that on the comparison's
/// `session`.
async fn ended(
    ctx: &Context,
    session: &AgentSession,
    user_ui: &AgentUi,
    payload: &mut ComparePayload,
    done: &Ran,
) {
    user_ui
        .send(system_message(progress(&ctx.host.name, &done.run)))
        .await;
    payload.runs.push(done.run.clone());
    // Among the finished runs now, it leaves the live ones; the view
    // sent shows both, so no run drops out of the table between them.
    ctx.ended(&done.run.name);
    record(&session.kernel, ctx.host.agent_id(), payload).await;
    show_live(ctx, payload).await;
}

/// What the comparison's record notes when the budget was spent before
/// the runs `schedule` still holds could start; nothing when every run
/// started or the user stopped the comparison.
fn budget_note(ctx: &Context, schedule: &mut Schedule) -> Option<String> {
    let skipped = schedule.drop_pending().len();
    (skipped > 0 && !ctx.cancel.is_cancelled()).then(|| {
        format!(
            "The budget of {} was spent after {}; {skipped} run{} did not start",
            money(ctx.plan.budget_usd.unwrap_or_default()),
            money(schedule.spent_usd()),
            plural(skipped),
        )
    })
}

/// The line the user's chat gets when `run` ends.
pub(super) fn progress(compare: &str, run: &CompareRun) -> String {
    let check = match run.check_passed {
        Some(true) => ", check passed",
        Some(false) => ", check failed",
        None => "",
    };
    match &run.error {
        Some(error) => format!("{compare}: {} stopped: {error}", run.name),
        None => format!(
            "{compare}: {} done in {}, {}{check}",
            run.name,
            duration(run.wall_ms),
            money(run.cost_usd)
        ),
    }
}
