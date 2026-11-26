use super::*;
use crate::components::{LiveOutput, MORE_PAGE_LINES};
use crate::themes::Theme;

fn activity_lines(
    activity: &Activity,
    expanded: bool,
    number: Option<StepNumber>,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    super::activity_lines(activity, expanded.into(), number, spinner, width, theme).0
}

fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn text(line: &Line<'_>) -> String {
    texts(std::slice::from_ref(line)).remove(0)
}

fn tool_step(name: &str, args: &str, status: ToolStatus, result: Option<&str>) -> Activity {
    Activity::Tool(ToolStep {
        id: "t1".to_string(),
        name: name.to_string(),
        args: args.to_string(),
        status,
        result: result.map(str::to_string),
        retry: None,
        started: None,
        duration_ms: Some(300),
        diff: None,
        policy: None,
        awaiting_approval: false,
        live: LiveOutput::default(),
    })
}

fn tool(status: ToolStatus, result: Option<&str>) -> Activity {
    tool_step("read_file", r#"{"path":"src/main.rs"}"#, status, result)
}

fn thought(ms: u64) -> Activity {
    Activity::Thinking(ThinkingStep {
        text: "hmm".to_string(),
        started: None,
        duration_ms: Some(ms),
        running: false,
    })
}

#[test]
fn collapsed_tool_is_one_header_line() {
    let lines = activity_lines(
        &tool(ToolStatus::Success, Some("a\nb\nc")),
        false,
        None,
        "⠋",
        80,
        &Theme::default(),
    );
    assert_eq!(texts(&lines), ["  ▸ Read main.rs"]);
}

#[test]
fn nested_step_is_numbered_with_its_body_under_the_label() {
    let step = tool(ToolStatus::Success, Some("fn main() {}"));
    let theme = Theme::default();
    let collapsed = activity_lines(&step, false, Some(StepNumber::new(3, 3)), "⠋", 80, &theme);
    assert_eq!(texts(&collapsed), ["    3 ▸ Read main.rs"]);
    let expanded = activity_lines(&step, true, Some(StepNumber::new(12, 12)), "⠋", 80, &theme);
    assert_eq!(texts(&expanded)[0], "    12 ▾ Read main.rs");
    assert_eq!(texts(&expanded).last().unwrap(), "         │ fn main() {}");
}

#[test]
fn numbers_in_a_group_share_one_marker_column_under_the_summary() {
    let step = tool(ToolStatus::Success, None);
    let theme = Theme::default();
    let header = text(&group_header(
        &[&step],
        GroupView::Full,
        false,
        "",
        "⠋",
        80,
        &theme,
    ));
    let row =
        |n| text(&activity_lines(&step, false, Some(StepNumber::new(n, 10)), "⠋", 80, &theme)[0]);
    assert_eq!(header, "  ▾ Explored main.rs");
    assert_eq!(row(9), "     9 ▸ Read main.rs");
    assert_eq!(row(10), "    10 ▸ Read main.rs");
    let label = display_width(&header[..header.find("Explored").unwrap()]);
    assert_eq!(row(10).find("10"), Some(label));
}

#[test]
fn open_thinking_without_text_says_so() {
    let step = Activity::Thinking(ThinkingStep {
        text: String::new(),
        started: None,
        duration_ms: Some(5000),
        running: false,
    });
    let theme = Theme::default();
    let one = Some(StepNumber::new(1, 1));
    let closed = activity_lines(&step, false, one, "⠋", 80, &theme);
    assert_eq!(texts(&closed), ["    1 ▸ Thought 5s"]);
    let open = activity_lines(&step, true, one, "⠋", 80, &theme);
    assert_eq!(
        texts(&open),
        [
            "    1 ▾ Thought 5s",
            "        │ the model shared no reasoning text"
        ]
    );
    assert_eq!(
        text(&records_link(one, 80, &theme)),
        "        │ → records for this turn"
    );
}

#[test]
fn expanded_tool_shows_args_and_result() {
    let lines = activity_lines(
        &tool(ToolStatus::Error, Some("no such file")),
        true,
        None,
        "⠋",
        80,
        &Theme::default(),
    );
    let text = texts(&lines).join("\n");
    assert!(
        text.starts_with("  ▾ ✗ Read main.rs · no such file"),
        "{text}"
    );
    assert!(text.contains("│ path: src/main.rs"), "{text}");
    assert!(text.ends_with("│ no such file"), "{text}");
}

#[test]
fn expanded_task_shows_its_prompt_as_text_and_its_reply_as_markdown() {
    let args = serde_json::json!({
        "subagent_type": "explore",
        "prompt": "Read-only.\n\nDo two things.",
    });
    let step = tool_step(
        "task",
        &args.to_string(),
        ToolStatus::Success,
        Some("**Done**\n\n| a | b |\n|---|---|\n| 1 | 2 |"),
    );
    let lines = activity_lines(&step, true, None, "⠋", 80, &Theme::default());
    let text = texts(&lines).join("\n");
    assert!(
        text.starts_with("  ▾ Delegated to @explore: Read-only."),
        "{text}"
    );
    assert!(
        text.contains("│ prompt:\n    │   Read-only.\n    │ \n    │   Do two things."),
        "{text}"
    );
    assert!(text.contains("│ subagent_type: explore"), "{text}");
    assert!(!text.contains("**"), "{text}");
    assert!(!text.contains("|---|"), "{text}");
}

#[test]
fn expanded_command_shows_a_prompt_one_statement_to_a_row() {
    let args = serde_json::json!({
        "allow_shell": true,
        "background": true,
        "cwd": ".worktrees/channels-merge",
        "shell_script": "cargo fmt --all -- --check && echo FMT_OK; \
                         cargo clippy --all-targets 2>&1 | grep -E '^(error|warning)' | head -40; \
                         echo DONE",
    });
    let result = crate::app::format::format_tool_result(
        "run_command",
        "Started in the background (pid 42): cargo fmt\nJob id: 7697e889\n\
             Output: ./aura_data/processes/7697e889.log\nYou will get a message.",
    );
    let step = tool_step(
        "run_command",
        &args.to_string(),
        ToolStatus::Success,
        Some(&result),
    );
    let lines = activity_lines(&step, true, None, "⠋", 60, &Theme::default());
    assert_eq!(
        texts(&lines)[1..],
        [
            "    │ $ cargo fmt --all -- --check &&",
            "    │   echo FMT_OK",
            "    │   cargo clippy --all-targets 2>&1 | grep -E ",
            "    │     '^(error|warning)' | head -40",
            "    │   echo DONE",
            "    │ in .worktrees/channels-merge · background",
            "    │ ",
            "    │ Started in the background · pid 42",
            "    │ job 7697e889",
            "    │ log ./aura_data/processes/7697e889.log",
        ]
    );
}

#[test]
fn expanded_program_call_quotes_its_args_and_keeps_other_keys() {
    let args = serde_json::json!({
        "program": "git",
        "args": ["commit", "-m", "fix it's done"],
        "timeout_secs": 90,
        "extra": 1,
    });
    let step = tool_step("run_command", &args.to_string(), ToolStatus::Success, None);
    let lines = activity_lines(&step, true, None, "⠋", 80, &Theme::default());
    assert_eq!(
        texts(&lines)[1..],
        [
            r"    │ $ git commit -m 'fix it'\''s done'",
            "    │ timeout 1m 30s",
            "    │ extra: 1",
        ]
    );
}

#[test]
fn policy_shows_only_when_expanded() {
    let mut step = tool(ToolStatus::Success, Some("fn main() {}"));
    if let Activity::Tool(tool) = &mut step {
        tool.policy = Some("allowed by user.on".to_string());
    }
    let theme = Theme::default();
    let collapsed = activity_lines(&step, false, None, "⠋", 80, &theme);
    assert_eq!(texts(&collapsed), ["  ▸ Read main.rs"]);
    let expanded = activity_lines(&step, true, None, "⠋", 80, &theme);
    assert_eq!(texts(&expanded)[1], "    │ policy: allowed by user.on");
}

#[test]
fn expanded_result_is_capped() {
    let output: Vec<String> = (0..250).map(|i| format!("line {i}")).collect();
    let lines = activity_lines(
        &tool(ToolStatus::Success, Some(&output.join("\n"))),
        true,
        None,
        "⠋",
        80,
        &Theme::default(),
    );
    assert_eq!(
        texts(&lines).last().unwrap(),
        "    │ … 50 more lines · click to show 50 more"
    );
}

#[test]
fn each_page_moves_the_cut_of_a_result_until_it_is_all_shown() {
    let output: Vec<String> = (0..650).map(|i| format!("line {i}")).collect();
    let step = tool(ToolStatus::Success, Some(&output.join("\n")));
    let theme = Theme::default();
    let draw = |result| {
        let shown = Shown {
            expanded: true,
            result,
            diff: 0,
        };
        super::activity_lines(&step, shown, None, "⠋", 80, &theme)
    };
    let (lines, more) = draw(0);
    assert_eq!(more, [(lines.len() - 1, MoreSection::Result)]);
    assert_eq!(
        text(&lines[more[0].0]),
        "    │ … 450 more lines · click to show 200 more"
    );

    let (lines, more) = draw(MORE_PAGE_LINES);
    assert_eq!(
        text(&lines[more[0].0]),
        "    │ … 250 more lines · click to show 200 more"
    );
    assert_eq!(text(&lines[more[0].0 - 1]), "    │ line 399");

    let (lines, more) = draw(3 * MORE_PAGE_LINES);
    assert!(more.is_empty());
    assert_eq!(texts(&lines).last().unwrap(), "    │ line 649");
}

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

#[test]
fn long_label_is_cut_to_width() {
    let activity = Activity::Tool(ToolStep {
        id: "t1".to_string(),
        name: "run_command".to_string(),
        args: format!(r#"{{"command":"{}"}}"#, "x".repeat(200)),
        status: ToolStatus::Running,
        result: None,
        retry: Some((2, 3)),
        started: None,
        duration_ms: None,
        diff: None,
        policy: None,
        awaiting_approval: false,
        live: LiveOutput::default(),
    });
    let lines = activity_lines(&activity, false, None, "⠋", 40, &Theme::default());
    let header = &texts(&lines)[0];
    assert_eq!(display_width(header), 40, "{header}");
    assert!(header.starts_with("  ⠋ Ran xxx"), "{header}");
    assert!(header.ends_with("… · retrying (2/3)"), "{header}");
}

#[test]
fn streaming_write_shows_its_file_and_size() {
    let args = format!(
        r#"{{"path":"web/index.html","content":"{}"#,
        "x".repeat(3000)
    );
    let step = tool_step("write_file", &args, ToolStatus::Running, None);
    let lines = activity_lines(&step, false, None, "⠋", 80, &Theme::default());
    assert_eq!(texts(&lines), ["  ⠋ Wrote index.html · 2 KB"]);

    let done = tool_step(
        "write_file",
        r#"{"path":"web/index.html","content":"x"}"#,
        ToolStatus::Running,
        None,
    );
    let lines = activity_lines(&done, false, None, "⠋", 80, &Theme::default());
    assert_eq!(texts(&lines), ["  ⠋ Wrote index.html"]);
}

#[test]
fn finished_tool_keeps_how_long_it_took() {
    let mut step = tool_step(
        "write_file",
        r#"{"path":"src/types.ts","content":"x"}"#,
        ToolStatus::Success,
        None,
    );
    if let Activity::Tool(tool) = &mut step {
        tool.duration_ms = Some(28_400);
    }
    let lines = activity_lines(
        &step,
        false,
        Some(StepNumber::new(2, 2)),
        "⠋",
        80,
        &Theme::default(),
    );
    assert_eq!(texts(&lines), ["    2 ▸ Wrote types.ts · 28s"]);
}

#[test]
fn group_header_counts_what_the_steps_did() {
    let read = |path: &str| {
        tool_step(
            "read_file",
            &format!(r#"{{"path":"{path}"}}"#),
            ToolStatus::Success,
            None,
        )
    };
    let search = tool_step(
        "search_code",
        r#"{"pattern":"x"}"#,
        ToolStatus::Success,
        None,
    );
    let failed = tool_step(
        "run_command",
        r#"{"command":"ls"}"#,
        ToolStatus::Error,
        None,
    );
    let steps = [
        thought(500),
        read("a.rs"),
        read("b.rs"),
        read("a.rs"),
        search.clone(),
        search,
        failed,
    ];
    let steps: Vec<&Activity> = steps.iter().collect();
    let theme = Theme::default();

    let done = group_header(&steps, GroupView::Collapsed, false, "", "⠹", 80, &theme);
    assert_eq!(
        text(&done),
        "  ▸ Explored 2 files, 2 searches, ran 1 command · 1 failed"
    );
    let live = group_header(
        &steps[..2],
        GroupView::Full,
        true,
        " · step 2 · 9s",
        "⠹",
        80,
        &theme,
    );
    assert_eq!(text(&live), "  ⠹ Exploring a.rs · step 2 · 9s");
    let open = group_header(&steps[..2], GroupView::Full, false, "", "⠹", 80, &theme);
    assert_eq!(text(&open), "  ▾ Explored a.rs");
}

#[test]
fn collapsed_hint_counts_the_steps() {
    let theme = Theme::default();
    assert_eq!(
        text(&collapsed_hint(5, &theme)),
        "    5 steps (click to expand)"
    );
}

#[test]
fn group_of_thinking_sums_its_time() {
    let steps = [thought(3000), thought(9000)];
    let steps: Vec<&Activity> = steps.iter().collect();
    let header = group_header(
        &steps,
        GroupView::Collapsed,
        false,
        "",
        "⠹",
        80,
        &Theme::default(),
    );
    assert_eq!(text(&header), "  ▸ Thought 12s");
}
