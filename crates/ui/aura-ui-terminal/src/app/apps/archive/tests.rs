use super::*;
use crate::app::{AgentsTab, PanelFocus};
use crate::events::UiCommand;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

fn agent(id: &str, depth: usize) -> AgentSummary {
    AgentSummary {
        id: id.into(),
        name: id.to_uppercase(),
        depth,
        ..AgentSummary::default()
    }
}

fn app() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new();
    app.link.event_tx = Some(tx);
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", 0),
        agent("bb", 1),
        agent("cc", 0),
    ]));
    app.process_command(UiCommand::SetActiveAgent("bb".into()));
    app.focus = PanelFocus::Swarm;
    (app, rx)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// The screen at `width` columns, a line per row.
fn screen(app: &mut App, width: u16) -> Vec<String> {
    use ratatui::{backend::TestBackend, Terminal};
    let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
    let theme = crate::Theme::cyber();
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &theme))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

#[test]
fn the_empty_archive_says_so_inside_the_panels_padding() {
    let (mut app, _rx) = app();
    app.show_agents_tab(AgentsTab::Archive);
    assert_eq!(app.agents_tab(), AgentsTab::Archive);
    for width in [160, 200] {
        let rows = screen(&mut app, width);
        let first = rows.iter().position(|row| row.contains("No archived"));
        let first = first.unwrap_or_else(|| panic!("no message:\n{}", rows.join("\n")));
        // The panel's left border, then two columns of padding.
        let border = rows[first].find('│').expect("the panel's border");
        let lines: Vec<&str> = rows[first..]
            .iter()
            .map(|row| {
                row[border + '│'.len_utf8()..]
                    .split('│')
                    .next()
                    .unwrap_or("")
            })
            .take_while(|line| !line.trim().is_empty())
            .collect();
        assert_eq!(lines.len(), 1, "{width}: {lines:?}");
        let line = lines[0];
        assert!(line.starts_with("  "), "{width}: {line:?}");
        assert!(!line.starts_with("   "), "{width}: {line:?}");
        assert!(line.ends_with("  "), "{width}: {line:?}");
        assert_eq!(line.trim(), "No archived agents.");
    }
}

#[test]
fn archived_agents_keep_their_indent_under_the_one_they_ran_for() {
    let (mut app, _rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", 0),
        agent("ee", 1),
    ]));
    app.show_agents_tab(AgentsTab::Archive);
    app.handle_key(key(KeyCode::Char(' ')));
    let rows = screen(&mut app, 100);
    let column = |name: &str| {
        let row = rows.iter().find(|row| row.contains(name));
        let row = row.unwrap_or_else(|| panic!("no {name}:\n{}", rows.join("\n")));
        row[..row.find(name).unwrap_or(0)].chars().count()
    };
    assert_eq!(column("EE"), column("DD") + 2);
}

#[test]
fn delete_archives_the_highlighted_agent() {
    let (mut app, mut rx) = app();
    app.handle_key(key(KeyCode::Delete));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveAgent(id)) if id == "bb"));
}

#[test]
fn shift_delete_removes_the_project_only_once_confirmed() {
    let (mut app, mut rx) = app();
    app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::SHIFT));
    assert!(rx.try_recv().is_err());
    assert!(app
        .notification()
        .is_some_and(|(text, _)| text.contains("AA")));
    app.handle_key(key(KeyCode::Char('n')));
    assert!(rx.try_recv().is_err());

    // It names the project, by the key of the one `bb` is in.
    app.handle_command("/remove-project");
    app.handle_key(key(KeyCode::Char('y')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::RemoveProject(id)) if id == "aa"));
}

#[test]
fn enter_in_the_archive_opens_the_agent_and_stays_there() {
    let (mut app, mut rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", 0),
        agent("ee", 1),
    ]));
    app.show_agents_tab(AgentsTab::Archive);
    assert_eq!(app.agents_tab(), AgentsTab::Archive);
    assert_eq!(app.archived().len(), 2);
    assert!(
        matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "dd"),
        "the highlighted agent opens on coming to the Archive"
    );

    app.handle_key(key(KeyCode::Char(' ')));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_archived(), 1);
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "ee"));
    assert!(rx.try_recv().is_err(), "nothing is restored");
    assert_eq!(app.agents_tab(), AgentsTab::Archive);
    assert_eq!(app.active_agent_id(), "ee", "on screen at once");
}

#[test]
fn each_agent_app_shows_what_it_was_left_on() {
    let (mut app, mut rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", 0),
        agent("ee", 0),
    ]));
    app.show_agents_tab(AgentsTab::Archive);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.active_agent_id(), "ee");

    app.show_agents_tab(AgentsTab::Projects);
    assert_eq!(app.active_agent_id(), "bb", "PROJECTS was left on bb");
    app.show_agents_tab(AgentsTab::Archive);
    assert_eq!(app.active_agent_id(), "ee", "ARCHIVE was left on ee");
    app.show_agents_tab(AgentsTab::Projects);
    assert_eq!(app.active_agent_id(), "bb");
    let asked: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            UiEvent::SelectAgent(id) | UiEvent::FocusAgent(id) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(asked, ["dd", "ee", "bb", "ee", "bb"]);
}

#[test]
fn a_listed_agent_put_on_screen_brings_projects_to_the_front() {
    let (mut app, _rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![agent("dd", 0)]));
    app.show_agents_tab(AgentsTab::Archive);
    app.process_command(UiCommand::SetActiveAgent("dd".into()));
    assert_eq!(app.agents_tab(), AgentsTab::Archive);

    // The session going to a listed agent, as a notification does.
    app.process_command(UiCommand::SetActiveAgent("cc".into()));
    assert_eq!(app.active_agent_id(), "cc");
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
}

fn click(app: &mut App, (column, row): (u16, u16)) {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

/// The cell of the first row whose text contains `name`.
fn cell_of(rows: &[String], name: &str) -> (u16, u16) {
    let row = rows
        .iter()
        .position(|row| row.contains(name))
        .unwrap_or_else(|| panic!("no {name}:\n{}", rows.join("\n")));
    let line = &rows[row];
    let column = line[..line.find(name).unwrap_or(0)].chars().count();
    (u16::try_from(column).unwrap(), u16::try_from(row).unwrap())
}

#[test]
fn clicking_an_archived_agent_opens_it_and_stays_on_the_archive() {
    let (mut app, mut rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", 0),
        agent("ee", 1),
    ]));
    app.show_agents_tab(AgentsTab::Archive);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "dd"));
    app.handle_key(key(KeyCode::Char(' ')));
    let rows = screen(&mut app, 100);
    click(&mut app, cell_of(&rows, "EE"));
    assert_eq!(app.selected_archived(), 1);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "ee"));
    assert!(rx.try_recv().is_err(), "nothing is restored");
    assert_eq!(app.agents_tab(), AgentsTab::Archive);

    // A day header opens nothing.
    let rows = screen(&mut app, 100);
    click(&mut app, cell_of(&rows, "Earlier"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn unarchive_restores_only_an_archived_agent_on_screen() {
    let (mut app, mut rx) = app();
    app.handle_command("/unarchive");
    assert!(rx.try_recv().is_err(), "bb is listed");
    assert!(app
        .notification()
        .is_some_and(|(text, _)| text == "This agent is not archived"));

    let mut ee = agent("ee", 0);
    ee.is_active = true;
    app.process_command(UiCommand::SetArchivedAgents(vec![ee]));
    app.process_command(UiCommand::SetActiveAgent("ee".into()));
    app.handle_command("/unarchive");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::RestoreAgent(id)) if id == "ee"));
}

/// Noon local time on `day` of January 2024.
fn noon(day: u32) -> DateTime<Local> {
    use chrono::TimeZone;
    Local
        .with_ymd_and_hms(2024, 1, day, 12, 0, 0)
        .single()
        .unwrap()
}

fn ms_of(time: DateTime<Local>) -> Option<u64> {
    u64::try_from(time.timestamp_millis()).ok()
}

#[test]
fn archive_day_names_today_yesterday_a_date_or_earlier() {
    let now = noon(10);
    let cases = [
        (ms_of(noon(10)), "Today"),
        (ms_of(noon(9)), "Yesterday"),
        (ms_of(noon(3)), "2024-01-03"),
        (None, "Earlier"),
    ];
    for (ms, want) in cases {
        assert_eq!(archive_day(ms, now), want, "{ms:?}");
    }
}

fn archived_at(id: &str, depth: usize, at: DateTime<Local>) -> AgentSummary {
    AgentSummary {
        archived_at_ms: ms_of(at),
        ..agent(id, depth)
    }
}

#[test]
fn archive_rows_put_a_header_before_each_new_day() {
    let (mut app, _rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        archived_at("dd", 0, noon(10)),
        // Under dd: it stays with its head whatever its own time.
        archived_at("ee", 1, noon(3)),
        archived_at("ff", 0, noon(10)),
        archived_at("gg", 0, noon(9)),
        agent("hh", 0),
    ]));
    app.toggle_archived(0);
    let agent_row = |index| ArchiveRow::Agent { index };
    assert_eq!(
        app.archive_rows_at(noon(10)),
        vec![
            ArchiveRow::Day("Today".into()),
            agent_row(0),
            agent_row(1),
            agent_row(2),
            ArchiveRow::Day("Yesterday".into()),
            agent_row(3),
            ArchiveRow::Day("Earlier".into()),
            agent_row(4),
        ]
    );

    app.process_command(UiCommand::SetArchivedAgents(Vec::new()));
    assert!(app.archive_rows().is_empty());
}

#[test]
fn the_archive_draws_its_day_headers() {
    let (mut app, _rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        archived_at("dd", 0, Local::now()),
        agent("ee", 0),
    ]));
    app.show_agents_tab(AgentsTab::Archive);
    let rows = screen(&mut app, 100);
    let row_of = |name: &str| cell_of(&rows, name).1;
    assert!(row_of("Today") < row_of("DD"));
    assert!(row_of("DD") < row_of("Earlier"));
    assert!(row_of("Earlier") < row_of("EE"));
}

#[test]
fn the_highlight_follows_its_agent_when_a_newer_one_comes_in() {
    let (mut app, _rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", 0),
        agent("ee", 0),
    ]));
    app.show_agents_tab(AgentsTab::Archive);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_archived(), 1);

    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("zz", 0),
        agent("dd", 0),
        agent("ee", 0),
    ]));
    assert_eq!(app.archived()[app.selected_archived()].id, "ee");
}

#[test]
fn the_first_archive_highlights_the_agent_on_screen() {
    let (mut app, _rx) = app();
    let mut ee = agent("ee", 0);
    ee.is_active = true;
    app.process_command(UiCommand::SetArchivedAgents(vec![agent("dd", 0), ee]));
    assert_eq!(app.selected_archived(), 1);
}

/// An Archive of `dd` over `ee` over `ff`, then `gg` alone, then `hh`
/// over `ii`, shown in the Archive tab with `dd` on screen.
fn nested_archive() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, mut rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", 0),
        agent("ee", 1),
        agent("ff", 2),
        agent("gg", 0),
        agent("hh", 0),
        agent("ii", 1),
    ]));
    app.show_agents_tab(AgentsTab::Archive);
    assert_eq!(app.agents_tab(), AgentsTab::Archive);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "dd"));
    (app, rx)
}

/// The archived agents drawn, by id.
fn shown_ids(app: &App) -> Vec<&str> {
    app.archive_rows()
        .into_iter()
        .filter_map(|row| match row {
            ArchiveRow::Agent { index } => Some(app.archived()[index].id.as_str()),
            ArchiveRow::Day(_) => None,
        })
        .collect()
}

#[test]
fn the_archive_keeps_what_was_opened_in_it() {
    let (mut app, _rx) = nested_archive();
    assert_eq!(shown_ids(&app), ["dd", "gg", "hh"]);
    assert!(app.archived_folded(0) && app.archived_folded(4));
    assert!(!app.archived_folded(3), "gg has nothing to fold");

    // Folded at first; opened one level at a time.
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(shown_ids(&app), ["dd", "ee", "gg", "hh"]);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(shown_ids(&app), ["dd", "ee", "ff", "gg", "hh"]);
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(shown_ids(&app).len(), 5, "other tabs leave it alone");

    // Coming back finds them as they were left, highlight included.
    let selected = app.selected_archived();
    app.show_agents_tab(AgentsTab::Projects);
    app.show_agents_tab(AgentsTab::Archive);
    assert_eq!(shown_ids(&app), ["dd", "ee", "ff", "gg", "hh"]);
    assert_eq!(app.selected_archived(), selected);
}

#[test]
fn the_highlight_moves_over_drawn_rows_only() {
    let (mut app, _rx) = nested_archive();
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.archived()[app.selected_archived()].id, "gg");
    app.handle_key(key(KeyCode::End));
    assert_eq!(app.archived()[app.selected_archived()].id, "hh");
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.selected_archived(), 0);

    // Space on a subagent with nothing under it folds its parent and
    // moves the highlight onto it.
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Char(' ')));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.archived()[app.selected_archived()].id, "ii");
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(app.archived()[app.selected_archived()].id, "hh");
    assert!(app.archived_folded(4));
}

#[test]
fn a_hidden_agent_on_screen_lights_the_folded_agent_over_it() {
    let (mut app, _rx) = app();
    let mut ff = agent("ff", 2);
    ff.is_active = true;
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", 0),
        agent("ee", 1),
        ff,
    ]));
    app.process_command(UiCommand::SetActiveAgent("ff".into()));
    app.show_agents_tab(AgentsTab::Archive);
    assert_eq!(
        app.active_agent_id(),
        "ff",
        "the Archive keeps it on screen"
    );
    assert_eq!(app.shown_active_archived(), Some(0));
    assert_eq!(app.selected_archived(), 0);
    app.toggle_archived(0);
    assert_eq!(app.shown_active_archived(), Some(1));
}

#[test]
fn the_marker_folds_and_the_rest_of_the_row_opens_the_chat() {
    let (mut app, mut rx) = nested_archive();
    let rows = screen(&mut app, 100);
    assert!(rows
        .iter()
        .any(|row| row.contains("▸") && row.contains("DD")));
    assert!(!rows.iter().any(|row| row.contains("EE")));
    click(&mut app, cell_of(&rows, "▸"));
    assert!(rx.try_recv().is_err(), "the marker opens no chat");
    let rows = screen(&mut app, 100);
    assert!(rows
        .iter()
        .any(|row| row.contains("▾") && row.contains("DD")));
    assert!(rows.iter().any(|row| row.contains("EE")));

    click(&mut app, cell_of(&rows, "EE"));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "ee"));
}
