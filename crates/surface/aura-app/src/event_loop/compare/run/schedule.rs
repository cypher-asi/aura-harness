//! The runs started as the comparison's schedule allows: as many at once
//! as may work, until the budget is spent, each recorded and shown as it
//! ends while the BENCH tab hears how the others are doing.

use super::context::{Context, Ran};
use super::host::record;
use super::live::show_live;
use super::start::run_one;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::compare::view::{cost, duration, money, plural};
use crate::event_loop::handlers::system_message;
use aura_core_types::{ComparePayload, CompareRun};
use aura_gate::compare::{Schedule, Spent};
use std::fmt::Write;
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
    // Arms whose run the chat was told the budget cannot count.
    let mut warned = [false; 2];
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
            schedule.finished(0.0, 0);
            continue;
        };
        schedule.finished(done.run.cost_usd, done.run.unpriced_calls());
        ended(ctx, session, user_ui, payload, &done, &mut warned).await;
        ran.push(done);
    }
    payload.notes.extend(closing_note(ctx, &mut schedule));
    ran
}

/// The budget's note once every run has ended, dropping the runs
/// `schedule` still holds.
fn closing_note(ctx: &Context, schedule: &mut Schedule) -> Option<String> {
    let skipped = schedule.drop_pending().len();
    budget_note(
        ctx.plan.budget_usd,
        schedule.spent(),
        skipped,
        ctx.cancel.is_cancelled(),
    )
}

/// Tell the user's chat that `done` ended (and, the first time a run of
/// its arm had no priced call, that the budget cannot count that arm,
/// marking it in `warned`), put it among the finished runs of `payload`,
/// and record and show that on the comparison's `session`.
async fn ended(
    ctx: &Context,
    session: &AgentSession,
    user_ui: &AgentUi,
    payload: &mut ComparePayload,
    done: &Ran,
    warned: &mut [bool; 2],
) {
    user_ui
        .send(system_message(progress(&ctx.host.name, &done.run)))
        .await;
    let arm = done.run.arm.index();
    let unpriced = (!warned[arm])
        .then(|| unpriced_arm_note(&ctx.host.name, ctx.plan.budget_usd, &done.run))
        .flatten();
    if let Some(note) = unpriced {
        warned[arm] = true;
        user_ui.send(system_message(note)).await;
    }
    payload.runs.push(done.run.clone());
    // Among the finished runs now, it leaves the live ones; the view
    // sent shows both, so no run drops out of the table between them.
    ctx.ended(&done.run.name);
    record(&session.kernel, ctx.host.agent_id(), payload).await;
    show_live(ctx, payload).await;
}

/// What the comparison's record notes about its budget: when it was spent
/// before `skipped` runs could start, what was spent of it; and when some
/// calls had no known price, that the budget could not count them.
/// Nothing without a budget, when the user stopped the comparison, or
/// when every run started and every call had a price.
pub(super) fn budget_note(
    budget_usd: Option<f64>,
    spent: Spent,
    skipped: usize,
    cancelled: bool,
) -> Option<String> {
    let budget = budget_usd.filter(|_| !cancelled)?;
    let spent_of_budget = spent_of_budget(budget, spent);
    if skipped > 0 {
        return Some(format!(
            "{skipped} run{} did not start: {spent_of_budget}",
            plural(skipped)
        ));
    }
    (!spent.is_complete())
        .then(|| format!("The budget counts only priced calls: {spent_of_budget}"))
}

/// `spent $1.20 of the $2.00 budget`, and when some calls had no known
/// price, `spent $1.20+? of the $2.00 budget; 14 calls have no known
/// price and are not counted`.
pub(super) fn spent_of_budget(budget_usd: f64, spent: Spent) -> String {
    let mut text = format!(
        "spent {} of the {} budget",
        cost(spent.usd, spent.unpriced_calls),
        money(budget_usd)
    );
    let calls = spent.unpriced_calls;
    if calls > 0 {
        let (have, are) = if calls == 1 {
            ("has", "is")
        } else {
            ("have", "are")
        };
        let _ = write!(
            text,
            "; {calls} call{} {have} no known price and {are} not counted",
            plural(calls)
        );
    }
    text
}

/// The line the user's chat gets, once per arm, when a run of it made
/// calls and none of them had a known price, so the budget cannot count
/// that arm at all. `None` without a budget, or when some call was
/// priced.
pub(super) fn unpriced_arm_note(
    compare: &str,
    budget_usd: Option<f64>,
    run: &CompareRun,
) -> Option<String> {
    let budget = budget_usd?;
    let calls: u32 = run.tokens.iter().map(|row| row.calls).sum();
    let unpriced = run.unpriced_calls();
    (unpriced > 0 && unpriced >= calls).then(|| {
        format!(
            "{compare}: none of {}'s {unpriced} call{} had a known price, so the {} budget \
             does not count arm {}",
            run.name,
            plural(unpriced),
            money(budget),
            run.arm.as_str().to_ascii_uppercase(),
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
            cost(run.cost_usd, run.unpriced_calls())
        ),
    }
}
