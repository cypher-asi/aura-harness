use super::*;

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
        super::super::activity_lines(&step, shown, None, "⠋", 80, &theme)
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
