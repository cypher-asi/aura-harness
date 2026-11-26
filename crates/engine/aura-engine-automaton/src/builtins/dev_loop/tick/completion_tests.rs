use super::*;

fn test_context() -> (TickContext, tokio::sync::mpsc::Receiver<AutomatonEvent>) {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let ctx = TickContext::new(
        crate::types::AutomatonId::from_string("test-dev-loop"),
        crate::state::AutomatonState::new(),
        tx,
        serde_json::json!({}),
        None,
        tokio_util::sync::CancellationToken::new(),
    );
    (ctx, rx)
}

#[test]
fn terminal_task_failure_finishes_loop_as_failed() {
    let (mut ctx, mut rx) = test_context();
    let mut state = DevLoopState {
        completed_count: 2,
        failed_count: 1,
        ..DevLoopState::default()
    };

    let outcome =
        DevLoopAutomaton::finish_with_outcome(&mut ctx, &mut state, LoopFinishOutcome::Failed)
            .expect("failed finish should emit LoopFinished");

    assert!(matches!(outcome, TickOutcome::Done));
    let stored = DevLoopState::load(&ctx);
    assert!(
        stored.loop_finished,
        "failed finish must suppress on_stop's secondary LoopFinished event"
    );
    match rx.try_recv().expect("LoopFinished event expected") {
        AutomatonEvent::LoopFinished {
            outcome,
            completed_count,
            failed_count,
        } => {
            assert_eq!(outcome, "failed");
            assert_eq!(completed_count, 2);
            assert_eq!(failed_count, 1);
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn state_serializes_as_single_blob() {
    // Regression for the magic-string-key bag: writing the typed
    // `DevLoopState` once must round-trip every field through
    // `AutomatonState::get/set` (one JSON blob, one key).
    let mut state = AutomatonState::new();
    let saved = DevLoopState {
        initialized: true,
        task_queue: vec!["a".into(), "b".into()],
        completed_count: 3,
        failed_count: 1,
        loop_finished: false,
    };
    state.set(DEV_LOOP_STATE_KEY, &saved).expect("serialize");
    let loaded: DevLoopState = state.get(DEV_LOOP_STATE_KEY).expect("deserialize");
    assert!(loaded.initialized);
    assert_eq!(loaded.task_queue, vec!["a", "b"]);
    assert_eq!(loaded.completed_count, 3);
    assert_eq!(loaded.failed_count, 1);
    assert!(!loaded.loop_finished);
    // Exactly one key written — the one we wrote.
    let keys: Vec<&String> = state.keys().collect();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0], DEV_LOOP_STATE_KEY);
}

use crate::state::AutomatonState;
