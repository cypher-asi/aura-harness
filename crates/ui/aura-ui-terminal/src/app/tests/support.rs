use super::*;

pub(super) fn submit(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
}

// ========================================================================
// Agents column
// ========================================================================

pub(super) fn agent(id: &str, name: &str) -> crate::events::AgentSummary {
    crate::events::AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        zns_id: format!("0://terminal/{id}"),
        is_active: false,
        record_count: 0,
        last_active: "-".to_string(),
        ..Default::default()
    }
}

pub(super) fn app_with_agents() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Terminal Agent"),
        agent("bb", "Researcher"),
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    (app, rx)
}

// ========================================================================
// Agents panel: roster tab
// ========================================================================

/// Agents panel focused on the roster tab.
pub(super) fn app_with_definitions() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_agents();
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        crate::events::AgentDefinitionSummary {
            name: "explorer".to_string(),
            description: "Reads code".to_string(),
            ..Default::default()
        },
        crate::events::AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
            ..Default::default()
        },
    ]));
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), crate::app::AgentsTab::Roster);
    (app, rx)
}

/// Open the folder browser with Ctrl+N, pointed at `dir` instead of the
/// launch directory.
pub(super) fn open_folder_picker(app: &mut App, dir: &std::path::Path) {
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!(app.state(), AppState::SelectingFolder);
    app.project_list.folder_picker =
        Some(folder_picker::FolderPicker::open(dir.to_path_buf()).unwrap());
}

pub(super) fn mouse(kind: crossterm::event::MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

pub(super) fn app_with_agent_areas() -> (App, mpsc::Receiver<UiEvent>) {
    let (app, rx) = app_with_agents();
    (with_agent_areas(app), rx)
}

pub(super) fn with_agent_areas(mut app: App) -> App {
    let swarm = Rect::new(0, 3, 20, 13);
    app.set_panel_areas(crate::app::PanelAreas {
        swarm: Some(swarm),
        agent_rows: None,
        chat: Some(Rect::new(20, 3, 60, 13)),
        ..crate::app::PanelAreas::default()
    });
    app.set_agent_rows_area(Rect::new(1, 4, 18, 10));
    app.set_new_project_area(Rect::new(1, 14, 18, 1));
    app
}

/// Projects `aa` (with child `aa1`), `bb` and `cc`. Rows from line 4:
/// aa 4, 5, 6; gap 7; bb 8, 9; gap 10; cc 11, 12.
pub(super) fn app_with_three_projects() -> App {
    let mut app = App::new();
    let child = crate::events::AgentSummary {
        depth: 1,
        ..agent("aa1", "Child")
    };
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "A"),
        child,
        agent("bb", "B"),
        agent("cc", "C"),
    ]));
    with_agent_areas(app)
}

pub(super) fn agent_ids(app: &App) -> Vec<&str> {
    app.agents().iter().map(|a| a.id.as_str()).collect()
}

pub(super) fn left(app: &mut App, kind: fn(MouseButton) -> MouseEventKind, row: u16) {
    app.handle_mouse(mouse(kind(MouseButton::Left), 5, row));
}

// ========================================================================
// Activity blocks
// ========================================================================

pub(super) fn tool_data(id: &str, name: &str) -> UiCommand {
    UiCommand::ShowTool(crate::events::ToolData {
        id: id.to_string(),
        name: name.to_string(),
        args: String::new(),
    })
}

pub(super) fn activities(app: &App) -> Vec<&Activity> {
    app.messages()
        .iter()
        .filter_map(Message::activity_step)
        .collect()
}

/// One collapsed step row per message id, on consecutive lines.
pub(super) fn step_rows(ids: &[u64]) -> Vec<ToggleRow> {
    ids.iter()
        .enumerate()
        .map(|(line, &id)| ToggleRow {
            line,
            target: ToggleTarget::Step(id),
            open: false,
        })
        .collect()
}

// ========================================================================
// Notification types
// ========================================================================

// ========================================================================
// Model selector
// ========================================================================

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

pub(super) fn app_with_models(user: Option<&str>) -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetModels {
        options: vec![
            ModelOptionView {
                id: "local/qwen3:4b".to_string(),
                label: "qwen3:4b".to_string(),
                local: true,
                installed: true,
                group: "Local (no login)".to_string(),
                provider: None,
                available: true,
                context_window: Some(32_768),
            },
            ModelOptionView {
                id: "aura-claude-sonnet-4-6".to_string(),
                label: "Claude Sonnet 4.6".to_string(),
                local: false,
                installed: true,
                group: "Cloud (aura-router)".to_string(),
                provider: Some("zos".to_string()),
                available: user.is_some(),
                context_window: Some(1_000_000),
            },
            ModelOptionView {
                id: "claude/claude-sonnet-4-6".to_string(),
                label: "Claude Sonnet 4.6".to_string(),
                local: false,
                installed: true,
                group: "Claude plan (direct)".to_string(),
                provider: Some("claude".to_string()),
                available: false,
                context_window: Some(1_000_000),
            },
        ],
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: user.map(str::to_string),
        plans: Vec::new(),
        thinking: None,
    });
    (app, rx)
}

// ========================================================================
// Slash menu
// ========================================================================

pub(super) fn type_chars(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

// ========================================================================
// Approvals, modes and alerts
// ========================================================================

pub(super) fn approval_request(id: &str, always: &str) -> UiCommand {
    UiCommand::RequestApproval {
        id: id.to_string(),
        tool: "run_command".to_string(),
        description: "cargo test".to_string(),
        always: always.to_string(),
    }
}

pub(super) fn for_agent(agent_id: &str, command: UiCommand) -> UiCommand {
    UiCommand::ForAgent {
        agent_id: agent_id.to_string(),
        command: Box::new(command),
    }
}

// ========================================================================
// @ mentions
// ========================================================================

pub(super) fn app_with_workspace() -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("target")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(dir.path().join("README.md"), "").unwrap();
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    (dir, app, rx)
}

pub(super) fn type_keys(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
}

pub(super) fn mention_paths(app: &App) -> Vec<String> {
    app.mention_matches()
        .iter()
        .map(|c| c.path.clone())
        .collect()
}

/// An app whose chat holds more than fits, drawn once on a 90 × 30 screen.
pub(super) fn overflowing_chat() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = App::new();
    for i in 0..60 {
        app.add_message(Message::new(MessageRole::Assistant, &format!("reply {i}")));
    }
    let terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    (app, terminal)
}

pub(super) fn draw(app: &mut App, terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>) {
    let theme = crate::Theme::by_name("cyber");
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &theme))
        .unwrap();
}

/// The screen cell where `text` is drawn, if it is.
pub(super) fn cell_of(
    terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
    text: &str,
) -> Option<(u16, u16)> {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height).find_map(|y| {
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        let at = row.find(text)?;
        let x = u16::try_from(row[..at].chars().count()).ok()?;
        Some((x, y))
    })
}

pub(super) fn click_at(app: &mut App, (column, row): (u16, u16)) {
    use crossterm::event::{MouseButton, MouseEventKind};
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
}

/// The text of the screen `app` draws, a line per row.
pub(super) fn drawn_text(app: &mut App) -> String {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).expect("backend");
    terminal
        .draw(|frame| crate::render(frame, app, &crate::Theme::default()))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn load_chat_of(app: &mut App, id: &str, text: &str) {
    app.process_command(UiCommand::ForAgent {
        agent_id: id.to_string(),
        command: Box::new(UiCommand::LoadConversation(vec![MessageData::new(
            MessageRole::Assistant,
            text,
        )])),
    });
}

/// The channel `#zode` (`c1`), coordinated by `coordinator`.
pub(super) fn zode(coordinator: &str) -> crate::events::ChannelSummary {
    crate::events::ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        title: "Zode release".to_string(),
        coordinator_id: coordinator.to_string(),
        members: vec![agent(coordinator, "Coordinator")],
        ..crate::events::ChannelSummary::default()
    }
}

/// The app with `#zode`, coordinated by `aa` (on screen), listed, and
/// come to CHANNELS, which opened it.
pub(super) fn app_with_channel() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, mut rx) = app_with_agents();
    app.process_command(UiCommand::SetChannels(vec![zode("aa")]));
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(app.channel_open(), "coming to CHANNELS opens it");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
    (app, rx)
}

// ========================================================================
// Channel coordinator chat
// ========================================================================

/// The app of [`app_with_workspace`] with `#zode` open and the chat on
/// `active`; `co` coordinates the channel, `RE36` and `SUB2` work in it.
pub(super) fn app_in_channel(active: &str) -> (tempfile::TempDir, App) {
    use crate::events::{AgentSummary, ChannelSummary};
    let member = |id: &str, code: &str, name: &str, depth: usize| AgentSummary {
        id: id.to_string(),
        code: Some(code.to_string()),
        name: name.to_string(),
        depth,
        ..AgentSummary::default()
    };
    let (dir, mut app, _rx) = app_with_workspace();
    app.process_command(UiCommand::SetChannels(vec![ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        coordinator_id: "co".to_string(),
        members: vec![
            member("co", "CO1", "Lead", 0),
            member("re", "RE36", "Reviewer", 0),
            member("sb", "SUB2", "Scout", 1),
        ],
        ..ChannelSummary::default()
    }]));
    // These tests are about the subagent's rows too: show them.
    app.unfold_channel_members();
    app.process_command(UiCommand::SetActiveAgent(active.to_string()));
    app.open_channel_by_id("c1");
    // Opening puts the coordinator on screen; the session confirms it,
    // then the test's agent comes on screen as a member's chat would.
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.process_command(UiCommand::SetActiveAgent(active.to_string()));
    app.focus = PanelFocus::Chat;
    (dir, app)
}

pub(super) fn project_summary(id: &str, name: &str) -> crate::events::ProjectSummary {
    crate::events::ProjectSummary {
        id: id.to_string(),
        name: name.to_string(),
        folder: format!("/src/{name}"),
        ..crate::events::ProjectSummary::default()
    }
}

pub(super) fn project_view(id: &str, name: &str) -> crate::events::ChannelProjectView {
    crate::events::ChannelProjectView {
        id: id.to_string(),
        name: name.to_string(),
        folder: format!("/src/{name}"),
    }
}

/// Projects `alpha` (`p1`) and `beta` (`p2`), in that order.
pub(super) fn alpha_and_beta() -> UiCommand {
    UiCommand::SetProjects(vec![
        project_summary("p1", "alpha"),
        project_summary("p2", "beta"),
    ])
}

/// `app_with_channel` where `#zode` works in project `beta` and the app
/// lists projects `alpha` and `beta`.
pub(super) fn app_with_projects_channel() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_channel();
    app.process_command(alpha_and_beta());
    app.process_command(UiCommand::SetChannels(vec![
        crate::events::ChannelSummary {
            description: "Ship the zode crate".to_string(),
            project: Some(project_view("p2", "beta")),
            ..zode("aa")
        },
    ]));
    (app, rx)
}

/// Draw the app and right-click the first cell showing `text` on a row
/// `pick` accepts, e.g. to skip the chat bar's title.
pub(super) fn right_click_on_where(app: &mut App, text: &str, pick: impl Fn(&str) -> bool) {
    use crossterm::event::{MouseButton, MouseEventKind};
    let drawn = drawn_text(app);
    let rows: Vec<&str> = drawn.lines().collect();
    let row = rows
        .iter()
        .position(|row| row.contains(text) && pick(row))
        .unwrap_or_else(|| panic!("no {text}:\n{drawn}"));
    let line = rows[row];
    let column = line[..line.find(text).unwrap_or(0)].chars().count();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::NONE,
    });
}

pub(super) fn menu_items(app: &App) -> Vec<super::super::context_menu::MenuItem> {
    app.context_menu()
        .map(|menu| menu.items.clone())
        .unwrap_or_default()
}
