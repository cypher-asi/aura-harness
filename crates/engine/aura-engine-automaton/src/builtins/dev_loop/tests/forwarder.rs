use super::*;

// ---------------------------------------------------------------
// Post-E.4 drop-policy regression tests
// ---------------------------------------------------------------
//
// Lock in the contract documented in
// `dev_loop/forward_event.rs::ForwardOutcome` and exercised by the
// `spawn_agent_event_forwarder` consolidated drain helper:
//
// 1. `forward_agent_event` reports `Full` / `Closed` outcomes
//    without logging (`forward_agent_event_reports_full` /
//    `_reports_closed`). The pre-fix call site warned per-event;
//    after the fix the function returns the typed outcome and the
//    drain helper decides whether to log.
// 2. `spawn_agent_event_forwarder` keeps consuming the inner
//    channel after the outer receiver drops — the regression that
//    let the agent loop's own `try_send` accumulate backpressure
//    and ultimately fail the tick (`forwarder_drains_inner_after_outer_closed`).
// 3. The forwarder doesn't lose protocol events in an
//    end-to-end burst when the outer channel has slack
//    (`forwarder_forwards_all_events_when_outer_has_capacity`).

#[test]
fn forward_agent_event_reports_sent_when_outer_has_capacity() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AutomatonEvent>(8);
    let outcome = forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::TextDelta("hi".to_string()),
        Some("task-x"),
    );
    assert_eq!(outcome, ForwardOutcome::Sent);
    rx.try_recv().expect("event must be on the outer channel");
}

#[test]
fn forward_agent_event_reports_full_without_panicking() {
    // Capacity-1 outer channel + an already-buffered event guarantees
    // the next `try_send` returns `TrySendError::Full`. Before the
    // fix this branch warned per call; the new contract returns
    // `DroppedFull` and lets the caller debounce.
    let (tx, _rx) = tokio::sync::mpsc::channel::<AutomatonEvent>(1);
    let outcome_first = forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::TextDelta("first".to_string()),
        Some("task-x"),
    );
    assert_eq!(outcome_first, ForwardOutcome::Sent);
    let outcome_second = forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::TextDelta("second".to_string()),
        Some("task-x"),
    );
    assert_eq!(
        outcome_second,
        ForwardOutcome::DroppedFull,
        "second send must observe TrySendError::Full"
    );
}

#[test]
fn forward_agent_event_reports_closed_when_receiver_dropped() {
    // The pre-fix symptom: receiver dropped mid-task and every
    // subsequent per-delta forward warned. Lock in that the
    // function now returns the typed outcome.
    let (tx, rx) = tokio::sync::mpsc::channel::<AutomatonEvent>(8);
    drop(rx);
    let outcome = forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::TextDelta("orphaned".to_string()),
        Some("task-x"),
    );
    assert_eq!(outcome, ForwardOutcome::DroppedClosed);
}

#[test]
fn forward_agent_event_reports_ignored_for_unprojected_variant() {
    // `forward_agent_event`'s `_ => return ForwardOutcome::Ignored`
    // arm covers `AgentLoopEvent` variants we don't currently
    // surface on the WS stream (e.g. `Started`). Without this the
    // forwarder's accounting (full / closed thresholds) would be
    // off by N per task.
    let (tx, _rx) = tokio::sync::mpsc::channel::<AutomatonEvent>(8);
    // `ThinkingComplete` is intentionally not projected onto
    // `AutomatonEvent` (no WS-side consumer needs it), so it
    // exercises the wildcard arm.
    let outcome = forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::ThinkingComplete,
        Some("task-x"),
    );
    assert_eq!(outcome, ForwardOutcome::Ignored);
}

#[tokio::test(flavor = "current_thread")]
async fn forwarder_drains_inner_after_outer_closed() {
    // Headline regression test.
    //
    // Reproduce the post-E.4 flood by dropping the outer receiver
    // mid-stream and then flooding the inner channel with 200
    // advisory events (the same shape the streaming pump produces:
    // per-delta `TextDelta` per `OutputItemDone`).
    //
    // Pre-fix: each event triggered `forward_agent_event`'s
    // per-call `warn!("automaton event channel full or closed: ...")`,
    // and the inner-channel drain task in `tick.rs` had no special
    // handling — the warnings simply piled up while the agent loop
    // continued to call `event_tx.try_send` for the next 6+
    // samplings. That's the 40+ WARN lines per task in the
    // operator report.
    //
    // Post-fix: `spawn_agent_event_forwarder` observes the closed
    // outer once (single `debug!`), then keeps draining the inner
    // channel so the agent loop's `try_send` never sees a full
    // inner queue. This test asserts the inner channel is fully
    // drained (`recv()` returns `None` exactly when all 200
    // senders' messages have been consumed) and the spawned task
    // exits cleanly when the inner sender is dropped.
    let (outer_tx, outer_rx) = tokio::sync::mpsc::channel::<AutomatonEvent>(2);
    let (inner_tx, inner_rx) = tokio::sync::mpsc::channel::<aura_agent::AgentLoopEvent>(64);
    drop(outer_rx);

    let handle = spawn_agent_event_forwarder(outer_tx, inner_rx, Some("task-x".to_string()));

    // Flood the inner channel with the per-delta event shape that
    // E.4's streaming pump produces. 200 chosen to comfortably
    // exceed both the inner-channel capacity (64) and the
    // power-of-two log threshold cadence (1, 2, 4, …, 128) the
    // forwarder uses internally.
    for i in 0..200u32 {
        inner_tx
            .send(aura_agent::AgentLoopEvent::TextDelta(format!("d{i}")))
            .await
            .expect(
                "inner send must succeed: the forwarder keeps draining \
                 even after the outer receiver dropped, so the inner \
                 channel never backs up",
            );
    }
    drop(inner_tx);

    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("forwarder must exit within 5s of the inner sender dropping")
        .expect("forwarder task must not panic");
}

#[tokio::test(flavor = "current_thread")]
async fn forwarder_forwards_all_events_when_outer_has_capacity() {
    // Positive-path regression: with a sized outer channel the
    // forwarder must project every advisory event through
    // `forward_agent_event`. Lock this so a future refactor of
    // `spawn_agent_event_forwarder` (e.g. coalescing `TextDelta`s)
    // doesn't silently drop events on the happy path.
    let (outer_tx, mut outer_rx) = tokio::sync::mpsc::channel::<AutomatonEvent>(512);
    let (inner_tx, inner_rx) = tokio::sync::mpsc::channel::<aura_agent::AgentLoopEvent>(64);

    let handle = spawn_agent_event_forwarder(outer_tx, inner_rx, Some("task-x".to_string()));

    for i in 0..32u32 {
        inner_tx
            .send(aura_agent::AgentLoopEvent::TextDelta(format!("d{i}")))
            .await
            .expect("inner send must succeed");
    }
    drop(inner_tx);

    handle.await.expect("forwarder task must not panic");

    let mut received = 0usize;
    while let Ok(event) = outer_rx.try_recv() {
        match event {
            AutomatonEvent::TextDelta { task_id, .. } => {
                assert_eq!(task_id.as_deref(), Some("task-x"));
                received += 1;
            }
            other => panic!("unexpected projection: {other:?}"),
        }
    }
    assert_eq!(
        received, 32,
        "all 32 per-delta events must reach the outer channel when it has capacity"
    );
}
