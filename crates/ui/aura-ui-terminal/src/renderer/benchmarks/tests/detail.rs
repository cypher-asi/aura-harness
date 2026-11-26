use super::*;

#[test]
fn a_partly_priced_session_shows_no_ratio_and_marks_its_cost() {
    let theme = Theme::default();
    let mut view = view();
    if let Some(baseline) = view.baseline.as_mut() {
        baseline.unpriced_calls = 3;
    }
    let shown = text(&detail_lines(&view, Some(0), &theme, 120, None));
    assert!(!shown.contains("vs session"), "{shown}");
    assert!(
        shown.contains("session: 12 calls, $0.20+? on opus-4-7"),
        "{shown}"
    );
    if let Some(baseline) = view.baseline.as_mut() {
        baseline.priced = false;
        baseline.cost_usd = 0.0;
        baseline.unpriced_calls = 12;
    }
    let shown = text(&detail_lines(&view, Some(0), &theme, 120, None));
    assert!(!shown.contains("vs session"), "{shown}");
    assert!(
        shown.contains("session: 12 calls, no price on opus-4-7"),
        "{shown}"
    );
}

#[test]
fn the_detail_shows_cost_quality_and_each_prompt() {
    let theme = Theme::default();
    let shown = text(&detail_lines(&view(), Some(0), &theme, 120, None));
    let row = |label: &str| {
        shown
            .lines()
            .find(|l| l.starts_with(&format!("{label} ")))
            .unwrap_or_else(|| panic!("no {label} row in\n{shown}"))
            .to_string()
    };
    assert!(
        row("").contains("A gate") && row("").contains("B opus-4-7"),
        "{shown}"
    );
    for (label, a, b) in [
        ("cost", "$0.12", "$0.80"),
        ("cost/run", "$0.06", "$0.40"),
        ("$/fix", "$0.12", "$0.80"),
    ] {
        let line = row(label);
        assert!(
            line.contains(a) && line.contains("-85%") && line.trim_end().ends_with(b),
            "{line}"
        );
    }
    assert!(row("vs session").contains("0.30×"), "{shown}");
    assert!(row("vs session").contains("2.00×"), "{shown}");
    assert!(row("vs yours +/=/-").contains("1/1/0"), "{shown}");
    assert!(shown.contains("$0.03 2✓"), "{shown}");
    assert!(shown.contains("$0.03 1✗"), "{shown}");
    assert!(
        shown.contains("a-1 b-1  B  A= B+  B also fixed the docs"),
        "{shown}"
    );
    assert!(
        shown.contains("session: 12 calls, $0.20 on opus-4-7"),
        "{shown}"
    );
    assert!(shown.contains("›  a-1  sonnet-4-6"), "{shown}");
    assert!(
        !shown.lines().any(|l| l.starts_with("moved up")),
        "no summary row when nothing moved up: {shown}"
    );
}

#[test]
fn a_run_shows_what_drove_its_difficulty() {
    let theme = Theme::default();
    let mut view = view();
    let header = |view: &BenchView| {
        let shown = text(&detail_lines(view, None, &theme, 160, None));
        let header = shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .nth(1)
            .unwrap_or_default()
            .to_string();
        (shown, header)
    };
    let (_, plain) = header(&view);
    assert!(
        !plain.split_whitespace().any(|w| w == "why"),
        "no column for runs whose classifier named the difficulty: {plain}"
    );
    view.runs[0].difficulty = Some("hard".to_string());
    view.runs[0].drivers = Some("many files, loose spec, deep exploring".to_string());
    let (shown, header) = header(&view);
    let heads: Vec<&str> = header.split_whitespace().collect();
    assert!(
        heads.windows(2).any(|w| w == ["difficulty", "why"]),
        "{shown}"
    );
    let run = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .find(|l| l.split_whitespace().next() == Some("a-1"))
        .unwrap_or_default();
    assert!(
        run.contains("hard") && run.contains("many files, loose spec, deep exploring"),
        "{shown}"
    );
}

#[test]
fn a_run_shows_where_it_moved_up_and_how_it_was_classified() {
    let shown = classified_shown();
    let moved = run_line(&shown, "a-1");
    assert!(moved.contains("sonnet-4-6"), "{shown}");
    let words: Vec<&str> = moved.split_whitespace().collect();
    assert!(
        words.windows(2).any(|w| w == ["multi-file", "hard"]),
        "{shown}"
    );
    assert!(moved.contains("to opus-4-7"), "{shown}");
    assert!(moved.contains("yes, $0.26 in subtasks"), "{shown}");
}

#[test]
fn the_runs_header_names_the_class_and_the_difficulty() {
    let shown = classified_shown();
    let header = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .nth(1)
        .unwrap_or_default();
    let heads: Vec<&str> = header.split_whitespace().collect();
    assert!(
        heads.windows(2).any(|w| w == ["classified", "difficulty"]),
        "{shown}"
    );
    assert!(shown.contains("classified"), "{shown}");
}

#[test]
fn a_run_that_stayed_shows_its_class_and_neither_moved_up_nor_led() {
    let shown = classified_shown();
    let stayed = run_line(&shown, "b-1");
    assert!(stayed.contains("small change"), "{shown}");
    assert_eq!(
        stayed.split_whitespace().filter(|w| *w == "no").count(),
        2,
        "neither moved up nor led: {shown}"
    );
}

#[test]
fn the_summary_counts_runs_moved_up_and_nudged() {
    let shown = classified_shown();
    assert!(line_of(&shown, "moved up").contains("1/"), "{shown}");
    assert!(line_of(&shown, "nudged").contains("1/"), "{shown}");
}

#[test]
fn a_move_that_only_raised_the_effort_shows_as_moved_up() {
    let theme = Theme::default();
    let mut view = view();
    view.runs[0].model = "claude/claude-opus-5-5".to_string();
    view.runs[0].escalated_to = Some("claude/claude-opus-5-5".to_string());
    view.runs[0].escalated_effort = Some("max".to_string());
    let shown = text(&detail_lines(&view, None, &theme, 160, None));
    let moved = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .find(|l| l.split_whitespace().next() == Some("a-1"))
        .unwrap_or_else(|| panic!("no a-1 run in\n{shown}"));
    assert!(moved.contains("to opus-5-5 max"), "{shown}");
}

#[test]
fn moved_up_and_led_show_only_when_a_run_did_either() {
    let theme = Theme::default();
    let mut view = view();
    view.runs[0].category = Some("small_change".to_string());
    view.runs[0].difficulty = Some("medium".to_string());
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let header = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .nth(1)
        .unwrap_or_default();
    assert!(header.contains("difficulty"), "{shown}");
    assert!(
        !header.contains("moved up") && !header.contains("led"),
        "{header}"
    );
    let run = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .find(|l| l.split_whitespace().next() == Some("a-1"))
        .unwrap_or_default();
    assert!(run.contains("small change  medium"), "{shown}");
    assert!(!run.split_whitespace().any(|w| w == "no"), "{run}");
}
