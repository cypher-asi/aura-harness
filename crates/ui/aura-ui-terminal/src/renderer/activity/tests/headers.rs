use super::*;

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
fn collapsed_steps_counts_the_steps() {
    let theme = Theme::default();
    assert_eq!(text(&collapsed_steps(5, &theme)), "    5 steps");
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
