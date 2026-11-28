use super::*;
use ratatui::style::Modifier;

fn todo(state: TodoState) -> TodoView {
    TodoView {
        content: "Fix the redirect".to_string(),
        state,
        assignee: Some("FEN6".to_string()),
        task: None,
    }
}

fn agent(working: Option<bool>, secs: Option<u64>) -> TodoAgent {
    TodoAgent {
        label: "FEN6".to_string(),
        working,
        running_for: secs.map(Duration::from_secs),
    }
}

fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| &*s.content).collect())
        .collect()
}

#[test]
fn an_item_takes_two_rows_its_text_then_its_agent_and_status() {
    let theme = Theme::default();
    let busy = agent(Some(true), Some(250));
    let lines = item_lines(&todo(TodoState::InProgress), Some(&busy), 60, &theme);
    assert_eq!(texts(&lines), ["◐ Fix the redirect", "  FEN6 · working 4m"]);
    let idle = agent(Some(false), None);
    let lines = item_lines(&todo(TodoState::Pending), Some(&idle), 60, &theme);
    assert_eq!(texts(&lines), ["☐ Fix the redirect", "  FEN6 · idle"]);
    let unlisted = agent(None, None);
    let lines = item_lines(&todo(TodoState::Pending), Some(&unlisted), 60, &theme);
    assert_eq!(texts(&lines)[1], "  FEN6");
}

#[test]
fn an_item_given_to_no_agent_says_its_state() {
    let theme = Theme::default();
    let doing = item_lines(&todo(TodoState::InProgress), None, 60, &theme);
    assert_eq!(texts(&doing)[1], "  In progress");
    let waiting = item_lines(&todo(TodoState::Pending), None, 60, &theme);
    assert_eq!(texts(&waiting)[1], "  Pending");
}

#[test]
fn nothing_is_bold_and_only_the_marker_has_the_state_color() {
    let theme = Theme::default();
    let colors = &theme.colors;
    let busy = agent(Some(true), Some(5));
    for state in [TodoState::Pending, TodoState::InProgress] {
        let [text, detail] = item_lines(&todo(state), Some(&busy), 60, &theme);
        let mut spans = text.spans.iter().chain(&detail.spans);
        assert!(spans.all(|s| !s.style.add_modifier.contains(Modifier::BOLD)));
        assert_eq!(text.spans[0].style.fg, Some(marker_color(state, &theme)));
        assert_eq!(text.spans[1].style.fg, Some(colors.foreground), "{state:?}");
        assert_eq!(detail.spans[1].style.fg, Some(colors.muted));
    }
    let [dropped, _] = item_lines(&todo(TodoState::Cancelled), None, 60, &theme);
    assert_eq!(dropped.spans[1].style, name_style(Tone::Settled, &theme));
}

#[test]
fn the_count_is_not_bold_either() {
    let theme = Theme::default();
    let mut app = App::new();
    app.process_command(crate::events::UiCommand::SetTodos(vec![todo(
        TodoState::InProgress,
    )]));
    for (working, frame) in [(false, None), (true, None), (true, Some(3))] {
        let spans = count_spans(&app, &theme, working, frame);
        assert!(
            spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::BOLD)),
            "{spans:?}"
        );
    }
}

#[test]
fn a_narrow_drawer_cuts_each_row_on_its_own() {
    let theme = Theme::default();
    let busy = agent(Some(true), Some(250));
    let lines = item_lines(&todo(TodoState::InProgress), Some(&busy), 12, &theme);
    assert_eq!(texts(&lines), ["◐ Fix the r…", "  FEN6 · wo…"]);
}

#[test]
fn a_closed_item_says_how_it_ended() {
    let theme = Theme::default();
    let by = agent(Some(false), None);
    let done = item_lines(&todo(TodoState::Completed), Some(&by), 60, &theme);
    assert_eq!(texts(&done), ["✓ Fix the redirect", "  FEN6 · done"]);
    let dropped = item_lines(&todo(TodoState::Cancelled), None, 60, &theme);
    assert_eq!(texts(&dropped), ["⊘ Fix the redirect", "  Cancelled"]);
}

#[test]
fn durations_read_short() {
    assert_eq!(short_duration(Duration::from_secs(12)), "12s");
    assert_eq!(short_duration(Duration::from_secs(250)), "4m");
    assert_eq!(short_duration(Duration::from_secs(3_900)), "1h 5m");
}

#[test]
fn fitting_keeps_the_item_in_progress_and_the_history_row() {
    let pending = todo(TodoState::Pending);
    let current = todo(TodoState::InProgress);
    let mut entries: Vec<Entry<'_>> = vec![Entry::Open(&pending); 6];
    entries[3] = Entry::Open(&current);
    entries.push(Entry::History(false));
    // Six items of two rows, and the history row.
    assert_eq!(fit(&entries, 13, None), ((0..7).collect(), false));
    assert_eq!(
        fit(&entries, 6, None),
        (vec![3, 4, 6], true),
        "from the current item, with the history row"
    );
    assert_eq!(
        fit(&entries, 5, None),
        (vec![3, 6], true),
        "the history row takes the place of an item"
    );
    assert_eq!(
        fit(&entries, 2, None),
        (vec![3], true),
        "the current item, cut short"
    );

    let mut late: Vec<Entry<'_>> = vec![Entry::Open(&pending); 6];
    late[5] = Entry::Open(&current);
    late.push(Entry::History(false));
    assert_eq!(
        fit(&late, 6, None),
        (vec![4, 5, 6], true),
        "rows left at the end go to the items before"
    );
}

#[test]
fn an_open_history_fits_from_where_the_wheel_left_it() {
    let open = todo(TodoState::Pending);
    let done = todo(TodoState::Completed);
    // Two open items, the history row at 2, then five closed items.
    let mut entries: Vec<Entry<'_>> = vec![Entry::Open(&open); 2];
    entries.push(Entry::History(true));
    entries.extend([Entry::Closed(&done); 5]);
    // Seven rows: six for entries, one for `+N more`.
    assert_eq!(fit(&entries, 7, Some(2)), (vec![2, 3, 4], true));
    assert_eq!(fit(&entries, 7, Some(4)), (vec![4, 5, 6], true));
    assert_eq!(
        fit(&entries, 7, Some(40)),
        (vec![5, 6, 7], true),
        "past the end, the last page"
    );
    assert_eq!(fit(&entries, 30, Some(4)), ((0..8).collect(), false));
    assert_eq!(fit(&entries, 2, Some(3)), (vec![3], true), "one, cut short");
}
