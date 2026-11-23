use super::*;

fn drain(schedule: &mut Schedule) -> Vec<(CompareArm, u32)> {
    std::iter::from_fn(|| schedule.start_next()).collect()
}

#[test]
fn runs_alternate_between_arms_up_to_the_cap() {
    let mut schedule = Schedule::new(3, 4, None);
    assert_eq!(
        drain(&mut schedule),
        [
            (CompareArm::A, 1),
            (CompareArm::B, 1),
            (CompareArm::A, 2),
            (CompareArm::B, 2),
        ]
    );
    assert_eq!(schedule.running, 4);
    schedule.finished(0.1, 0);
    assert_eq!(drain(&mut schedule), [(CompareArm::A, 3)]);
    schedule.finished(0.1, 0);
    assert_eq!(drain(&mut schedule), [(CompareArm::B, 3)]);
    assert!(drain(&mut schedule).is_empty());
    assert!(schedule.drop_pending().is_empty());
    assert_eq!(run_name(CompareArm::B, 3), "b-3");
}

#[test]
fn the_budget_stops_new_runs_once_spent() {
    let mut schedule = Schedule::new(3, 1, Some(0.25));
    assert_eq!(drain(&mut schedule), [(CompareArm::A, 1)]);
    schedule.finished(0.10, 0);
    assert_eq!(drain(&mut schedule), [(CompareArm::B, 1)]);
    schedule.finished(0.20, 0);
    assert!(schedule.over_budget());
    assert!(
        drain(&mut schedule).is_empty(),
        "no run starts past the budget"
    );
    assert_eq!(schedule.running, 0);
    assert_eq!(schedule.drop_pending().len(), 4);
    let spent = schedule.spent();
    assert!((spent.usd - 0.30).abs() < 1e-9);
    assert!(spent.is_complete());

    let mut unlimited = Schedule::new(1, 0, None);
    assert_eq!(drain(&mut unlimited).len(), 1, "a cap of 0 still runs one");
}

#[test]
fn unpriced_calls_are_counted_but_never_reach_the_budget() {
    let mut schedule = Schedule::new(2, 1, Some(0.25));
    assert_eq!(drain(&mut schedule), [(CompareArm::A, 1)]);
    schedule.finished(0.0, 14);
    assert_eq!(
        drain(&mut schedule),
        [(CompareArm::B, 1)],
        "calls with no known price spend nothing of the budget"
    );
    schedule.finished(0.0, 6);
    assert!(!schedule.over_budget());
    assert_eq!(
        schedule.spent(),
        Spent {
            usd: 0.0,
            unpriced_calls: 20
        }
    );
    assert!(!schedule.spent().is_complete());
}

#[test]
fn the_budget_still_stops_on_priced_spend_when_some_calls_are_unpriced() {
    let mut schedule = Schedule::new(3, 1, Some(0.25));
    assert_eq!(drain(&mut schedule), [(CompareArm::A, 1)]);
    schedule.finished(0.30, 3);
    assert!(schedule.over_budget());
    assert!(drain(&mut schedule).is_empty());
    assert_eq!(schedule.drop_pending().len(), 5);
    let spent = schedule.spent();
    assert!((spent.usd - 0.30).abs() < 1e-9);
    assert_eq!(spent.unpriced_calls, 3);
}
