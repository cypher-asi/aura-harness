//! A cloud turn or move ending: only the loop frees its agent, and a
//! turn's end carries how it went.

use super::*;

#[test]
fn an_ended_cloud_turn_frees_its_agent_through_the_loop() {
    let cloud = CloudState::default();
    let agent = AgentId::generate();
    cloud
        .shared
        .running
        .lock()
        .insert(agent, CancellationToken::new());
    cloud.shared.end_turn(agent, None);
    assert!(!cloud.is_running(agent), "no loop listens: freed at once");

    let mut ended = cloud.take_ended();
    cloud
        .shared
        .running
        .lock()
        .insert(agent, CancellationToken::new());
    let outcome = TurnOutcome {
        failure: Some("boom".into()),
        messages: None,
        said: None,
    };
    cloud.shared.end_turn(agent, Some(outcome));
    assert!(
        cloud.is_running(agent),
        "what is sent until the loop frees it still queues"
    );
    let turn = ended.try_recv().unwrap();
    assert!(
        matches!(turn, Ended::Turn(a, Some(_)) if a == agent),
        "{turn:?}"
    );
    let (freed, outcome) = cloud.free(turn);
    assert_eq!(freed, agent);
    assert_eq!(outcome.and_then(|o| o.failure).as_deref(), Some("boom"));
    assert!(!cloud.is_running(agent));
}

#[test]
fn an_ended_move_lets_its_agent_take_work_only_through_the_loop() {
    let cloud = CloudState::default();
    let agent = AgentId::generate();
    cloud.shared.moving.lock().insert(agent);
    cloud.shared.end_move(agent);
    assert!(!cloud.is_moving(agent), "no loop listens: let go at once");

    let mut ended = cloud.take_ended();
    cloud.shared.moving.lock().insert(agent);
    cloud
        .shared
        .running
        .lock()
        .insert(agent, CancellationToken::new());
    cloud.shared.end_move(agent);
    assert!(
        cloud.is_moving(agent),
        "work still queues until the loop hears"
    );
    let moved = ended.try_recv().unwrap();
    assert!(matches!(moved, Ended::Move(a) if a == agent), "{moved:?}");
    let (freed, outcome) = cloud.free(moved);
    assert_eq!(freed, agent);
    assert!(outcome.is_none(), "a move settles no turn");
    assert!(!cloud.is_moving(agent));
    assert!(
        cloud.is_running(agent),
        "a move's end leaves a turn the loop started alone"
    );
}
