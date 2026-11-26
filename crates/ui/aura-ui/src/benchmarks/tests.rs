use super::*;

#[test]
fn the_larger_cost_is_the_whole_and_the_other_says_how_much_less() {
    assert_eq!(
        cost_shares(Some(0.14), Some(0.21)),
        [" · 33% less", " · 100%"]
    );
    assert_eq!(
        cost_shares(Some(0.11), Some(0.04)),
        [" · 100%", " · 64% less"]
    );
    assert_eq!(cost_shares(Some(1.0), Some(0.996)), [" · 100%", " · 100%"]);
    assert_eq!(cost_shares(Some(0.0), Some(0.0)), ["", ""]);
    assert_eq!(cost_shares(None, Some(0.2)), ["", ""]);
}

fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else {
        format!("${usd:.2}")
    }
}

#[test]
fn a_cost_with_unpriced_calls_reads_as_a_part_or_unknown_never_free() {
    assert_eq!(BenchCost::new(0.0, 0).read(money), "free", "a local model");
    assert_eq!(BenchCost::new(0.11, 0).read(money), "$0.11");
    assert_eq!(BenchCost::new(0.11, 2).read(money), "$0.11+?");
    assert_eq!(BenchCost::new(0.0, 3).read(money), "unknown");
    assert_eq!(BenchCost::new(0.11, 2).known(), None);
    assert_eq!(BenchCost::new(0.11, 0).known(), Some(0.11));
}

#[test]
fn a_partly_priced_baseline_has_no_known_cost() {
    let baseline = |priced, unpriced_calls| BenchBaseline {
        calls: 4,
        cost_usd: 0.2,
        priced,
        unpriced_calls,
        models: Vec::new(),
    };
    assert_eq!(
        baseline(true, 0).cost().and_then(BenchCost::known),
        Some(0.2)
    );
    let partly = baseline(true, 1).cost();
    assert_eq!(partly.map(|c| c.read(money)).as_deref(), Some("$0.20+?"));
    assert_eq!(partly.and_then(BenchCost::known), None);
    assert_eq!(baseline(false, 4).cost(), None);
}

#[test]
fn per_fix_cells_compare_only_complete_costs() {
    let fix = |usd, unpriced| cost_per_fix(BenchCost::new(usd, unpriced), 2);
    assert_eq!(
        per_fix_cells(fix(0.4, 0), fix(0.2, 0), money),
        ["$0.20 · 100%", "$0.10 · 50% less"]
    );
    assert_eq!(
        per_fix_cells(fix(0.4, 0), fix(0.0, 4), money),
        ["$0.20", "unknown"],
        "no -100% from a missing price"
    );
    assert_eq!(
        per_fix_cells(None, fix(0.2, 1), money),
        ["no fix", "$0.10+?"]
    );
}

#[test]
fn the_unpriced_note_names_each_model_once() {
    assert_eq!(unpriced_note([]), None);
    assert_eq!(
        unpriced_note(["glm-9", "mystery", "glm-9"]).as_deref(),
        Some("+? some calls have no known price: glm-9, mystery")
    );
}

#[test]
fn progress_counts_runs_and_follows_the_phases() {
    let task = |setup| BenchTaskRow {
        setup,
        ..BenchTaskRow::default()
    };
    let mut view = BenchView {
        repeats: 2,
        tasks: vec![
            task(BenchTaskSetup::Fetching { pull: true }),
            task(BenchTaskSetup::Queued),
        ],
        live: vec![BenchLiveRun::default()],
        ..BenchView::default()
    };
    let progress = view.progress();
    assert_eq!((progress.total, progress.done), (8, 0));
    assert_eq!(progress.phase, BenchPhase::SettingUp);

    view.tasks[0].setup = BenchTaskSetup::Ready;
    view.live[0].wall_ms = Some(1_000);
    view.runs = vec![
        BenchRun::default(),
        BenchRun {
            error: Some("timed out".into()),
            ..BenchRun::default()
        },
    ];
    let progress = view.progress();
    assert_eq!(progress.phase, BenchPhase::Running);
    assert_eq!(
        (progress.done, progress.failed, progress.working),
        (2, 1, 1)
    );

    view.live.clear();
    view.runs = vec![BenchRun::default(); 8];
    assert_eq!(view.progress().phase, BenchPhase::Judging);
    view.status = BenchStatus::Applied("a-1".into());
    assert_eq!(view.progress().phase, BenchPhase::Done);
    assert!(!BenchPhase::Done.under_way());
    view.status = BenchStatus::Cancelled;
    assert_eq!(view.progress().phase, BenchPhase::Stopped);
}

#[test]
fn statuses_read_naturally_and_know_their_copies() {
    assert_eq!(BenchStatus::Applied("a-2".into()).label(), "applied a-2");
    assert!(BenchStatus::Done.has_copies());
    assert!(!BenchStatus::Dismissed.has_copies());
    assert_eq!(arm_letter(0), "A");
}
