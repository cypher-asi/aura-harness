use super::*;

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
