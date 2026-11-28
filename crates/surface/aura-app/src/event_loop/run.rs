//! The event loop proper: what arrives, handled one at a time, each
//! timed ([`super::timing`]), and the inbox saved after each.

use super::*;
use std::time::Instant;

/// The stall sweep's timer: its first tick a sweep from now. A loop busy
/// past a sweep runs one late sweep, not a burst of them: strikes count
/// minutes, and a burst would count several at once.
fn sweep_timer() -> tokio::time::Interval {
    let first_sweep = tokio::time::Instant::now() + stalls::SWEEP_EVERY;
    let mut sweeps = tokio::time::interval_at(first_sweep, stalls::SWEEP_EVERY);
    sweeps.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    sweeps
}

/// One thing that reached the loop.
enum Arrived {
    Sweep,
    Done(Box<WorkDone>),
    CloudEnded(cloud::Ended),
    SpawnFinished,
    Completion(Transaction),
    Notice(notices::Notice),
    Mail(mail::MailItem),
    Link(auth::LinkOutcome),
    Ui(UiEvent),
    /// The agent lists asked for within their interval are due
    ListsDue,
    /// The loop's paced inbox save is due
    SaveDue,
    /// Time for startup's next step: nothing else waited
    Startup,
}

impl Arrived {
    /// What the timings call it.
    const fn kind(&self) -> &'static str {
        match self {
            Self::Sweep => "stall sweep",
            Self::Done(_) => "work done",
            Self::CloudEnded(_) => "cloud turn ended",
            Self::SpawnFinished => "spawn finished",
            Self::Completion(_) => "command completed",
            Self::Notice(notice) if notice.inbox.is_some() => "inbox message",
            Self::Notice(_) => "notice",
            Self::Mail(_) => "mail view",
            Self::Link(_) => "sign-in link",
            Self::Ui(_) => "ui event",
            Self::ListsDue => "agent lists",
            Self::SaveDue => "inbox save",
            Self::Startup => "startup step",
        }
    }
}

/// Handle what arrives until the UI quits or goes away, running
/// `startup`'s steps while no UI event waits.
pub(super) async fn run_loop(
    state: &mut LoopState<'_>,
    inbox: &mut Inbox,
    startup: &mut startup::Startup,
) {
    let hub = Arc::clone(&state.hub);
    let mut cloud_ended = state.cloud.take_ended();
    let mut sweeps = sweep_timer();
    loop {
        let arrived = tokio::select! {
            _ = sweeps.tick() => Arrived::Sweep,
            Some(done) = inbox.done_rx.recv() => Arrived::Done(Box::new(done)),
            Some(ended) = cloud_ended.recv() => Arrived::CloudEnded(ended),
            () = hub.spawn_finished.notified() => Arrived::SpawnFinished,
            Some(tx) = inbox.process_completions.recv() => Arrived::Completion(tx),
            Some(notice) = inbox.notice_rx.recv() => Arrived::Notice(notice),
            Some(item) = inbox.mail_rx.recv() => Arrived::Mail(item),
            Some(outcome) = inbox.link_rx.recv() => Arrived::Link(outcome),
            () = hub.loop_work.lists.due() => Arrived::ListsDue,
            () = hub.loop_work.saves.due() => Arrived::SaveDue,
            () = inbox.quit.cancelled() => break,
            event = state.events.recv() => match event {
                Some(event) => Arrived::Ui(event),
                None => break,
            },
            // Only when no UI event waits: the user goes first.
            () = std::future::ready(()), if startup.may_run(state) => Arrived::Startup,
        };
        if matches!(arrived, Arrived::Startup) {
            let started = Instant::now();
            startup.run_next(state).await;
            hub.loop_work
                .timings
                .record("startup step", started.elapsed());
            continue;
        }
        let kind = arrived.kind();
        let saves = !matches!(arrived, Arrived::SaveDue);
        let started = Instant::now();
        let quit = handle(state, inbox, arrived).await;
        hub.loop_work.timings.record(kind, started.elapsed());
        if quit {
            break;
        }
        // A due save saved already; saving after it again would only
        // make the next one due.
        if saves {
            let started = Instant::now();
            inbox::save_paced(state).await;
            hub.loop_work
                .timings
                .record("inbox save", started.elapsed());
        }
    }
}

/// Handle `arrived`. Returns whether the loop is to stop.
async fn handle(state: &mut LoopState<'_>, inbox: &Inbox, arrived: Arrived) -> bool {
    match arrived {
        Arrived::Sweep => stalls::sweep(state).await,
        Arrived::Done(done) => handle_work_done(state, *done).await,
        Arrived::CloudEnded(ended) => cloud::handle_ended(state, ended).await,
        Arrived::SpawnFinished => {
            state.park_finished_spawns().await;
            agents::send_agent_list(state).await;
        }
        Arrived::Completion(tx) => handle_completion(state, tx).await,
        Arrived::Notice(notice) => notices::handle_notice(state, notice).await,
        Arrived::Mail(item) => {
            if let Some(sender) = item.title_from {
                state
                    .hub
                    .title_thread(sender, item.view.thread_id.clone(), item.view.text.clone());
            }
            mail::forward(state.commands, item.view).await;
        }
        Arrived::Link(outcome) => auth::handle_link_outcome(state, outcome).await,
        Arrived::ListsDue => {
            if state.hub.loop_work.lists.take_due() {
                state.hub.send_agent_list_now().await;
            }
        }
        Arrived::SaveDue => inbox::save_due(state),
        // Run by the loop itself, with its own `startup`.
        Arrived::Startup => {}
        Arrived::Ui(event) => {
            state.hub.loop_work.timings.arrived(&event);
            // The user waits to see what they did: lists go at once.
            let hub = Arc::clone(&state.hub);
            hub.loop_work.lists.set_urgent(true);
            let quit = handle_ui_event(state, event, &inbox.link_tx).await;
            hub.loop_work.lists.set_urgent(false);
            return quit;
        }
    }
    false
}
