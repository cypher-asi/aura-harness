//! The responsiveness the load harness holds the app to: a time budget
//! at 200 agents (ignored; release builds), and counts that every test
//! run checks: a burst of asks builds the agent lists and saves the
//! inbox once per interval, not once per ask.

use super::*;
use crate::event_loop::inbox as owed;

/// At 200 agents the loop takes an event within this, at p99 (it was
/// 15.6 ms before the pacing, 0.07 ms after).
const PROBE_WAIT_P99: Duration = Duration::from_millis(5);
/// At 200 agents the loop is busy at most this share of the time (68%
/// before the pacing, 15% after).
const LOOP_BUSY: f64 = 0.40;
/// At 200 agents no loop event takes longer than this.
const LONGEST_EVENT: Duration = Duration::from_millis(100);
/// At 200 agents a frame is drawn within this, at p99.
const FRAME_P99: Duration = Duration::from_millis(16);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a time budget: run it in release with --ignored"]
async fn the_app_stays_responsive_with_200_agents() {
    let measured = run(200, Duration::from_secs(10)).await;
    println!("{}", report(&measured));
    let waits = Spread::of(&measured.waits);
    assert!(waits.count > 0, "probes reached the loop");
    assert!(
        waits.p99 <= PROBE_WAIT_P99,
        "probe wait p99 {:?}",
        waits.p99
    );
    let busy: Duration = measured.loop_events.0.values().flatten().sum();
    let share = busy.as_secs_f64() / measured.span.as_secs_f64();
    assert!(
        share <= LOOP_BUSY,
        "the loop was busy {:.0}%",
        share * 100.0
    );
    let longest = (measured.loop_events.spreads().iter())
        .map(|(kind, spread)| (spread.max, *kind))
        .max();
    if let Some((max, kind)) = longest {
        assert!(max <= LONGEST_EVENT, "{kind} took {max:?}");
    }
    let frames = Spread::of(&measured.ui.frames);
    assert!(frames.p99 <= FRAME_P99, "frame p99 {:?}", frames.p99);
}

#[tokio::test]
async fn a_burst_of_list_asks_builds_the_lists_once_per_interval() {
    let (fixture, mut commands) = fixture();
    let hub = &fixture.hub;
    hub.loop_work.lists.set_every(Duration::from_secs(60));
    for _ in 0..100 {
        hub.send_agent_list().await;
    }
    let sent = drain(&mut commands)
        .into_iter()
        .filter(|(_, command)| matches!(command, UiCommand::SetAgents(_)))
        .count();
    assert_eq!(sent, 1, "once now; the rest wait for the interval");
    assert_eq!(hub.loop_work.lists.runs(), 1);
    assert!(
        !hub.loop_work.lists.take_due(),
        "due once the interval is over"
    );

    // What the user does goes at once.
    hub.loop_work.lists.set_urgent(true);
    hub.send_agent_list().await;
    hub.loop_work.lists.set_urgent(false);
    assert_eq!(hub.loop_work.lists.runs(), 2);
}

#[tokio::test]
async fn a_list_asked_for_within_the_interval_goes_from_the_loop_once_it_is_over() {
    let (fixture, mut commands) = fixture();
    fixture
        .hub
        .loop_work
        .lists
        .set_every(Duration::from_millis(30));
    let (events_tx, events_rx) = mpsc::channel::<UiEvent>(8);
    let mut b = borrowed(&fixture);
    let quit = CancellationToken::new();
    let mut inbox = loop_inbox(&fixture, &mut b, events_rx, &quit);
    let mut state = loop_state(&fixture, &mut b);
    fixture.hub.send_agent_list().await;
    fixture.hub.send_agent_list().await;
    drain(&mut commands);
    let hub = Arc::clone(&fixture.hub);
    let stop = async move {
        tokio::time::timeout(Duration::from_secs(5), async {
            while hub.loop_work.lists.runs() < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the loop sends the lists it was asked for");
        quit.cancel();
        drop(events_tx);
    };
    tokio::join!(run::run_loop(&mut state, &mut inbox), stop);
    let sent = drain(&mut commands)
        .into_iter()
        .filter(|(_, command)| matches!(command, UiCommand::SetAgents(_)))
        .count();
    assert_eq!(sent, 1, "the last ask is not lost");
}

#[tokio::test]
async fn the_loop_saves_the_inbox_once_per_interval_however_many_events() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    state.hub.loop_work.saves.set_every(Duration::from_secs(60));
    for _ in 0..100 {
        owed::save_paced(&state).await;
    }
    assert_eq!(
        state.hub.loop_work.saves.runs(),
        1,
        "one save, the rest due"
    );
    assert!(!state.hub.loop_work.saves.take_due());
}
