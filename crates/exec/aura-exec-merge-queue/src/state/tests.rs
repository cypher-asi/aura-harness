use super::*;
use crate::entry::{Origin, QueuedPush};

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + seconds, 0).unwrap_or_default()
}

fn request(branch: &str, head: &str) -> MergeRequest {
    MergeRequest {
        push: QueuedPush {
            branch: branch.to_string(),
            head: head.to_string(),
            remote: Some("origin".to_string()),
        },
        origin: Origin::default(),
    }
}

fn merged(sha: &str) -> RunOutcome {
    RunOutcome::Merged {
        merged: sha.to_string(),
        note: "ok".to_string(),
        pr: None,
    }
}

#[test]
fn entries_run_one_at_a_time_in_the_order_they_were_queued() {
    let mut state = QueueState::default();
    for (n, branch) in [(0, "a"), (1, "b"), (2, "c")] {
        state.enqueue(request(branch, "h"), at(n));
    }
    let first = state.claim_next(at(10)).unwrap();
    assert_eq!(first.branch, "a");
    assert_eq!(state.claim_next(at(11)), None, "one runs at a time");
    state.finish(&first.id, merged("m1"), at(12));
    let second = state.claim_next(at(13)).unwrap();
    assert_eq!(second.branch, "b");
    state.finish(&second.id, RunOutcome::Failed("x".into()), at(14));
    assert_eq!(state.claim_next(at(15)).unwrap().branch, "c");
}

#[test]
fn an_entry_moves_from_queued_to_running_to_merged() {
    let mut state = QueueState::default();
    let id = state.enqueue(request("a", "h1"), at(0)).id().clone();
    assert_eq!(state.entries[0].status, MergeStatus::Queued);
    let running = state.claim_next(at(5)).unwrap();
    assert_eq!(running.status, MergeStatus::Running);
    assert_eq!(running.started_at, Some(at(5)));
    assert_eq!(running.attempts, 1);
    let done = state.finish(&id, merged("m"), at(9)).unwrap();
    assert_eq!(done.status, MergeStatus::Merged);
    assert_eq!(done.merged.as_deref(), Some("m"));
    assert_eq!(done.finished_at, Some(at(9)));
    assert_eq!(
        state.finish(&id, RunOutcome::Failed("late".into()), at(10)),
        None,
        "a finished entry never changes"
    );
}

#[test]
fn a_failed_entry_keeps_its_reason() {
    let mut state = QueueState::default();
    let id = state.enqueue(request("a", "h"), at(0)).id().clone();
    state.claim_next(at(1));
    let done = state
        .finish(&id, RunOutcome::Failed("tests failed".into()), at(2))
        .unwrap();
    assert_eq!(done.status, MergeStatus::Failed);
    assert_eq!(done.detail.as_deref(), Some("tests failed"));
}

#[test]
fn a_new_push_of_a_queued_branch_keeps_its_place() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h1"), at(0));
    state.enqueue(request("b", "h1"), at(1));
    let again = state.enqueue(request("a", "h2"), at(2));
    assert!(matches!(again, Enqueued::Updated(_)));
    assert_eq!(state.entries.len(), 2);
    let next = state.claim_next(at(3)).unwrap();
    assert_eq!((next.branch.as_str(), next.head.as_str()), ("a", "h2"));
    assert_eq!(next.enqueued_at, at(0));
}

#[test]
fn the_same_push_again_is_a_duplicate_while_running_or_merged() {
    let mut state = QueueState::default();
    let id = state.enqueue(request("a", "h"), at(0)).id().clone();
    state.claim_next(at(1));
    assert!(matches!(
        state.enqueue(request("a", "h"), at(2)),
        Enqueued::Duplicate(_)
    ));
    state.finish(&id, merged("m"), at(3));
    assert!(matches!(
        state.enqueue(request("a", "h"), at(4)),
        Enqueued::Duplicate(_)
    ));
    assert!(matches!(
        state.enqueue(request("a", "h2"), at(5)),
        Enqueued::Added(_)
    ));
}

#[test]
fn a_failed_push_can_be_queued_again() {
    let mut state = QueueState::default();
    let id = state.enqueue(request("a", "h"), at(0)).id().clone();
    state.claim_next(at(1));
    state.finish(&id, RunOutcome::Failed("x".into()), at(2));
    assert!(matches!(
        state.enqueue(request("a", "h"), at(3)),
        Enqueued::Added(_)
    ));
}

#[test]
fn recovery_requeues_an_interrupted_entry_in_its_place() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h"), at(0));
    state.enqueue(request("b", "h"), at(1));
    state.claim_next(at(2));
    let failed = state.recover(at(3), 3);
    assert!(failed.is_empty());
    assert_eq!(state.entries[0].status, MergeStatus::Queued);
    let again = state.claim_next(at(4)).unwrap();
    assert_eq!(again.branch, "a", "an interrupted entry keeps its place");
    assert_eq!(again.attempts, 2);
}

#[test]
fn recovery_fails_an_entry_interrupted_too_often() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h"), at(0));
    for n in 0..3 {
        state.claim_next(at(n));
        state.recover(at(n), 3);
    }
    assert_eq!(state.entries[0].status, MergeStatus::Failed);
    assert!(state.entries[0]
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("interrupted 3 times"));
}

#[test]
fn entries_waiting_too_long_expire() {
    let mut state = QueueState::default();
    state.enqueue(request("old", "h"), at(0));
    state.enqueue(request("new", "h"), at(7_000));
    let expired = state.expire(at(7_300), Duration::hours(2));
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].branch, "old");
    assert_eq!(state.claim_next(at(7_301)).unwrap().branch, "new");
}

#[test]
fn the_same_push_of_a_queued_branch_is_a_duplicate() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h1"), at(0));
    assert!(matches!(
        state.enqueue(request("a", "h1"), at(1)),
        Enqueued::Duplicate(_)
    ));
    assert_eq!(state.entries[0].pushed_at, None);
}

#[test]
fn a_new_push_restarts_the_wait() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h1"), at(0));
    state.enqueue(request("a", "h2"), at(7_000));
    assert!(state.expire(at(7_300), Duration::hours(2)).is_empty());
    assert_eq!(state.expire(at(14_300), Duration::hours(2)).len(), 1);
}

#[test]
fn archiving_takes_the_oldest_finished_entries_only() {
    let mut state = QueueState::default();
    for n in 0..4 {
        let id = state
            .enqueue(request(&format!("b{n}"), "h"), at(n))
            .id()
            .clone();
        state.claim_next(at(n));
        state.finish(&id, merged("m"), at(n));
    }
    state.enqueue(request("waiting", "h"), at(9));
    let taken = state.take_archivable(2);
    let names = |entries: &[MergeEntry]| -> Vec<String> {
        entries.iter().map(|e| e.branch.clone()).collect()
    };
    assert_eq!(names(&taken), ["b0", "b1"]);
    assert_eq!(names(&state.entries), ["b2", "b3", "waiting"]);
    assert!(state.take_archivable(2).is_empty());
}

#[test]
fn state_round_trips_through_json() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h"), at(0));
    state.claim_next(at(1));
    let json = serde_json::to_string(&state).unwrap();
    assert_eq!(serde_json::from_str::<QueueState>(&json).unwrap(), state);
}

#[test]
fn an_entry_put_back_after_a_crash_waits_from_then() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h"), at(0));
    state.claim_next(at(1));
    // The hub was down for two days; recovery puts the entry back.
    let back = at(0) + Duration::hours(48);
    state.recover(back, 3);
    let ttl = Duration::hours(24);
    assert!(state.expire(back + Duration::hours(1), ttl).is_empty());
    let expired = state.expire(back + Duration::hours(25), ttl);
    assert_eq!(expired.len(), 1);
}

#[test]
fn a_state_file_from_before_history_loads() {
    let json = r#"{"version":1,"next_seq":2,"entries":[{"id":"q1","branch":"a","head":"h",
        "status":"queued","enqueued_at":"2026-11-27T10:00:00Z"}]}"#;
    let state: QueueState = serde_json::from_str(json).unwrap();
    assert_eq!(state.history, HistoryMark::default());
    assert_eq!(state.entries[0].updated_at, None);
    assert_eq!(
        state.entries[0].waiting_since(),
        state.entries[0].enqueued_at
    );
}

fn hold(retry_at: i64, target_tip: Option<&str>) -> Hold {
    Hold {
        reason: "main is broken".to_string(),
        kind: if target_tip.is_some() {
            HoldKind::TargetBroken
        } else {
            HoldKind::ForgeUnavailable
        },
        since: at(0),
        retry_at: at(retry_at),
        target_tip: target_tip.map(str::to_string),
        tests: vec!["cli_golden_top_level_help".to_string()],
        excerpt: None,
        suspects: Vec::new(),
    }
}

#[test]
fn a_held_entry_goes_back_in_its_place_uncharged_and_nothing_starts_until_the_retry() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h1"), at(0));
    state.enqueue(request("b", "h2"), at(1));
    let first = state.claim_next(at(2)).unwrap();
    let back = state
        .finish(&first.id, RunOutcome::Held(hold(100, None)), at(3))
        .unwrap();
    assert_eq!(back.status, MergeStatus::Queued);
    assert_eq!(back.attempts, 0);
    assert!(back.finished_at.is_none());
    assert_eq!(state.claim_next(at(50)), None, "held");
    let again = state.claim_next(at(100)).unwrap();
    assert_eq!(again.branch, "a", "it kept its place");
    assert!(state.hold.is_none());
}

#[test]
fn a_hold_on_the_target_ends_when_the_target_moves() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h1"), at(0));
    let first = state.claim_next(at(1)).unwrap();
    state.finish(&first.id, RunOutcome::Held(hold(10_000, Some("t1"))), at(2));
    assert!(!state.release_hold("t1", at(3)), "same tip: still held");
    assert_eq!(state.claim_next(at(3)), None, "parked");
    assert!(state.broken.is_some());
    assert!(state.release_hold("t2", at(3)));
    assert!(state.broken.is_none());
    assert_eq!(state.claim_next(at(4)).unwrap().branch, "a");
}

#[test]
fn a_held_queue_round_trips_through_json() {
    let state = QueueState {
        hold: Some(hold(5, None)),
        broken: Some(hold(5, Some("t1"))),
        ..QueueState::default()
    };
    let text = serde_json::to_string(&state).unwrap_or_default();
    let back: QueueState = serde_json::from_str(&text).unwrap_or_default();
    assert_eq!(back, state);
    let old: QueueState =
        serde_json::from_str(r#"{"version":1,"next_seq":1,"entries":[]}"#).unwrap_or_default();
    assert!(old.hold.is_none(), "files from before holds load");
}

#[test]
fn a_broken_target_parks_only_its_entry_so_the_fix_still_lands() {
    let mut state = QueueState::default();
    state.enqueue(request("a", "h1"), at(0));
    state.enqueue(request("fix", "h2"), at(1));
    let first = state.claim_next(at(2)).unwrap();
    state.finish(&first.id, RunOutcome::Held(hold(10_000, Some("t1"))), at(3));
    let next = state.claim_next(at(4)).unwrap();
    assert_eq!(next.branch, "fix", "the entry behind it runs");
    state.finish(&next.id, merged("t2"), at(5));
    assert!(state.waits_on_target());
    assert!(state.release_hold("t2", at(6)), "the fix moved the target");
    assert_eq!(state.claim_next(at(7)).unwrap().branch, "a", "tried again");
    assert!(!state.waits_on_target());
}
