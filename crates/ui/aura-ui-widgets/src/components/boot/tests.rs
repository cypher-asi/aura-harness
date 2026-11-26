use super::*;
use aura_ui::events::{BootPlanStep, BootUpdate};

fn stage(key: &str, state: BootStageState) -> BootUpdate {
    BootUpdate::Stage {
        key: key.into(),
        label: format!("Doing {key}"),
        state,
    }
}

#[test]
fn updates_build_the_console() {
    let mut console = BootConsole::new("b1", "Moving to the cloud");
    console.apply(stage("create", BootStageState::Running));
    console.apply(BootUpdate::Fact {
        key: "region".into(),
        value: "nyc".into(),
    });
    console.apply(BootUpdate::Fact {
        key: "region".into(),
        value: "fra".into(),
    });
    console.apply(stage("create", BootStageState::Done { ms: 1200 }));
    console.apply(BootUpdate::Progress {
        key: "import_record".into(),
        done: 2,
        total: 5,
    });
    assert_eq!(console.steps.len(), 2);
    assert_eq!(console.steps[0].state, BootStageState::Done { ms: 1200 });
    assert_eq!(console.steps[1].progress, Some((2, 5)));
    assert_eq!(console.fact("region"), Some("fra"));
    assert_eq!(console.facts.len(), 1);
    assert!(console.is_running());

    console.apply(BootUpdate::End {
        ok: false,
        summary: "claim failed".into(),
    });
    assert!(!console.is_running());
    assert_eq!(
        console.steps[1].state,
        BootStageState::Failed("stopped".into())
    );
}

#[test]
fn a_plan_lists_the_steps_to_come_and_unplanned_ones_go_before_them() {
    let mut console = BootConsole::new("b1", "Bringing the agent back");
    console.apply(BootUpdate::Plan(
        ["seal", "copy_record"]
            .into_iter()
            .map(|key| BootPlanStep {
                key: key.into(),
                label: key.into(),
            })
            .collect(),
    ));
    console.apply(stage("wake", BootStageState::Running));
    let keys = |c: &BootConsole| c.steps.iter().map(|s| s.key.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&console), ["wake", "seal", "copy_record"]);
    assert!(console.fraction().abs() < f64::EPSILON);

    console.apply(stage("wake", BootStageState::Done { ms: 900 }));
    console.apply(stage("seal", BootStageState::Running));
    console.apply(BootUpdate::Progress {
        key: "seal".into(),
        done: 1,
        total: 2,
    });
    assert!((console.fraction() - 0.5).abs() < 1e-9, "1.5 of 3 steps");

    console.apply(BootUpdate::End {
        ok: false,
        summary: "seal failed".into(),
    });
    assert_eq!(keys(&console), ["wake", "seal"], "steps never begun go");
}

fn waiting(key: &str, what: Option<&str>, limit_ms: Option<u64>) -> BootUpdate {
    BootUpdate::Waiting {
        key: key.into(),
        what: what.map(str::to_string),
        limit_ms,
    }
}

#[test]
fn a_wait_is_set_reworded_and_cleared() {
    let mut console = BootConsole::new("b1", "Moving to the cloud");
    console.apply(stage("create", BootStageState::Running));
    console.apply(waiting(
        "create",
        Some("boxd is creating aura-1"),
        Some(180_000),
    ));
    let wait = console.steps[0].waiting.clone().expect("waits");
    assert_eq!(wait.what, "boxd is creating aura-1");
    assert_eq!(wait.limit_ms, Some(180_000));

    console.apply(waiting("create", Some("boxd says pending"), Some(180_000)));
    let reworded = console.steps[0].waiting.clone().expect("still waits");
    assert_eq!(reworded.what, "boxd says pending");
    assert_eq!(reworded.since, wait.since, "rewording keeps the clock");
    assert!(console
        .transcript()
        .contains("… Doing create (running; waiting on boxd says pending, "));

    console.apply(waiting("create", None, None));
    assert_eq!(console.steps[0].waiting, None);
    console.apply(waiting("create", Some("again"), None));
    assert!(console.steps[0].waiting.as_ref().unwrap().since >= wait.since);

    console.apply(waiting("claim", None, None));
    assert_eq!(console.steps.len(), 1, "a clear never adds a step");
}

#[test]
fn a_step_that_ends_or_a_console_that_ends_waits_on_nothing() {
    let mut console = BootConsole::new("b1", "t");
    console.apply(stage("create", BootStageState::Running));
    console.apply(waiting("create", Some("create"), None));
    console.apply(stage("create", BootStageState::Done { ms: 5 }));
    assert_eq!(console.steps[0].waiting, None);

    console.apply(stage("claim", BootStageState::Running));
    console.apply(waiting("claim", Some("claim"), None));
    console.apply(BootUpdate::End {
        ok: false,
        summary: "stopped".into(),
    });
    assert!(console.steps.iter().all(|s| s.waiting.is_none()));
}

#[test]
fn every_update_resets_the_quiet_clock() {
    let mut console = BootConsole::new("b1", "t");
    console.last_update = Instant::now()
        .checked_sub(Duration::from_secs(90))
        .expect("an instant 90s ago");
    assert!(console.quiet_ms() >= 90_000);
    console.apply(waiting("create", Some("x"), None));
    assert!(console.quiet_ms() < 90_000);
}

#[test]
fn a_wait_clock_reads_against_its_limit() {
    let wait = BootWait {
        what: "x".into(),
        since: Instant::now(),
        limit_ms: Some(180_000),
    };
    assert!(
        wait_clock(&wait).ends_with(" of 3m 00s"),
        "{}",
        wait_clock(&wait)
    );
    let open = BootWait {
        limit_ms: None,
        ..wait
    };
    assert!(!wait_clock(&open).contains(" of "));
}

#[test]
fn the_log_keeps_the_newest_lines() {
    let mut console = BootConsole::new("b1", "t");
    let lines = (0..MAX_BOOT_LOG_LINES + 20)
        .map(|i| BootLogLine {
            stderr: false,
            text: i.to_string(),
        })
        .collect();
    console.apply(BootUpdate::Log(lines));
    assert_eq!(console.log.len(), MAX_BOOT_LOG_LINES);
    assert_eq!(console.dropped, 20);
    assert_eq!(console.log.front().map(|l| l.text.as_str()), Some("20"));
}

fn numbered(range: std::ops::Range<usize>) -> BootUpdate {
    BootUpdate::Log(
        range
            .map(|i| BootLogLine {
                stderr: i % 7 == 0,
                text: format!("line {i}"),
            })
            .collect(),
    )
}

fn texts(lines: &VecDeque<BootLogLine>) -> Vec<String> {
    lines.iter().map(|l| l.text.clone()).collect()
}

#[test]
fn earlier_lines_come_from_memory_then_from_the_spill_file() {
    let mut console = BootConsole::new("b1", "t");
    console.apply(numbered(0..MAX_BOOT_LOG_LINES + 300));
    console.apply(numbered(MAX_BOOT_LOG_LINES + 300..MAX_BOOT_LOG_LINES + 350));
    assert_eq!(console.dropped, 350);
    assert_eq!(console.log_shown(), LIVE_BOOT_LOG_LINES);
    assert_eq!(
        console.hidden(),
        350 + MAX_BOOT_LOG_LINES - LIVE_BOOT_LOG_LINES
    );
    assert_eq!(console.can_show(), console.hidden());

    console.show_earlier(MAX_BOOT_LOG_LINES - LIVE_BOOT_LOG_LINES + 100);
    assert_eq!(console.log_shown(), MAX_BOOT_LOG_LINES, "memory first");
    assert_eq!(
        texts(&console.earlier),
        (250..350).map(|i| format!("line {i}")).collect::<Vec<_>>()
    );
    assert!(console.earlier[0].stderr == (250 % 7 == 0));
    assert_eq!(console.hidden(), 250);

    console.apply(numbered(850..852));
    assert_eq!(
        console.earlier.back().map(|l| l.text.as_str()),
        Some("line 351"),
        "lines dropped once earlier ones show stay shown"
    );
    console.show_earlier(1000);
    assert_eq!(console.earlier.len(), 352);
    assert_eq!(console.earlier[0].text, "line 0");
    assert_eq!(console.hidden(), 0);
    assert_eq!(console.can_show(), 0);
}

#[test]
fn the_transcript_has_the_whole_log_spilled_lines_included() {
    let mut console = BootConsole::new("b1", "Moving to the cloud");
    console.apply(stage(
        "snapshot",
        BootStageState::Failed("build failed".into()),
    ));
    console.apply(BootUpdate::Fact {
        key: "machine".into(),
        value: "aura-1".into(),
    });
    console.apply(numbered(0..MAX_BOOT_LOG_LINES + 40));
    console.apply(BootUpdate::End {
        ok: false,
        summary: "Could not move the agent".into(),
    });
    let text = console.transcript();
    assert!(
        text.starts_with("Moving to the cloud (failed after "),
        "{text}"
    );
    assert!(text.contains("\nCould not move the agent\n"));
    assert!(text.contains("✗ Doing snapshot  build failed\n"));
    assert!(text.contains("machine  aura-1\n"));
    let log: Vec<&str> = text.lines().filter(|l| l.starts_with("line ")).collect();
    assert_eq!(log.len(), MAX_BOOT_LOG_LINES + 40, "every line, none twice");
    assert_eq!(log[0], "line 0");
    assert_eq!(log.last().copied(), Some("line 539"));
}

#[test]
fn the_spill_file_goes_with_the_last_copy() {
    let mut console = BootConsole::new("b1", "t");
    console.apply(numbered(0..MAX_BOOT_LOG_LINES + 1));
    let path = console
        .spill
        .as_ref()
        .map(|s| s.lock().path.clone())
        .unwrap();
    assert!(path.exists());
    let copy = console.clone();
    drop(console);
    assert!(path.exists());
    drop(copy);
    assert!(!path.exists());
}
