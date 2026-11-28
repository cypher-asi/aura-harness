use super::*;

const MIN: u64 = 60_000;

fn agent(n: u8) -> AgentId {
    AgentId::new([n; 32])
}

#[test]
fn a_sign_in_pause_waits_until_cleared_then_resumes_every_agent() {
    let recovery = Recovery::default();
    recovery.pause_for_sign_in(agent(1), "401 revoked", 0);
    recovery.pause_for_sign_in(agent(2), "401 revoked", 0);
    assert!(recovery.take_due(10 * MIN).is_empty(), "not cleared yet");
    assert_eq!(
        recovery.sign_in_wait(agent(1)).as_deref(),
        Some("401 revoked")
    );

    recovery.worked(agent(3));
    let mut resumed: Vec<AgentId> = recovery
        .take_due(10 * MIN)
        .into_iter()
        .map(|(a, _)| a)
        .collect();
    resumed.sort_by_key(|a| a.to_hex());
    assert_eq!(resumed, [agent(1), agent(2)]);
    assert!(!recovery.is_held(agent(1)));
}

#[test]
fn a_pause_straight_after_a_resume_waits_a_growing_cooldown() {
    let recovery = Recovery::default();
    let a = agent(1);
    recovery.pause_for_sign_in(a, "401", 0);
    recovery.signed_in();
    assert_eq!(
        recovery.take_due(0).len(),
        1,
        "the first pause has no cooldown"
    );

    // Refused again at once: 2 minutes.
    recovery.pause_for_sign_in(a, "401", MIN);
    recovery.signed_in();
    assert!(recovery.take_due(2 * MIN).is_empty());
    assert_eq!(recovery.take_due(3 * MIN).len(), 1);

    // And again: 4 minutes, then up to 30.
    recovery.pause_for_sign_in(a, "401", 10 * MIN);
    recovery.signed_in();
    assert!(recovery.take_due(13 * MIN).is_empty());
    assert_eq!(recovery.take_due(14 * MIN).len(), 1);
    for _ in 0..10 {
        recovery.pause_for_sign_in(a, "401", 0);
        recovery.signed_in();
        recovery.take_due(u64::MAX);
    }
    recovery.pause_for_sign_in(a, "401", 0);
    let Some(Hold::SignIn { cooldown, .. }) = recovery.release(a) else {
        panic!("held for sign-in");
    };
    assert_eq!(cooldown, 30 * MIN, "capped");

    // Its own model call worked: the next pause starts afresh.
    recovery.worked(a);
    recovery.pause_for_sign_in(a, "401", 0);
    let Some(Hold::SignIn { cooldown, .. }) = recovery.release(a) else {
        panic!("held for sign-in");
    };
    assert_eq!(cooldown, 0);
}

#[test]
fn turn_retries_back_off_and_stop_at_the_cap() {
    let recovery = Recovery::default();
    let a = agent(1);
    let mut waits = Vec::new();
    for _ in 0..MAX_TURN_RETRIES {
        let (_, wait) = recovery.hold_turn(a, "408", 0).expect("under the cap");
        waits.push(wait);
        assert!(recovery.take_due(wait - 1).is_empty(), "not due yet");
        assert_eq!(recovery.take_due(wait).len(), 1);
    }
    assert_eq!(waits, [MIN, 2 * MIN, 4 * MIN]);
    assert_eq!(recovery.hold_turn(a, "408", 0), None, "the fourth fails");
    assert!(!recovery.is_held(a));
    assert_eq!(
        recovery.hold_turn(a, "408", 0),
        None,
        "and the next, until one works"
    );
    // A turn that worked starts the count over.
    recovery.worked(a);
    assert_eq!(recovery.hold_turn(a, "408", 0).map(|(n, _)| n), Some(1));
}

#[test]
fn start_retries_wait_for_a_fix_when_the_project_failed() {
    let recovery = Recovery::default();
    let (machine, project) = (agent(1), agent(2));
    assert_eq!(
        recovery.hold_start(machine, "killed", false, 0),
        Some((1, MIN))
    );
    assert_eq!(
        recovery.hold_start(project, "does not build", true, 0),
        Some((1, BLOCKED_RETRY_MS))
    );
    for _ in 1..MAX_START_RETRIES {
        recovery.hold_start(machine, "killed", false, 0).unwrap();
    }
    assert_eq!(recovery.hold_start(machine, "killed", false, 0), None);
    for _ in 1..MAX_BLOCKED_START_RETRIES {
        recovery
            .hold_start(project, "does not build", true, 0)
            .unwrap();
    }
    assert_eq!(
        recovery.hold_start(project, "does not build", true, 0),
        None
    );
    recovery.started(machine);
    assert_eq!(
        recovery
            .hold_start(machine, "killed", false, 0)
            .map(|(n, _)| n),
        Some(1)
    );
}
