use super::*;

#[test]
fn running_thinking_shows_its_tail_and_finished_collapses() {
    let total = LIVE_THINKING_LINES + 2;
    let mut step = ThinkingStep {
        text: (1..=total)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        started: None,
        duration_ms: None,
        running: true,
    };
    let live = activity_lines(
        &Activity::Thinking(step.clone()),
        false,
        None,
        "⠹",
        80,
        &Theme::default(),
    );
    let expected: Vec<String> = std::iter::once("  ⠹ Thinking…".to_string())
        .chain((3..=total).map(|n| format!("    │ line {n}")))
        .collect();
    assert_eq!(texts(&live), expected);

    step.running = false;
    step.duration_ms = Some(4200);
    let done = activity_lines(
        &Activity::Thinking(step),
        false,
        None,
        "⠹",
        80,
        &Theme::default(),
    );
    assert_eq!(texts(&done), ["  ▸ Thought 4s"]);
}

#[test]
fn short_thinking_is_brief() {
    let lines = activity_lines(&thought(800), false, None, "⠹", 80, &Theme::default());
    assert_eq!(texts(&lines), ["  ▸ Thought briefly"]);
}

#[test]
fn a_call_awaiting_approval_says_so_instead_of_counting_up() {
    let mut activity = tool_step("edit_file", r#"{"path":"a.ts"}"#, ToolStatus::Running, None);
    if let Activity::Tool(step) = &mut activity {
        step.started = Some(std::time::Instant::now() - std::time::Duration::from_secs(90));
        step.awaiting_approval = true;
    }
    let lines = activity_lines(&activity, false, None, "⠋", 80, &Theme::default());
    let header = &texts(&lines)[0];
    assert!(header.ends_with(" · awaiting approval"), "{header}");
    assert!(!header.contains("1m"), "{header}");
}

fn running_command(output: &str) -> Activity {
    let mut activity = tool_step(
        "run_command",
        r#"{"shell_script":"cargo test"}"#,
        ToolStatus::Running,
        None,
    );
    if let Activity::Tool(step) = &mut activity {
        step.started = Some(std::time::Instant::now());
        step.live.push(output);
    }
    activity
}

#[test]
fn a_running_command_shows_its_last_lines_collapsed_and_more_open() {
    let output: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    let activity = running_command(&output);

    let collapsed = texts(&activity_lines(
        &activity,
        false,
        None,
        "⠋",
        80,
        &Theme::default(),
    ));
    assert_eq!(collapsed.len(), 4, "{collapsed:?}");
    assert!(collapsed[0].contains("Ran cargo test"), "{collapsed:?}");
    assert_eq!(
        collapsed[1..],
        ["    │ line 28", "    │ line 29", "    │ line 30"]
    );

    let open = texts(&activity_lines(
        &activity,
        true,
        None,
        "⠋",
        80,
        &Theme::default(),
    ));
    let shown: Vec<&String> = open.iter().filter(|l| l.contains("│ line ")).collect();
    assert_eq!(shown.len(), LIVE_OUTPUT_LINES, "{open:?}");
    assert!(shown[0].ends_with("line 11"), "{open:?}");
    assert!(open.last().unwrap().ends_with("line 30"), "{open:?}");
}

#[test]
fn a_finished_command_shows_no_live_output() {
    let mut activity = running_command("partial\n");
    if let Activity::Tool(step) = &mut activity {
        step.status = ToolStatus::Success;
    }
    let collapsed = texts(&activity_lines(
        &activity,
        false,
        None,
        "⠋",
        80,
        &Theme::default(),
    ));
    assert_eq!(collapsed.len(), 1, "{collapsed:?}");
}

#[test]
fn a_long_quiet_command_says_how_long_it_printed_nothing() {
    let ago = |secs| std::time::Instant::now().checked_sub(std::time::Duration::from_secs(secs));
    let mut activity = running_command("");
    if let Activity::Tool(step) = &mut activity {
        step.started = ago(95);
    }
    let header = texts(&activity_lines(
        &activity,
        false,
        None,
        "⠋",
        80,
        &Theme::default(),
    ))
    .remove(0);
    assert!(header.ends_with(" · 1m 35s · no output 1m 35s"), "{header}");

    if let Activity::Tool(step) = &mut activity {
        step.live.push("still going\n");
    }
    let header = texts(&activity_lines(
        &activity,
        false,
        None,
        "⠋",
        80,
        &Theme::default(),
    ))
    .remove(0);
    assert!(!header.contains("no output"), "{header}");

    let mut read = tool_step("read_file", "{}", ToolStatus::Running, None);
    if let Activity::Tool(step) = &mut read {
        step.started = ago(95);
    }
    let header = texts(&activity_lines(
        &read,
        false,
        None,
        "⠋",
        80,
        &Theme::default(),
    ))
    .remove(0);
    assert!(
        !header.contains("no output"),
        "only commands print: {header}"
    );
}
