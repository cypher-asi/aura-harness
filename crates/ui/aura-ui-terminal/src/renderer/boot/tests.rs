
use super::*;
use crate::events::{BootLogLine, BootUpdate};

fn boot_lines(
    console: &BootConsole,
    expanded: bool,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    super::boot_lines(console, expanded, spinner, width, theme).0
}

fn text(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn console() -> BootConsole {
    let mut console = BootConsole::new("b1", "Moving to the cloud");
    for (key, state) in [
        ("quota", BootStageState::Done { ms: 210 }),
        ("snapshot", BootStageState::Done { ms: 192_000 }),
        ("create", BootStageState::Running),
    ] {
        console.apply(BootUpdate::Stage {
            key: key.into(),
            label: format!("Step {key}"),
            state,
        });
    }
    console.apply(BootUpdate::Fact {
        key: "machine".into(),
        value: "aura-re36-3f9c".into(),
    });
    console.apply(BootUpdate::Fact {
        key: "region".into(),
        value: "nyc".into(),
    });
    console.apply(BootUpdate::Log(
        (0..12)
            .map(|i| BootLogLine {
                stderr: i == 11,
                text: format!("line {i}"),
            })
            .collect(),
    ));
    console
}

#[test]
fn the_copy_button_is_drawn_where_its_click_lands() {
    let theme = Theme::cyber();
    let console = console();
    for width in [40, 80, 120] {
        let (lines, _) = super::boot_lines(&console, false, "⠋", width, &theme);
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.ends_with(COPY_BUTTON), "{header}");
        assert_eq!(display_width(&header), width);
        let hit = boot_copy(&console, width).expect("room for it");
        assert_eq!(usize::from(hit.x), width - display_width(COPY_BUTTON));
        assert_eq!(hit.text, CopyText::BootLog("b1".into()));
    }
    assert!(boot_copy(&console, 30).is_none());
    let (narrow, _) = super::boot_lines(&console, false, "⠋", 30, &theme);
    assert!(narrow[0].width() <= 30);
}

#[test]
fn a_running_console_shows_steps_facts_and_the_newest_output() {
    let theme = Theme::cyber();
    let shown = text(&boot_lines(&console(), false, "⠋", 80, &theme));
    assert!(shown.starts_with("  ⠋ Moving to the cloud  ·  aura-re36-3f9c  ·  "));
    assert!(shown.contains("✓ Step quota  210 ms"), "{shown}");
    assert!(shown.contains("✓ Step snapshot  3m 12s"), "{shown}");
    assert!(shown.contains("⠋ Step create"), "{shown}");
    assert!(shown.contains("region   nyc"), "{shown}");
    assert!(
        shown.contains("│ … 4 earlier lines · click to load 4 more"),
        "{shown}"
    );
    assert!(shown.contains("│ line 11"));
    assert!(!shown.contains("│ line 3\n"));
}

#[test]
fn the_earlier_lines_row_loads_more_until_none_are_left() {
    let theme = Theme::cyber();
    let mut console = console();
    let (lines, more) = super::boot_lines(&console, false, "⠋", 80, &theme);
    assert_eq!(more.len(), 1);
    let (row, section) = more[0];
    assert_eq!(section, MoreSection::BootLog);
    assert!(text(&lines[row..=row]).contains("4 earlier lines"));

    console.show_earlier(2);
    let shown = text(&boot_lines(&console, false, "⠋", 80, &theme));
    assert!(shown.contains("│ … 2 earlier lines"), "{shown}");
    assert!(shown.contains("│ line 2\n"), "{shown}");

    console.show_earlier(MORE_PAGE_LINES);
    let (lines, more) = super::boot_lines(&console, false, "⠋", 80, &theme);
    assert!(more.is_empty());
    let shown = text(&lines);
    assert!(!shown.contains("earlier lines"), "{shown}");
    assert!(shown.contains("│ line 0\n"), "{shown}");
}

#[test]
fn a_finished_console_folds_to_its_outcome() {
    let theme = Theme::cyber();
    let mut done = console();
    done.apply(BootUpdate::End {
        ok: true,
        summary: "Now running in the cloud.".into(),
    });
    let folded = text(&boot_lines(&done, false, "⠋", 80, &theme));
    assert!(folded.starts_with("  ▸ ✓ Moving to the cloud"), "{folded}");
    assert!(folded.contains("Now running in the cloud."));
    assert!(!folded.contains("Step quota"));
    let open = text(&boot_lines(&done, true, "⠋", 80, &theme));
    assert!(open.starts_with("  ▾ ✓ "));
    assert!(open.contains("│ line 0"), "the whole log once opened");

    let mut failed = console();
    failed.apply(BootUpdate::End {
        ok: false,
        summary: "claim failed".into(),
    });
    let failed = text(&boot_lines(&failed, true, "⠋", 80, &theme));
    assert!(failed.starts_with("  ▾ ✗ "));
    assert!(failed.contains("✗ Step create  stopped"), "{failed}");
}

#[test]
fn a_running_console_shows_a_bar_of_its_steps_and_those_to_come() {
    let theme = Theme::cyber();
    let mut running = console();
    running.apply(BootUpdate::Plan(vec![crate::events::BootPlanStep {
        key: "claim".into(),
        label: "Claiming aura node".into(),
    }]));
    let lines = boot_lines(&running, false, "⠋", 80, &theme);
    let bar = text(&lines[1..2]);
    assert!(bar.ends_with("   50%"), "2 of 4 steps done: {bar}");
    assert!(bar.contains('█') && bar.contains('░'), "{bar}");
    assert!(bar.contains('▒') || bar.contains('▓'), "{bar}");
    let shown = text(&lines);
    assert!(shown.contains("· Claiming aura node"), "{shown}");

    running.apply(BootUpdate::End {
        ok: true,
        summary: "done".into(),
    });
    let done = text(&boot_lines(&running, true, "⠋", 80, &theme));
    assert!(!done.contains('░') && !done.contains('%'), "{done}");
    assert!(!done.contains("Claiming aura node"), "never begun");
}

#[test]
fn long_lines_fit_the_width() {
    let theme = Theme::cyber();
    let mut console = console();
    console.apply(BootUpdate::Log(vec![BootLogLine {
        stderr: false,
        text: "x".repeat(300),
    }]));
    for line in boot_lines(&console, true, "⠋", 40, &theme) {
        assert!(line.width() <= 40, "{line:?}");
    }
}

#[test]
fn build_output_is_colored_by_what_it_says() {
    let theme = Theme::cyber();
    let colors = |text: &str| -> Vec<(String, Option<ratatui::style::Color>)> {
        log_spans(text, &theme)
            .into_iter()
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect()
    };
    let compiling = colors("   Compiling tokio v1.49.0 (/workspace/tokio)");
    assert_eq!(
        compiling[1],
        ("Compiling".to_string(), Some(theme.colors.success))
    );
    assert_eq!(compiling[3], ("tokio".to_string(), Some(theme.colors.text)));
    assert_eq!(compiling[4].1, Some(theme.colors.muted));
    assert_eq!(
        colors("error[E0599]: no variant")[0],
        ("error".to_string(), Some(theme.colors.error))
    );
    assert_eq!(
        colors("warning: unused import")[0].1,
        Some(theme.colors.warning)
    );
    assert_eq!(colors("$ cargo fetch")[1].1, Some(theme.colors.text));
    // Prose that only starts like a status line stays quiet.
    assert_eq!(
        colors("Compiling went fine"),
        [("Compiling went fine".to_string(), Some(theme.colors.muted))]
    );
    assert_eq!(colors("  Note this").len(), 1);
}

#[test]
fn a_finished_console_folds_and_opens_on_its_header_outcome_and_hint() {
    let theme = Theme::cyber();
    let mut console = console();
    assert!(boot_fold_rows(&console, false, 80).is_empty(), "running");
    console.apply(BootUpdate::End {
        ok: true,
        summary: "Now running in the cloud on aura-age1301-acaa (4 vCPU). Its record \
                      syncs back here; `/local` brings it back."
            .into(),
    });
    let folded = boot_lines(&console, false, "⠋", 80, &theme);
    let rows = boot_fold_rows(&console, false, 80);
    assert_eq!(rows, 0..4, "header, two summary rows, hint");
    assert!(text(&folded[rows.end - 1..rows.end]).contains("click to show the boot log"));
    assert_eq!(rows.end, folded.len());

    let open = boot_lines(&console, true, "⠋", 80, &theme);
    let rows = boot_fold_rows(&console, true, 80);
    assert_eq!(rows, 0..3, "header and summary");
    assert!(text(&open[1..3]).contains("`/local` brings it back."));
    assert!(!text(&open[rows]).contains("Step quota"));
}

#[test]
fn a_running_step_shows_what_it_waits_on_and_its_limit() {
    let theme = Theme::cyber();
    let mut console = console();
    console.apply(BootUpdate::Waiting {
        key: "create".into(),
        what: Some("boxd is creating aura-re36-3f9c from snapshot proj-x".into()),
        limit_ms: Some(180_000),
    });
    let shown = text(&boot_lines(&console, false, "⠋", 120, &theme));
    assert!(
        shown.contains("↳ boxd is creating aura-re36-3f9c from snapshot proj-x"),
        "{shown}"
    );
    assert!(shown.contains(" of 3m 00s"), "{shown}");
    assert!(!shown.contains("past its time limit"));

    // Past its limit, it says so.
    let step = &mut console.steps[2];
    if let Some(wait) = step.waiting.as_mut() {
        wait.since = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(200))
            .expect("an instant 200s ago");
    }
    let late = text(&boot_lines(&console, false, "⠋", 120, &theme));
    assert!(late.contains("past its time limit"), "{late}");

    console.apply(BootUpdate::Waiting {
        key: "create".into(),
        what: None,
        limit_ms: None,
    });
    assert!(!text(&boot_lines(&console, false, "⠋", 120, &theme)).contains('↳'));
}

#[test]
fn a_quiet_console_says_how_long_it_has_heard_nothing() {
    let theme = Theme::cyber();
    let mut console = console();
    assert!(!text(&boot_lines(&console, false, "⠋", 120, &theme)).contains("no news"));
    console.last_update = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(47))
        .expect("an instant 47s ago");
    let header = text(&boot_lines(&console, false, "⠋", 120, &theme)[..1]);
    assert!(header.contains("no news for 47.0s"), "{header}");

    console.apply(BootUpdate::End {
        ok: true,
        summary: "done".into(),
    });
    console.last_update = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(47))
        .expect("an instant 47s ago");
    assert!(!text(&boot_lines(&console, false, "⠋", 120, &theme)).contains("no news"));
}

#[test]
fn durations_read_naturally() {
    assert_eq!(duration(850), "850 ms");
    assert_eq!(duration(12_450), "12.4s");
    assert_eq!(duration(185_000), "3m 05s");
}
