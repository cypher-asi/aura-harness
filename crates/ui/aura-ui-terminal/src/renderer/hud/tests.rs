
use super::*;

#[test]
fn plan_mode_status_says_how_to_leave() {
    let theme = Theme::default();
    let hud = crate::app::hud::Hud {
        mode: Some(PermissionMode::Plan),
        ..crate::app::hud::Hud::default()
    };
    let parts = status_parts(&hud, FooterModel::Model { window: None }, &theme);
    assert_eq!(parts[0].0, "plan mode (/plan to leave)");
}

#[test]
fn non_agent_modes_are_drawn_in_the_accent_color() {
    let theme = Theme::default();
    for mode in [
        PermissionMode::Plan,
        PermissionMode::ReadOnly,
        PermissionMode::Debug,
    ] {
        let hud = crate::app::hud::Hud {
            mode: Some(mode),
            ..crate::app::hud::Hud::default()
        };
        let parts = status_parts(&hud, FooterModel::Model { window: None }, &theme);
        assert_eq!(parts[0].1, theme.colors.success, "{mode:?}");
    }
}

#[test]
fn agent_mode_is_left_out_of_the_status() {
    let theme = Theme::default();
    let hud = crate::app::hud::Hud {
        mode: Some(PermissionMode::Agent),
        ..crate::app::hud::Hud::default()
    };
    assert!(status_parts(&hud, FooterModel::Model { window: None }, &theme).is_empty());
}

#[test]
fn the_status_marks_a_partly_priced_total_and_drops_an_unpriced_one() {
    use crate::app::hud::SessionStats;
    let theme = Theme::default();
    let cost_part = |stats: SessionStats| {
        let hud = Hud {
            stats,
            ..Hud::default()
        };
        status_parts(&hud, FooterModel::Gate, &theme)
            .into_iter()
            .map(|(text, _)| text)
            .find(|text| text.contains('$'))
    };
    let stats = |priced, unpriced_calls| SessionStats {
        calls: 3,
        cost_usd: 0.25,
        priced,
        unpriced_calls,
        ..SessionStats::default()
    };
    assert_eq!(cost_part(stats(true, 0)).as_deref(), Some("$0.250"));
    assert_eq!(cost_part(stats(true, 2)).as_deref(), Some("$0.250+?"));
    assert_eq!(cost_part(stats(false, 3)), None);
}

#[test]
fn checklist_boxes_each_state() {
    let theme = Theme::default();
    let todo = |content: &str, state| TodoView {
        content: content.to_string(),
        state,
        assignee: None,
        task: None,
    };
    let hud = Hud {
        todos: vec![
            todo("read", TodoState::Completed),
            todo("fix", TodoState::InProgress),
            todo("test", TodoState::Pending),
            todo("drop", TodoState::Cancelled),
        ],
        ..Hud::default()
    };
    let lines = todo_checklist(&hud, 40, &theme, None, false).lines;
    let text = |line: &Line<'_>| -> String { line.spans.iter().map(|s| &*s.content).collect() };
    let rows: Vec<String> = lines.iter().map(text).collect();
    assert_eq!(
        rows,
        [
            "⬢ Working on 3 to-dos",
            "☒ read",
            "▣ fix",
            "☐ test",
            "☐ drop"
        ]
    );
    let item = |row: usize| lines[row].spans[1].style;
    assert_eq!(item(1).fg, Some(theme.colors.faint));
    assert_eq!(item(2).fg, Some(theme.colors.foreground));
    assert!(item(2).add_modifier.contains(Modifier::BOLD));
    assert_eq!(item(3).fg, Some(theme.colors.muted));
    assert!(item(4).add_modifier.contains(Modifier::CROSSED_OUT));
}

#[test]
fn checklist_working_label_shimmers_when_animated() {
    let theme = Theme::default();
    let hud = Hud {
        todos: vec![TodoView {
            content: "fix".to_string(),
            state: TodoState::InProgress,
            assignee: None,
            task: None,
        }],
        ..Hud::default()
    };
    let header = |frame| {
        let lines = todo_checklist(&hud, 40, &theme, frame, false).lines;
        lines[0].spans.clone()
    };
    let text = |spans: &[Span<'_>]| -> String { spans.iter().map(|s| &*s.content).collect() };
    // Mid-pass, the band lights part of `Working` and leaves the rest.
    let lit = header(Some(4));
    assert_eq!(text(&lit), "⬢ Working on 1 to-do");
    assert!(lit.len() > 3, "band splits the label: {lit:?}");
    assert!(lit[1..lit.len() - 1]
        .iter()
        .all(|s| s.style.add_modifier.contains(Modifier::BOLD)));
    // Without animation the label is one steady span.
    let steady = header(None);
    assert_eq!(steady.len(), 3);
    assert_eq!(steady[1].style.fg, Some(theme.colors.foreground));
}

fn text_of(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| &*s.content).collect()
}

fn pending(content: &str) -> TodoView {
    TodoView {
        content: content.to_string(),
        state: TodoState::Pending,
        assignee: None,
        task: None,
    }
}

#[test]
fn checklist_counts_what_it_leaves_out() {
    let theme = Theme::default();
    let hud = Hud {
        todos: (0..MAX_TODO_ROWS + 2)
            .map(|i| pending(&format!("step {i}")))
            .collect(),
        ..Hud::default()
    };
    let checklist = todo_checklist(&hud, 40, &theme, None, false);
    assert_eq!(checklist.lines.len(), MAX_TODO_ROWS + 2);
    assert!(checklist.has_footer);
    let last = text_of(&checklist.lines[MAX_TODO_ROWS + 1]);
    assert_eq!(last.trim(), "2 more");
}

#[test]
fn short_checklist_cuts_long_items_to_one_row() {
    let theme = Theme::default();
    let hud = Hud {
        todos: vec![pending("parse every config file the user has")],
        ..Hud::default()
    };
    let checklist = todo_checklist(&hud, 20, &theme, None, false);
    let rows: Vec<String> = checklist.lines.iter().map(text_of).collect();
    assert_eq!(rows[1..], ["☐ parse every confi…"]);
    assert!(!checklist.has_footer);
}

#[test]
fn expanded_checklist_lists_every_item_in_full() {
    let theme = Theme::default();
    let mut todos: Vec<TodoView> = (0..MAX_TODO_ROWS + 2)
        .map(|i| pending(&format!("step {i}")))
        .collect();
    todos[0] = TodoView {
        assignee: Some("bob".to_string()),
        ..pending("parse the config files and tests too")
    };
    let hud = Hud {
        todos,
        ..Hud::default()
    };
    let checklist = todo_checklist(&hud, 20, &theme, None, true);
    let rows: Vec<String> = checklist.lines.iter().map(text_of).collect();
    // The long item wraps at words, later rows under its text, and its
    // agent ends the last row.
    assert_eq!(
        rows[1..4],
        ["☐ parse the config", "  files and tests", "  too  → bob"]
    );
    assert_eq!(rows.len(), 1 + 3 + MAX_TODO_ROWS + 1 + 1);
    assert_eq!(
        rows[rows.len() - 2],
        format!("☐ step {}", MAX_TODO_ROWS + 1)
    );
    assert_eq!(rows[rows.len() - 1].trim(), "show less");
    assert!(checklist.has_footer);
}

#[test]
fn expanded_checklist_breaks_a_word_too_long_for_a_row() {
    let theme = Theme::default();
    let hud = Hud {
        todos: vec![pending("abcdefghijkl")],
        ..Hud::default()
    };
    let checklist = todo_checklist(&hud, 7, &theme, None, true);
    let rows: Vec<String> = checklist.lines.iter().map(text_of).collect();
    assert_eq!(rows[1..4], ["☐ abcde", "  fghij", "  kl"]);
}

#[test]
fn columns_slice_by_width_and_pad_cut_wide_characters() {
    assert_eq!(columns("hello world", 6, 5), "world");
    assert_eq!(columns("hello world", 0, 5), "hello");
    assert_eq!(columns("a漢字b", 2, 3), " 字");
    assert_eq!(columns("a漢字b", 0, 2), "a ");
}

#[test]
fn truncate_marks_the_cut() {
    assert_eq!(truncate("hello", 5), "hello");
    assert_eq!(truncate("hello world", 6), "hello…");
}
