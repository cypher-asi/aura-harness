use super::*;

fn reading(ms: u64, plan: Plan, window: &str, used: f64, reset: Option<u64>) -> PlanReading {
    PlanReading {
        recorded_ms: ms,
        usage: PlanUsage {
            plan,
            window: window.to_string(),
            used,
            resets_at: reset,
        },
    }
}

fn used(shares: &[PlanShare], window: &str) -> f64 {
    shares
        .iter()
        .find(|s| s.window == window)
        .map_or(f64::NAN, |s| s.used)
}

fn share(plan: Plan, window: &str, (first, last, used): (f64, f64, f64)) -> PlanShare {
    PlanShare {
        plan,
        window: window.to_string(),
        first,
        last,
        used,
    }
}

#[test]
fn a_run_alone_on_a_plan_gets_each_rise_after_the_first_reading() {
    let calls = [
        reading(1, Plan::Claude, "5h", 0.10, Some(5_000)),
        reading(1, Plan::Claude, "7d", 0.50, Some(9_000)),
        reading(2, Plan::Claude, "5h", 0.12, Some(5_000)),
        reading(3, Plan::Claude, "5h", 0.15, Some(5_000)),
        reading(3, Plan::Claude, "7d", 0.51, Some(9_000)),
    ];
    let shares = plan_shares(&[&calls]);
    assert_eq!(shares.len(), 1);
    let five = &shares[0][0];
    assert_eq!(
        (five.window.as_str(), five.first, five.last),
        ("5h", 0.10, 0.15)
    );
    assert!((five.used - 0.05).abs() < 1e-9);
    assert!((used(&shares[0], "7d") - 0.01).abs() < 1e-9);
    assert!(plan_shares(&[&[]])[0].is_empty(), "no readings, no shares");
}

#[test]
fn runs_at_the_same_time_split_the_rises_instead_of_each_counting_them() {
    // gate-1 and other-1 work at once on one Claude plan, which goes
    // from 10% to 20% while they run.
    let gate = [
        reading(10, Plan::Claude, "5h", 0.10, Some(5_000)),
        reading(30, Plan::Claude, "5h", 0.14, Some(5_000)),
        reading(50, Plan::Claude, "5h", 0.20, Some(5_000)),
    ];
    let other = [
        reading(20, Plan::Claude, "5h", 0.12, Some(5_000)),
        reading(40, Plan::Claude, "5h", 0.13, Some(5_000)),
        reading(45, Plan::Claude, "5h", 0.17, Some(5_000)),
    ];
    let shares = plan_shares(&[&gate, &other]);
    let (g, o) = (used(&shares[0], "5h"), used(&shares[1], "5h"));
    assert!(
        (g - 0.05).abs() < 1e-9,
        "gate saw 0.14-0.12 and 0.20-0.17: {g}"
    );
    assert!(
        (o - 0.05).abs() < 1e-9,
        "other saw 0.12-0.10 and 0.17-0.14: {o}"
    );
    assert!(
        (g + o - 0.10).abs() < 1e-9,
        "the shares add up to how far the window moved"
    );
    let naive = (0.20 - 0.10) + (0.17 - 0.12);
    assert!(
        g + o < naive,
        "first-to-last per run would count overlap twice"
    );
    assert_eq!((shares[1][0].first, shares[1][0].last), (0.12, 0.17));
}

#[test]
fn late_readings_add_nothing_and_a_restarted_window_counts_from_zero() {
    let calls = [
        reading(1, Plan::Chatgpt, "5h", 0.30, Some(10_000)),
        reading(2, Plan::Chatgpt, "5h", 0.35, Some(10_030)),
        reading(3, Plan::Chatgpt, "5h", 0.33, Some(10_010)),
        reading(4, Plan::Chatgpt, "5h", 0.36, None),
        reading(5, Plan::Chatgpt, "5h", 0.02, Some(28_000)),
        reading(6, Plan::Chatgpt, "5h", 0.04, Some(28_000)),
    ];
    let shares = plan_shares(&[&calls]);
    let share = &shares[0][0];
    assert!(
        (share.used - (0.05 + 0.01 + 0.02 + 0.02)).abs() < 1e-9,
        "{share:?}"
    );
    assert_eq!(share.last, 0.04);
}

#[test]
fn plans_and_windows_are_kept_apart_and_merged_per_arm() {
    let one = [
        reading(1, Plan::Claude, "5h", 0.10, None),
        reading(2, Plan::Claude, "5h", 0.20, None),
    ];
    let two = [
        reading(3, Plan::Chatgpt, "5h", 0.50, None),
        reading(4, Plan::Chatgpt, "5h", 0.60, None),
        reading(5, Plan::Claude, "5h", 0.25, None),
    ];
    let shares = plan_shares(&[&one, &two]);
    assert_eq!(shares[1].len(), 2);
    assert_eq!(shares[1][0].plan, Plan::Claude, "sorted by plan");
    assert!((shares[1][0].used - 0.05).abs() < 1e-9);
    assert!((shares[1][1].used - 0.10).abs() < 1e-9);

    let merged = merge_plan_shares(shares.iter().flatten());
    assert_eq!(merged.len(), 2);
    let claude = &merged[0];
    assert_eq!((claude.first, claude.last), (0.10, 0.25));
    assert!((claude.used - 0.15).abs() < 1e-9);
}

#[test]
fn shares_and_their_sums_are_exactly_what_they_were() {
    let gate = [
        reading(10, Plan::Claude, "5h", 0.10, Some(5_000)),
        reading(10, Plan::Claude, "7d", 0.50, Some(9_000)),
        reading(30, Plan::Claude, "5h", 0.14, Some(5_000)),
        reading(50, Plan::Claude, "5h", 0.20, Some(5_000)),
        reading(50, Plan::Chatgpt, "5h", 0.30, None),
        reading(5, Plan::Claude, "7d", 0.49, Some(9_000)),
    ];
    let other = [
        reading(10, Plan::Claude, "5h", 0.11, Some(5_000)),
        reading(20, Plan::Claude, "5h", 0.12, Some(5_000)),
        reading(40, Plan::Claude, "5h", 0.13, Some(5_000)),
        reading(45, Plan::Claude, "5h", 0.17, None),
        reading(60, Plan::Claude, "5h", 0.02, Some(23_000)),
        reading(60, Plan::Chatgpt, "5h", 0.35, Some(7_000)),
        reading(70, Plan::Claude, "7d", 0.53, Some(9_030)),
    ];
    let shares = plan_shares(&[&gate, &other, &[]]);
    let expected = [
        vec![
            share(Plan::Claude, "5h", (0.1, 0.2, 0.05000000000000002)),
            share(Plan::Claude, "7d", (0.49, 0.5, 0.010000000000000009)),
            share(Plan::Chatgpt, "5h", (0.3, 0.3, 0.0)),
        ],
        vec![
            share(Plan::Claude, "5h", (0.11, 0.02, 0.06999999999999999)),
            share(Plan::Claude, "7d", (0.53, 0.53, 0.030000000000000027)),
            share(Plan::Chatgpt, "5h", (0.35, 0.35, 0.04999999999999999)),
        ],
        Vec::new(),
    ];
    assert_eq!(shares, expected);
    let merged = merge_plan_shares(shares.iter().flatten().chain(&shares[0]));
    let expected = [
        share(Plan::Claude, "5h", (0.1, 0.2, 0.17000000000000004)),
        share(Plan::Claude, "7d", (0.49, 0.5, 0.050000000000000044)),
        share(Plan::Chatgpt, "5h", (0.3, 0.3, 0.04999999999999999)),
    ];
    assert_eq!(merged, expected);
}
