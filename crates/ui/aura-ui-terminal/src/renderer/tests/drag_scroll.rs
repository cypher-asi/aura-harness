use super::*;

#[test]
fn test_a_scrolled_projects_list_draws_the_rows_it_shows() {
    use crate::app::ProjectListRow;
    use crate::events::{AgentSummary, UiCommand};
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::App::new().with_event_sender(tx);
    let agent = |id: String, name: String, depth: usize| AgentSummary {
        zns_id: format!("0://terminal/{id}"),
        id,
        name,
        depth,
        ..AgentSummary::default()
    };
    let mut agents = Vec::new();
    for project in 0..12 {
        agents.push(agent(format!("p{project}"), format!("Lead{project:02}"), 0));
        for helper in 0..3 {
            agents.push(agent(
                format!("p{project}h{helper}"),
                format!("Helper{project:02}-{helper}"),
                1,
            ));
        }
    }
    app.process_command(UiCommand::SetAgents(agents));
    app.process_command(UiCommand::SetActiveAgent("p0".to_string()));

    let buffer = render_buffer(&mut app, 120);
    let top = row_of(&buffer, "LEAD00");
    let label = |app: &crate::App, row: usize| match app.project_rows()[row] {
        ProjectListRow::Project { root, .. } => Some(app.agents()[root].name.to_uppercase()),
        ProjectListRow::Agent { index } => Some(app.agents()[index].name.clone()),
        _ => None,
    };
    for _ in 0..30 {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 4,
            row: top,
            modifiers: KeyModifiers::empty(),
        });
        let buffer = render_buffer(&mut app, 120);
        let first = app.agents_scroll();
        for offset in 0..3 {
            let y = top + u16::try_from(offset).unwrap();
            if let Some(name) = label(&app, first + offset) {
                assert!(find_cell(&buffer, y, &name).is_some(), "{name} at {y}");
            }
        }
        assert!(row_of(&buffer, "New project") > top);
    }
    assert!(app.agents_scroll() > 0);
}

#[test]
fn test_smooth_scrolling_draws_the_chat_row_below_the_body() {
    use crate::events::UiCommand;

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::App::new().with_event_sender(tx);
    let lines: String = (0..80).map(|n| format!("line {n}\n\n")).collect();
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText(lines));
    render_buffer(&mut app, 120);
    assert_eq!(app.chat_scroll(), None, "only when the front end asks");

    app.set_smooth_scroll(true);
    // Up past the blank rows kept below the last line, to a line of text.
    let up = chat_room(&app) + 21;
    app.scroll_chat_rows(isize::try_from(up).unwrap());
    render_buffer(&mut app, 120);
    let view = app.chat_scroll().expect("the chat is drawn");
    assert_eq!(view.below, up);
    let (cells, _) = app.chat_row_below();
    let next: String = cells.iter().map(ratatui::buffer::Cell::symbol).collect();

    app.scroll_chat_rows(-1);
    let buffer = render_buffer(&mut app, 120);
    let view = app.chat_scroll().expect("the chat is drawn");
    assert_eq!(view.below, up - 1);
    let last = view.body.bottom() - 1;
    let row: String = (view.body.left()..view.body.right())
        .map(|x| buffer[(x, last)].symbol())
        .collect();
    let body = usize::from(view.body.x)..usize::from(view.body.right());
    assert_eq!(row, next[body].to_string(), "the row that scrolls in");
    assert!(row.contains("line"), "{row:?}");
}

/// Press on `first` in the chat panel, hold the pointer below the screen
/// until the chat scrolls `last` into view, then let go. Checks that the
/// rows scrolled in are selected and that the copy holds `first` and
/// everything after it up to `last`, which it returns, a line per row.
fn select_past_the_bottom(app: &mut crate::App, first: &str, last: &str) -> Vec<String> {
    use crossterm::event::{MouseButton, MouseEventKind};

    let on_screen = |buffer: &ratatui::buffer::Buffer, needle| {
        (0..30).find(|&y| find_cell(buffer, y, needle).is_some())
    };
    let buffer = render_buffer(app, 120);
    let top = row_of(&buffer, first);
    let (column, _) = find_cell(&buffer, top, first).unwrap();
    assert!(on_screen(&buffer, last).is_none(), "{last:?} starts below");

    mouse_at(app, MouseEventKind::Down(MouseButton::Left), (column, top));
    let below = (119, 29);
    mouse_at(app, MouseEventKind::Drag(MouseButton::Left), below);
    assert!(app.selection_scrolling(), "held past the bottom");
    let mut buffer = render_buffer(app, 120);
    for _ in 0..40 {
        if on_screen(&buffer, last).is_some() {
            break;
        }
        app.scroll_selection();
        buffer = render_buffer(app, 120);
    }
    assert!(on_screen(&buffer, first).is_none(), "scrolled away");
    let shown = on_screen(&buffer, last).expect("scrolled into view");
    let (at, _) = find_cell(&buffer, shown, last).unwrap();
    assert!(
        buffer[(at, shown)]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED),
        "the rows scrolled into view are selected"
    );
    mouse_at(app, MouseEventKind::Up(MouseButton::Left), below);
    assert!(!app.selection_scrolling());
    let buffer = render_buffer(app, 120);

    let text = app.take_copy_text(&buffer).expect("copied on release");
    let copied: Vec<String> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    assert!(copied[0].contains(first), "{copied:?}");
    assert!(copied.iter().any(|l| l.contains(last)), "{copied:?}");
    copied
}

/// `line 00` to `line 40`, all in `copied`.
fn assert_copied_lines_to_40(copied: &[String]) {
    for n in 0..=40 {
        let line = format!("line {n:02}");
        assert!(
            copied.iter().any(|l| l.contains(&line)),
            "{line}: {copied:?}"
        );
    }
}

#[test]
fn test_a_selection_dragged_past_the_chat_scrolls_it_and_copies_it_all() {
    use crate::events::UiCommand;

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::App::new().with_event_sender(tx);
    let lines: String = (0..60).map(|n| format!("line {n:02}\n\n")).collect();
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText(lines));
    scroll_chat_to_top(&mut app, "line 59");

    let copied = select_past_the_bottom(&mut app, "line 00", "line 40");
    assert_eq!(copied[0], "line 00");
    assert_copied_lines_to_40(&copied);
}

#[test]
fn test_a_selection_dragged_past_the_terminal_scrolls_it() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    submit_message(&mut app, "$seq");
    let output: String = (0..60).map(|n| format!("line {n:02}\n")).collect();
    app.process_command(UiCommand::CommandFinished {
        output,
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Terminal);
    scroll_chat_to_top(&mut app, "line 59");

    let copied = select_past_the_bottom(&mut app, "line 00", "line 40");
    assert_copied_lines_to_40(&copied);
}

#[test]
fn test_a_selection_dragged_past_a_benchmark_scrolls_it() {
    use crate::events::{BenchArm, BenchRun, BenchView, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    app.process_command(UiCommand::BenchList {
        benches: vec![BenchView {
            id: "ab".to_string(),
            name: "Compare #1".to_string(),
            source: "the source".to_string(),
            arms: [BenchArm::default(), BenchArm::default()],
            runs: (0..60)
                .map(|n| BenchRun {
                    name: format!("line {n:02}"),
                    ..BenchRun::default()
                })
                .collect(),
            ..BenchView::default()
        }],
    });
    app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT));
    assert_eq!(app.agents_tab(), crate::app::AgentsTab::Bench);

    // The name is in the header and the list too; the source line is
    // the benchmark's own.
    let copied = select_past_the_bottom(&mut app, "the source", "line 40");
    assert_copied_lines_to_40(&copied);

    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "line 40");
    for _ in 0..3 {
        mouse_at(
            &mut app,
            crossterm::event::MouseEventKind::ScrollUp,
            (60, row),
        );
    }
    assert!(app.selection().is_some(), "the wheel keeps the selection");
}

#[test]
fn test_a_selection_dragged_past_a_record_scrolls_it() {
    use crate::events::{RecordStatus, RecordSummary, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = crate::App::new();
    app.process_command(UiCommand::NewRecord(RecordSummary {
        seq: 7,
        timestamp: "12:00:00".to_string(),
        full_hash: String::new(),
        hash_suffix: String::new(),
        tx_kind: "Prompt".to_string(),
        sender: String::new(),
        message: (0..60).map(|n| format!("line {n:02}\n\n")).collect(),
        action_count: 0,
        effect_status: String::new(),
        status: RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: String::new(),
        agent_id: String::new(),
        ts_ms: 0,
        policy: None,
    }));
    while app.focus() != crate::app::PanelFocus::Records {
        app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.detail_view(), Some(&crate::app::DetailView::Record));

    let copied = select_past_the_bottom(&mut app, "Sequence", "line 40");
    assert_copied_lines_to_40(&copied);
}
