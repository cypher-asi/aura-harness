use super::notice::columns;
use super::*;
use crate::events::TodoState;

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
    let mut lines = vec![checklist_header(&hud, &theme, true, None)];
    lines.extend(hud.todos.iter().map(|todo| todo_line(todo, 38, &theme)));
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
    let header = |frame| checklist_header(&hud, &theme, true, frame).spans;
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
