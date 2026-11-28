//! The wheel over a list scrolls its rows and leaves its highlight, and
//! so what is open, alone; a key moves the highlight and brings the list
//! back to it.

use super::*;
use crate::app::{AgentsTab, SidePanelTab};
use crate::events::{AgentDefinitionSummary, ChannelSummary, ProcessKind, ProcessSummary};
use crossterm::event::MouseEventKind;

/// Rows of each list: more than any panel shows on the screen drawn.
const ROWS: usize = 40;

/// Draw `app` on a screen 140 columns by 30 rows, as a front end does.
fn draw(app: &mut App) {
    let backend = ratatui::backend::TestBackend::new(140, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &crate::Theme::cyber()))
        .unwrap();
}

/// Turn the wheel `notches` times over the middle of `area`, down or up,
/// drawing a frame after each notch.
fn wheel(app: &mut App, area: Option<Rect>, down: bool, notches: usize) {
    let area = area.expect("the panel was drawn");
    let kind = if down {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    for _ in 0..notches {
        let (column, row) = (area.x + area.width / 2, area.y + area.height / 2);
        app.handle_mouse(mouse(kind, column, row));
        draw(app);
    }
}

/// What a list shows: its highlight and its first row on screen.
type Read = fn(&App) -> (usize, usize);

/// Over `area`, the wheel scrolls the list `read` reads, down or up,
/// without moving its highlight or opening anything; then `next` moves
/// the highlight and the list scrolls back to show it.
fn check_list(
    app: &mut App,
    (area, read): (fn(&App) -> Option<Rect>, Read),
    rx: &mut mpsc::Receiver<UiEvent>,
    (down, next): (bool, KeyCode),
) {
    draw(app);
    while rx.try_recv().is_ok() {}
    let (selected, scroll) = read(app);
    let on_screen = app.active_agent_id().to_string();
    let channel = app.channel_open();

    wheel(app, area(app), down, 2);
    let (after, scrolled) = read(app);
    assert_eq!(after, selected, "the wheel leaves the highlight alone");
    assert_ne!(scrolled, scroll, "the wheel scrolls the rows");
    assert!(
        if down {
            scrolled > scroll
        } else {
            scrolled < scroll
        },
        "{scroll} -> {scrolled}"
    );
    assert_eq!(app.active_agent_id(), on_screen, "no other agent opens");
    assert_eq!(app.channel_open(), channel, "no channel opens or closes");
    let opened: Vec<UiEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        !opened
            .iter()
            .any(|event| matches!(event, UiEvent::SelectAgent(_) | UiEvent::OpenChannel(_))),
        "{opened:?}"
    );

    app.handle_key(key(next));
    draw(app);
    let (moved, shown) = read(app);
    assert_ne!(moved, selected, "the key moves the highlight");
    let visible = area(app).map_or(1, |area| usize::from(area.height));
    assert!(
        shown <= moved && moved < shown + visible,
        "the highlight {moved} is back in view from row {shown}"
    );
}

/// `app` with the agents panel focused, after `steps` of Left from
/// PROJECTS.
fn agents_tab_after(app: &mut App, steps: usize, tab: AgentsTab) {
    app.focus = PanelFocus::Swarm;
    for _ in 0..steps {
        app.handle_key(key(KeyCode::Left));
    }
    assert_eq!(app.agents_tab(), tab);
}

fn swarm(app: &App) -> Option<Rect> {
    app.areas.swarm
}

fn records_panel(app: &App) -> Option<Rect> {
    app.areas.records
}

fn side_panel(app: &App) -> Option<Rect> {
    app.areas.sandbox
}

#[test]
fn the_wheel_scrolls_projects_without_switching_agents() {
    let (mut app, mut rx) = app_with_agents();
    let agents: Vec<_> = (0..ROWS)
        .map(|n| agent(&format!("a{n:02}"), &format!("Agent {n:02}")))
        .collect();
    app.process_command(UiCommand::SetAgents(agents));
    app.process_command(UiCommand::SetActiveAgent("a00".to_string()));
    app.focus = PanelFocus::Swarm;
    let read: Read = |app| (app.selected_agent(), app.agents_scroll());
    check_list(&mut app, (swarm, read), &mut rx, (true, KeyCode::Down));
}

#[test]
fn the_wheel_scrolls_channels_without_opening_one() {
    let (mut app, mut rx) = app_with_agents();
    let channels = (0..ROWS)
        .map(|n| ChannelSummary {
            id: format!("c{n:02}"),
            slug: format!("chan{n:02}"),
            title: format!("Channel {n:02}"),
            coordinator_id: "aa".to_string(),
            members: vec![agent("aa", "Coordinator")],
            ..ChannelSummary::default()
        })
        .collect();
    app.process_command(UiCommand::SetChannels(channels));
    agents_tab_after(&mut app, 1, AgentsTab::Channels);
    let read: Read = |app| (app.selected_channel(), app.channels_scroll());
    check_list(&mut app, (swarm, read), &mut rx, (true, KeyCode::Down));
}

#[test]
fn the_wheel_scrolls_the_roster_without_moving_its_highlight() {
    let (mut app, mut rx) = app_with_agents();
    let definitions = (0..ROWS)
        .map(|n| AgentDefinitionSummary {
            name: format!("agent-{n:02}"),
            description: "Does things".to_string(),
            ..AgentDefinitionSummary::default()
        })
        .collect();
    app.process_command(UiCommand::SetAgentDefinitions(definitions));
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Roster);
    let read: Read = |app| (app.selected_definition(), app.definitions_scroll());
    check_list(&mut app, (swarm, read), &mut rx, (true, KeyCode::Down));
}

#[test]
fn the_wheel_scrolls_the_archive_without_opening_an_agent() {
    let (mut app, mut rx) = app_with_agents();
    let archived = (0..ROWS)
        .map(|n| crate::events::AgentSummary {
            last_active_ms: Some(1_700_000_000_000),
            ..agent(&format!("z{n:02}"), &format!("Old {n:02}"))
        })
        .collect();
    app.process_command(UiCommand::SetArchivedAgents(archived));
    agents_tab_after(&mut app, 2, AgentsTab::Archive);
    // The highlight's row: day headers take rows of their own.
    let read: Read = |app| {
        let highlight = crate::app::ArchiveRow::Agent {
            index: app.selected_archived(),
        };
        let row = app.archive_rows().iter().position(|row| *row == highlight);
        (row.unwrap_or(usize::MAX), app.archived_scroll())
    };
    check_list(&mut app, (swarm, read), &mut rx, (true, KeyCode::Down));
}

#[test]
fn the_wheel_scrolls_the_kernel_records_without_moving_the_highlight() {
    let (mut app, mut rx) = app_with_agents();
    let records = (0..ROWS as u64)
        .map(super::super::detail_tests::record)
        .collect();
    app.process_command(UiCommand::SetRecords(records));
    app.focus = PanelFocus::Records;
    // The highlight follows the newest record, at the bottom: the wheel
    // goes up from there.
    let read: Read = |app| (app.selected_record(), app.records_scroll());
    check_list(
        &mut app,
        (records_panel, read),
        &mut rx,
        (false, KeyCode::Up),
    );
}

#[test]
fn the_wheel_scrolls_the_sandbox_tree_without_moving_its_highlight() {
    let dir = tempfile::tempdir().unwrap();
    for n in 0..ROWS {
        std::fs::write(dir.path().join(format!("file{n:02}.rs")), "").unwrap();
    }
    let (mut app, mut rx) = app_with_agents();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.focus = PanelFocus::Sandbox;
    let read: Read = |app| (app.sandbox().selected(), app.side.sandbox_scroll);
    check_list(&mut app, (side_panel, read), &mut rx, (true, KeyCode::Down));
}

#[test]
fn the_wheel_scrolls_processes_without_moving_the_highlight() {
    let (mut app, mut rx) = app_with_agents();
    let processes = (0..ROWS)
        .map(|n| ProcessSummary {
            id: format!("cmd:{n}"),
            kind: ProcessKind::Command,
            owner_agent_id: "aa".to_string(),
            label: format!("command {n:02}"),
            ..ProcessSummary::default()
        })
        .collect();
    app.process_command(UiCommand::SetProcesses(processes));
    app.show_side_tab(SidePanelTab::Processes);
    app.focus = PanelFocus::Sandbox;
    let read: Read = |app| (app.side.processes.selected, app.side.processes.scroll);
    check_list(&mut app, (side_panel, read), &mut rx, (true, KeyCode::Down));
}

#[test]
fn the_wheel_over_a_list_that_fits_does_nothing() {
    let (mut app, _rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;
    draw(&mut app);
    let area = app.areas.swarm;
    wheel(&mut app, area, true, 3);
    assert_eq!(app.agents_scroll(), 0);
    assert_eq!(app.selected_agent(), 1, "still on Terminal Agent");
    assert_eq!(app.active_agent_id(), "aa");
}
