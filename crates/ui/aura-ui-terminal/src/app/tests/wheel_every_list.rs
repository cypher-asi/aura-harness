//! The wheel never moves a highlight or opens anything, over any list of
//! any panel: every tab of the agents panel, the Record panel and the
//! side panel, with and without a channel open. The cases are matched on
//! the tab enums with no catch-all, so a tab added later does not build
//! until it says what it lists and how its highlight is read here.

use super::*;
use crate::app::{AgentsTab, Dialog, RecordsTab, SidePanelTab};
use crate::events::{
    AgentDefinitionSummary, ChannelPostAuthor, ChannelPostView, ChannelSummary, MailView, PlanView,
    ProcessKind, ProcessStatus, ProcessSummary, TaskView, TodoState, TodoView, TraceNode,
};
use crate::repo::{EventKind, EventState, RepoEvent, RepoTarget, RepoView};
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

/// A panel's list: which panel it is in and how its highlight reads.
struct List {
    name: String,
    area: fn(&App) -> Option<Rect>,
    /// Its highlight, for a list that has one; `None` for a document
    /// that only scrolls (INFO, QUEUE, HOST, POLICIES)
    highlight: fn(&App) -> Option<usize>,
}

/// The app with the agents `aa` and `bb`, `aa` on screen, and `#zode`
/// coordinated by `aa` listed; open when `channel` is.
fn app(channel: bool) -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(4096);
    let mut app = App::new().with_event_sender(tx);
    let agents = (0..ROWS)
        .map(|n| match n {
            0 => agent("aa", "Terminal Agent"),
            1 => agent("bb", "Researcher"),
            n => agent(&format!("a{n:02}"), &format!("Agent {n:02}")),
        })
        .collect();
    app.process_command(UiCommand::SetAgents(agents));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    let mut zode = zode("aa");
    zode.members
        .extend((0..ROWS).map(|n| agent(&format!("m{n:02}"), &format!("Member {n:02}"))));
    let channels = std::iter::once(zode)
        .chain((0..ROWS).map(|n| ChannelSummary {
            id: format!("c{n:02}x"),
            slug: format!("chan{n:02}"),
            title: format!("Channel {n:02}"),
            coordinator_id: "aa".to_string(),
            ..ChannelSummary::default()
        }))
        .collect();
    app.process_command(UiCommand::SetChannels(channels));
    if channel {
        app.open_channel_by_id("c1");
    }
    (app, rx)
}

/// Fill what the agents panel's `tab` lists and show it.
fn fill_agents(app: &mut App, tab: AgentsTab) -> List {
    match tab {
        AgentsTab::Channels | AgentsTab::Projects => {}
        AgentsTab::Roster => {
            let definitions = (0..ROWS)
                .map(|n| AgentDefinitionSummary {
                    name: format!("def{n:02}"),
                    ..AgentDefinitionSummary::default()
                })
                .collect();
            app.process_command(UiCommand::SetAgentDefinitions(definitions));
        }
        AgentsTab::Bench => {
            let benches = (0..ROWS)
                .map(|n| crate::events::BenchView {
                    id: format!("{n}"),
                    root: "aa".to_string(),
                    name: format!("Compare {n:02}"),
                    title: format!("work {n:02}"),
                    ..crate::events::BenchView::default()
                })
                .collect();
            app.process_command(UiCommand::BenchList { benches });
        }
        AgentsTab::Archive => {
            let archived = (0..ROWS)
                .map(|n| crate::events::AgentSummary {
                    last_active_ms: Some(1_700_000_000_000),
                    ..agent(&format!("z{n:02}"), &format!("Old {n:02}"))
                })
                .collect();
            app.process_command(UiCommand::SetArchivedAgents(archived));
        }
        AgentsTab::Integrations => {
            let integrations = (0..ROWS)
                .map(|n| crate::events::IntegrationView {
                    id: format!("svc{n}"),
                    name: format!("Service {n:02}"),
                    ..crate::events::IntegrationView::default()
                })
                .collect();
            app.process_command(UiCommand::SetIntegrations(integrations));
        }
    }
    app.show_agents_tab(tab);
    app.focus = PanelFocus::Swarm;
    let highlight: fn(&App) -> Option<usize> = match tab {
        AgentsTab::Channels => |app| Some(app.selected_channel()),
        AgentsTab::Projects => |app| Some(app.selected_agent()),
        AgentsTab::Roster => |app| Some(app.definitions.selected),
        AgentsTab::Bench => |app| Some(app.selected_bench()),
        AgentsTab::Archive => |app| Some(app.archived.selected),
        AgentsTab::Integrations => |app| Some(app.selected_integration()),
    };
    List {
        name: format!("{tab:?}"),
        area: |app| app.areas.swarm,
        highlight,
    }
}

fn process(n: usize, kind: ProcessKind) -> ProcessSummary {
    ProcessSummary {
        id: format!(
            "{}:p{n:02}",
            if kind == ProcessKind::Task {
                "task"
            } else {
                "cmd"
            }
        ),
        kind,
        label: format!("process {n:02}"),
        status: ProcessStatus::Running,
        owner_agent_id: "aa".to_string(),
        jump_agent_id: format!("p{n:02}"),
        started_ms: 10 + n as u64,
        ..ProcessSummary::default()
    }
}

/// Fill what the Record panel's `tab` lists and show it.
fn fill_records(app: &mut App, tab: RecordsTab) -> List {
    match tab {
        RecordsTab::Kernel => {
            let records = (0..ROWS as u64)
                .map(super::super::detail_tests::record)
                .collect();
            app.process_command(UiCommand::SetRecords(records));
        }
        RecordsTab::Members => {
            let members = (0..ROWS).map(|n| process(n, ProcessKind::Task)).collect();
            app.process_command(UiCommand::SetProcesses(members));
        }
        RecordsTab::Policies => {}
        RecordsTab::Inbox => {
            for n in 0..ROWS as u64 {
                app.process_command(UiCommand::AddMail(MailView {
                    id: n + 1,
                    thread_id: format!("t{n:02}"),
                    subject: format!("About {n:02}"),
                    from_id: "bb".to_string(),
                    from_name: "BB".to_string(),
                    to_id: "aa".to_string(),
                    to_name: "AA".to_string(),
                    reply: false,
                    text: format!("message {n}"),
                    sent_ms: 1_700_000_000_000 + n,
                }));
            }
        }
    }
    app.show_records_tab(tab);
    app.focus = PanelFocus::Records;
    let highlight: fn(&App) -> Option<usize> = match tab {
        RecordsTab::Kernel => |app| Some(app.selected_record()),
        RecordsTab::Members => |app| Some(app.selected_member()),
        RecordsTab::Policies => |_| None,
        RecordsTab::Inbox => |app| Some(app.selected_inbox_row()),
    };
    List {
        name: format!("{tab:?}"),
        area: |app| app.areas.records,
        highlight,
    }
}

/// Fill what the side panel's `tab` lists.
fn side_rows(app: &mut App, tab: SidePanelTab, dir: &std::path::Path) {
    match tab {
        SidePanelTab::Sandbox => {
            for n in 0..ROWS {
                std::fs::write(dir.join(format!("file{n:02}.rs")), "").unwrap();
            }
            app.process_command(UiCommand::SetSandboxRoot(dir.to_path_buf()));
        }
        SidePanelTab::Repo => repo_rows(app, dir),
        SidePanelTab::Processes => {
            let processes = (0..ROWS)
                .map(|n| process(n, ProcessKind::Command))
                .collect();
            app.process_command(UiCommand::SetProcesses(processes));
        }
        SidePanelTab::Host | SidePanelTab::ChannelInfo | SidePanelTab::ChannelQueue => {}
        SidePanelTab::Trace => trace_rows(app),
        SidePanelTab::Tasks => plan_rows(app),
        SidePanelTab::ChannelMembers => {}
        SidePanelTab::ChannelHistory => history_posts(app),
    }
}

/// Many commits on REPO's timeline.
fn repo_rows(app: &mut App, dir: &std::path::Path) {
    let events = (0..100_i64)
        .take(ROWS)
        .map(|n| RepoEvent {
            kind: EventKind::Commit,
            state: EventState::Done,
            at: 100 - n,
            title: format!("commit {n:02}"),
            detail: String::new(),
            target: RepoTarget::Commit {
                sha: format!("{n:08}"),
            },
            url: None,
        })
        .collect();
    app.process_command(UiCommand::SetRepo(Box::new(RepoView {
        agent_id: "aa".into(),
        root: dir.to_path_buf(),
        branch: Some("main".into()),
        head: "00000000".into(),
        events,
        ..RepoView::default()
    })));
}

/// Many steps in the TRACE of the agent on screen.
fn trace_rows(app: &mut App) {
    let nodes = (0..ROWS)
        .map(|n| TraceNode {
            id: format!("n{n:02}"),
            kind: "tool_call".to_string(),
            ..TraceNode::default()
        })
        .collect();
    app.process_command(UiCommand::GraphUpsert {
        run_id: "run-aa".to_string(),
        nodes,
    });
}

/// A plan of many todos on TASKS.
fn plan_rows(app: &mut App) {
    // A plan of many todos, which a task of `aa` works on.
    let todos = (0..ROWS)
        .map(|n| TodoView {
            content: format!("todo {n:02}"),
            state: TodoState::Pending,
            assignee: None,
            task: None,
        })
        .collect();
    app.process_command(UiCommand::SetPlans(vec![PlanView {
        path: "big.plan.md".to_string(),
        name: "big".to_string(),
        todos,
        changed_ms: None,
    }]));
    app.process_command(UiCommand::SetTasks(vec![TaskView {
        id: "t1".to_string(),
        title: "build it".to_string(),
        agent_id: "aa".to_string(),
        plan: Some("big.plan.md".to_string()),
        ..TaskView::default()
    }]));
}

/// Many posts of the user in the open channel, for HISTORY.
fn history_posts(app: &mut App) {
    for n in 0..ROWS as u64 {
        app.process_command(UiCommand::ChannelPost {
            channel_id: "c1".to_string(),
            post: ChannelPostView {
                seq: n + 1,
                author: ChannelPostAuthor::User,
                text: format!("post {n:02}"),
                at_ms: 1_704_200_000_000 + n * 1000,
                ..ChannelPostView::default()
            },
        });
    }
}

/// Fill what the side panel's `tab` lists and show it.
fn fill_side(app: &mut App, tab: SidePanelTab, dir: &std::path::Path) -> List {
    side_rows(app, tab, dir);
    app.show_side_tab(tab);
    app.focus = PanelFocus::Sandbox;
    let highlight: fn(&App) -> Option<usize> = match tab {
        SidePanelTab::Sandbox => |app| Some(app.sandbox().selected()),
        SidePanelTab::Repo => |app| Some(app.repo_selected()),
        SidePanelTab::Processes => |app| Some(app.side.processes.selected),
        SidePanelTab::Trace => |app| Some(app.trace_feed().selected()),
        SidePanelTab::Tasks => |app| Some(app.task_list().selected()),
        SidePanelTab::ChannelMembers => |app| Some(app.selected_channel_member()),
        SidePanelTab::ChannelHistory => |app| Some(app.selected_history_item()),
        SidePanelTab::Host | SidePanelTab::ChannelInfo | SidePanelTab::ChannelQueue => |_| None,
    };
    List {
        name: format!("{tab:?}"),
        area: |app| app.areas.sandbox,
        highlight,
    }
}

/// Open `dialog` over a list of many rows where it has one.
fn fill_dialog(app: &mut App, dialog: Dialog, dir: &std::path::Path) -> List {
    match dialog {
        Dialog::Help => {
            app.handle_command("/help");
        }
        Dialog::ModelSelector => {
            let options = (0..ROWS)
                .map(|n| crate::events::ModelOptionView {
                    id: format!("local/m{n:02}"),
                    label: format!("Model {n:02}"),
                    local: true,
                    installed: true,
                    group: "Local".to_string(),
                    provider: None,
                    available: true,
                    context_window: None,
                })
                .collect();
            app.process_command(UiCommand::SetModels {
                options,
                current: "local/m00".to_string(),
                route: None,
                user: None,
                plans: Vec::new(),
                thinking: None,
            });
            app.handle_command("/model");
        }
        Dialog::FolderPicker => {
            for n in 0..ROWS {
                std::fs::create_dir_all(dir.join(format!("dir{n:02}"))).unwrap();
            }
            open_folder_picker(app, dir);
        }
        Dialog::ProjectInfo => {
            app.process_command(alpha_and_beta());
            app.show_project_info("p1");
        }
        Dialog::ChannelForm => app.start_new_channel(),
    }
    let highlight: fn(&App) -> Option<usize> = match dialog {
        Dialog::ModelSelector => |app| Some(app.model_picker().cursor()),
        Dialog::FolderPicker => |app| app.folder_picker().map(|picker| picker.selected()),
        // A document that scrolls, a page of facts, a form.
        Dialog::Help | Dialog::ProjectInfo | Dialog::ChannelForm => |_| None,
    };
    List {
        name: format!("{dialog:?}"),
        area: |app| app.modal_area().map(|modal| modal.frame),
        highlight,
    }
}

/// What the wheel must leave alone: the agent on screen, the channel
/// open, and the chat tab shown.
fn on_screen(app: &App) -> (String, Option<String>, crate::app::ChatTab) {
    let channel = app.open_channel().map(|channel| channel.id.clone());
    (app.active_agent_id().to_string(), channel, app.chat_tab())
}

/// Turn the wheel over the middle of `list`'s panel: down three notches,
/// then up one, drawing a frame after each, as a front end does. Returns
/// what went wrong: the highlight moved, something opened, or the arrows
/// no longer move the highlight of a list that has one.
fn check(app: &mut App, rx: &mut mpsc::Receiver<UiEvent>, list: &List) -> Result<(), String> {
    let name = &list.name;
    draw(app);
    while rx.try_recv().is_ok() {}
    let highlight = (list.highlight)(app);
    let shown = on_screen(app);
    let area = (list.area)(app).ok_or(format!("{name}: its panel is not drawn"))?;
    let (column, row) = (area.x + area.width / 2, area.y + area.height / 2);
    for kind in [
        MouseEventKind::ScrollDown,
        MouseEventKind::ScrollDown,
        MouseEventKind::ScrollDown,
        MouseEventKind::ScrollUp,
    ] {
        app.handle_mouse(mouse(kind, column, row));
        draw(app);
        let now = (list.highlight)(app);
        if now != highlight {
            return Err(format!(
                "{name}: the wheel moved the highlight {highlight:?} -> {now:?}"
            ));
        }
    }
    if on_screen(app) != shown {
        return Err(format!("{name}: the wheel opened something"));
    }
    let asked: Vec<UiEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    if asked.iter().any(|event| {
        matches!(
            event,
            UiEvent::SelectAgent(_) | UiEvent::FocusAgent(_) | UiEvent::OpenChannel(_)
        )
    }) {
        return Err(format!("{name}: the wheel asked {asked:?}"));
    }
    if let Some(before) = highlight {
        // A list whose highlight starts on its last row moves up.
        app.handle_key(key(KeyCode::Down));
        if (list.highlight)(app) == Some(before) {
            app.handle_key(key(KeyCode::Up));
        }
        if (list.highlight)(app) == Some(before) {
            return Err(format!("{name}: the arrows no longer move the highlight"));
        }
    }
    Ok(())
}

/// Every list `lists` sets up passes [`check`]; the failures are named
/// together.
fn check_all(lists: impl IntoIterator<Item = (App, mpsc::Receiver<UiEvent>, List)>) {
    let failed: Vec<String> = lists
        .into_iter()
        .filter_map(|(mut app, mut rx, list)| check(&mut app, &mut rx, &list).err())
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn the_wheel_moves_no_highlight_in_any_agents_panel_list() {
    check_all(AgentsTab::ALL.into_iter().flat_map(|tab| {
        [false, true].map(|channel| {
            let (mut app, rx) = app(channel);
            let mut list = fill_agents(&mut app, tab);
            list.name = format!("{} (channel open: {channel})", list.name);
            (app, rx, list)
        })
    }));
}

#[test]
fn the_wheel_moves_no_highlight_in_any_record_panel_list() {
    check_all(RecordsTab::ALL.map(|tab| {
        let (mut app, rx) = app(false);
        let list = fill_records(&mut app, tab);
        (app, rx, list)
    }));
}

#[test]
fn the_wheel_moves_no_highlight_in_any_side_panel_list() {
    let dir = tempfile::tempdir().unwrap();
    let tabs = [false, true].into_iter().flat_map(|channel| {
        let icon = SidePanelTab::icon_tab(channel);
        SidePanelTab::tabs(channel)
            .iter()
            .copied()
            .chain(icon)
            .map(move |tab| (channel, tab))
    });
    check_all(tabs.map(|(channel, tab)| {
        let (mut app, rx) = app(channel);
        let mut list = fill_side(&mut app, tab, dir.path());
        list.name = format!("{} (channel open: {channel})", list.name);
        (app, rx, list)
    }));
}

#[test]
fn the_wheel_moves_no_highlight_in_any_dialog() {
    let dir = tempfile::tempdir().unwrap();
    check_all(Dialog::ALL.map(|dialog| {
        let (mut app, rx) = app(false);
        let list = fill_dialog(&mut app, dialog, dir.path());
        (app, rx, list)
    }));
}
