use crate::events::{AgentSummary, MailView, UiCommand};
use crate::{App, Theme};
use crate::{PaneTab, RecordsTab};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, Terminal};

fn render_rows(app: &mut App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(240, 40)).unwrap();
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

/// Left-click the first cell of `text` on screen.
fn click(app: &mut App, rows: &[String], text: &str) {
    let (column, row) = rows
        .iter()
        .enumerate()
        .find_map(|(y, row)| row.find(text).map(|x| (row[..x].chars().count(), y)))
        .unwrap_or_else(|| panic!("{text} not on screen"));
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::NONE,
    });
}

fn mail(id: u64, from: &str, to: &str, text: &str) -> MailView {
    MailView {
        id,
        thread_id: "t1".into(),
        subject: "Tokenizer review".into(),
        from_id: from.to_lowercase(),
        from_name: from.into(),
        to_id: to.to_lowercase(),
        to_name: to.into(),
        reply: id > 1,
        text: text.into(),
        sent_ms: 1_700_000_000_000 + id,
    }
}

#[test]
fn the_inbox_bar_counts_unread_threads() {
    let mut app = app_with_two_threads();
    app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
    let screen = render_rows(&mut app).join("\n");
    assert!(screen.contains("INBOX"), "{screen}");
    assert!(screen.contains("2 NEW │"), "{screen}");
    assert!(screen.contains("⏎ open"), "the focused tab shows its keys");
}

#[test]
fn thread_rows_show_their_dot_arrow_subject_count_and_participants() {
    let mut app = app_with_two_threads();
    app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
    let rows = render_rows(&mut app);
    let thread = rows
        .iter()
        .find(|row| row.contains("Tokenizer review"))
        .unwrap();
    for part in ["● ▸ Tokenizer review (2)", "Lead ↔ Scout"] {
        assert!(thread.contains(part), "{part} in {thread}");
    }
    let other = rows.iter().find(|row| row.contains("Benchmark")).unwrap();
    assert!(
        other.contains("(1)") && other.contains("Lead → Bench"),
        "{other}"
    );
}

#[test]
fn unfolding_a_thread_lists_its_messages_under_it() {
    let mut app = app_with_two_threads();
    app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
    app.handle_key(KeyEvent::from(KeyCode::Right));
    let rows = render_rows(&mut app);
    let screen = rows.join("\n");
    assert!(screen.contains("▾ Tokenizer review (2)"), "{screen}");
    let first = rows
        .iter()
        .find(|row| row.contains("Lead → Scout   Please check"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert!(first.contains("       Lead → Scout"), "indented: {first}");
    let reply = rows
        .iter()
        .find(|row| row.contains("Scout → Lead"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert!(
        reply.contains("↩ Scout → Lead   Found two issues:"),
        "{reply}"
    );
}

#[test]
fn an_empty_inbox_says_so() {
    let mut app = App::new();
    app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
    let screen = render_rows(&mut app).join("\n");
    assert!(
        screen.contains("No messages between agents yet."),
        "{screen}"
    );
    assert!(screen.contains("0 THREADS │"), "{screen}");
}

/// Lead on screen with thread t1 (two messages, the second a reply)
/// and thread t2 (one message to Bench), both unread.
fn app_with_two_threads() -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetAgents(vec![AgentSummary {
        id: "lead".into(),
        name: "Lead".into(),
        ..AgentSummary::default()
    }]));
    app.process_command(UiCommand::SetActiveAgent("lead".into()));
    for message in [
        mail(1, "Lead", "Scout", "Please check the tokenizer"),
        mail(2, "Scout", "Lead", "Found two issues:\n1. spaces"),
        MailView {
            thread_id: "t2".into(),
            subject: "Benchmark run".into(),
            // Older than t1, so t1 lists first and is highlighted.
            sent_ms: 1_600_000_000_000,
            ..mail(3, "Lead", "Bench", "Run the suite")
        },
    ] {
        app.process_command(UiCommand::AddMail(message));
    }
    app
}

#[test]
fn thread_view_folds_older_messages_and_unfolds_the_newest() {
    let mut app = App::new();
    app.process_command(UiCommand::SetAgents(vec![AgentSummary {
        id: "lead".into(),
        name: "Lead".into(),
        ..AgentSummary::default()
    }]));
    app.process_command(UiCommand::SetActiveAgent("lead".into()));
    app.process_command(UiCommand::AddMail(mail(
        1,
        "Lead",
        "Scout",
        "Please check the tokenizer",
    )));
    app.process_command(UiCommand::AddMail(mail(
        2,
        "Scout",
        "Lead",
        "Found two issues:\n1. spaces",
    )));
    app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
    let rows = render_rows(&mut app);
    click(&mut app, &rows, "Tokenizer review");

    let rows = render_rows(&mut app);
    let screen = rows.join("\n");
    assert!(screen.contains("THREAD Tokenizer review"), "{screen}");
    assert!(screen.contains("2 MESSAGES"), "{screen}");
    assert!(
        rows.iter()
            .any(|row| row.contains("▸ Lead → Scout · ")
                && row.contains("Please check the tokenizer")),
        "{screen}"
    );
    assert!(screen.contains("▾ Scout → Lead · "), "{screen}");
    assert!(screen.contains(" · reply"), "{screen}");
    assert!(screen.contains("  Found two issues:"), "{screen}");
    assert!(screen.contains("  1. spaces"), "{screen}");
    assert!(screen.contains("n/p message"), "{screen}");

    // The drawn headers map clicks: the first header unfolds.
    click(&mut app, &rows, "▸ Lead");
    let screen = render_rows(&mut app).join("\n");
    assert!(screen.contains("▾ Lead → Scout"), "{screen}");
    assert!(screen.contains("  Please check the tokenizer"), "{screen}");
}
