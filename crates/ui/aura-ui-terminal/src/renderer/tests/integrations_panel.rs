//! The Integrations panel: its plug just before BENCH's wrench, and its
//! connected and not-connected rows.

use super::*;
use crate::app::{AgentsTab, HoverTarget};
use crate::events::{IntegrationFieldView, IntegrationState, IntegrationView, UiCommand};
use crossterm::event::{MouseButton, MouseEventKind};

fn views() -> Vec<IntegrationView> {
    vec![
        IntegrationView {
            id: "github".to_string(),
            name: "GitHub".to_string(),
            icon: "⑂".to_string(),
            description: "Repositories, pull requests and pushes".to_string(),
            provides: vec!["pull requests on the REPO tab".to_string()],
            state: IntegrationState::Connected,
            status: "gh login".to_string(),
            takes_key: Some("Token".to_string()),
            login: Some("gh auth login".to_string()),
            ..IntegrationView::default()
        },
        IntegrationView {
            id: "boxd".to_string(),
            name: "boxd".to_string(),
            icon: "☁".to_string(),
            description: "Cloud machines for cloud agents".to_string(),
            state: IntegrationState::NotConnected,
            status: "add an api key".to_string(),
            takes_key: Some("API key".to_string()),
            config: vec![IntegrationFieldView {
                key: "console_url".to_string(),
                label: "Console URL".to_string(),
                value: None,
            }],
            ..IntegrationView::default()
        },
    ]
}

fn plug_cell(buffer: &ratatui::buffer::Buffer) -> (u16, u16) {
    let row = row_of(buffer, "🔌");
    let (column, _) = find_cell(buffer, row, "🔌").unwrap();
    (column, row)
}

/// The app with the plug clicked, then the list sent.
fn integrations_app() -> crate::App {
    let mut app = crate::App::new();
    let buffer = render_buffer(&mut app, 140);
    let at = plug_cell(&buffer);
    mouse_at(&mut app, MouseEventKind::Down(MouseButton::Left), at);
    mouse_at(&mut app, MouseEventKind::Up(MouseButton::Left), at);
    app.process_command(UiCommand::SetIntegrations(views()));
    app
}

#[test]
fn the_plug_sits_just_before_benchs_wrench() {
    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 140);
    let bar = rows.iter().find(|row| row.contains("🔧")).unwrap();
    let plug = bar.find("🔌").expect("the plug is drawn");
    let wrench = bar.find("🔧").unwrap();
    assert!(plug < wrench, "{bar}");
    let between = &bar[plug + "🔌".len()..wrench];
    assert_eq!(between.trim(), "│", "neighbouring cells: {bar}");
    assert!(
        !bar.contains("INTEGRATIONS") && !bar.contains("INT "),
        "{bar}"
    );
}

#[test]
fn clicking_the_plug_opens_integrations_and_lights_it() {
    let mut app = integrations_app();
    assert_eq!(app.agents_tab(), AgentsTab::Integrations);
    let buffer = render_buffer(&mut app, 140);
    let (column, row) = plug_cell(&buffer);
    let lit = buffer[(column, row)].style();
    assert!(lit.add_modifier.contains(Modifier::BOLD), "{lit:?}");
    mouse_at(&mut app, MouseEventKind::Moved, (column, row));
    assert_eq!(
        app.hover(),
        Some(HoverTarget::AgentTab(AgentsTab::Integrations))
    );
}

#[test]
fn connected_and_not_connected_integrations_are_listed_with_their_dots() {
    let mut app = integrations_app();
    let rows = render_rows(&mut app, 140);
    let github = line_with(&rows, "⑂ GitHub");
    let boxd = line_with(&rows, "☁ boxd");
    assert!(github < boxd);
    assert!(rows[github].contains('●'), "{}", rows[github]);
    assert!(rows[boxd].contains('○'), "{}", rows[boxd]);
    let actions = line_with(&rows, "Connect  Configure");
    assert!(actions > github && actions < boxd, "under the selected row");
    let disconnect = line_with(&rows, "Disconnect");
    assert!(disconnect >= actions && disconnect < boxd);
    assert!(line_with(&rows, "Test") < boxd);
    assert!(line_with(&rows, "gh login") < actions);

    let buffer = render_buffer(&mut app, 140);
    let dot_row = row_of(&buffer, "⑂ GitHub");
    let (column, _) = find_cell(&buffer, dot_row, "●").unwrap();
    let theme = test_theme();
    assert_eq!(buffer[(column, dot_row)].fg, theme.colors.success);
    // Not connected is the shared dot's Off: hollow, muted.
    let boxd_row = row_of(&buffer, "☁ boxd");
    let (column, _) = find_cell(&buffer, boxd_row, "○").unwrap();
    assert_eq!(buffer[(column, boxd_row)].fg, theme.colors.muted);
}

#[test]
fn connect_asks_for_the_key_masked() {
    let mut app = integrations_app();
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Down,
    ));
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Char('c'),
    ));
    assert_eq!(app.integration_prompt(), "boxd API key: ");
    assert!(app.integration_input_is_secret());
    for c in "bxd_secret".chars() {
        app.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Char(c),
        ));
    }
    let rows = render_rows(&mut app, 140);
    assert!(rows.iter().all(|row| !row.contains("bxd_secret")), "masked");
}

/// INTEGRATIONS with `n` integrations, the first selected.
fn many(n: usize) -> crate::App {
    let mut app = integrations_app();
    let rows = (0..n)
        .map(|i| IntegrationView {
            id: format!("svc{i}"),
            name: format!("Service {i:02}"),
            icon: "◆".to_string(),
            description: format!("Thing {i:02}"),
            ..IntegrationView::default()
        })
        .collect();
    app.process_command(UiCommand::SetIntegrations(rows));
    app
}

#[test]
fn the_wheel_scrolls_the_rows_and_a_key_brings_the_highlight_back() {
    let mut app = many(30);
    let rows = render_rows(&mut app, 140);
    assert!(rows.iter().any(|r| r.contains("◆ Service 00")));
    let (column, row) = {
        let buffer = render_buffer(&mut app, 140);
        let row = row_of(&buffer, "◆ Service 00");
        (5, row)
    };
    for _ in 0..4 {
        mouse_at(&mut app, MouseEventKind::ScrollDown, (column, row));
    }
    let rows = render_rows(&mut app, 140);
    assert!(
        rows.iter().all(|r| !r.contains("◆ Service 00")),
        "scrolled past the first"
    );
    assert_eq!(app.selected_integration(), 0, "the highlight stays");

    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Down,
    ));
    let rows = render_rows(&mut app, 140);
    assert_eq!(app.selected_integration(), 1);
    assert!(
        rows.iter().any(|r| r.contains("◆ Service 01")),
        "a key move follows the highlight again"
    );
}
