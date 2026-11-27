use super::*;
use crate::events::{AgentSummary, UiCommand, UiEvent};
use tokio::sync::mpsc;

const LEAD: &str = "aa";

fn member(id: &str, kind: ProcessKind, owner: &str, started_ms: u64) -> ProcessSummary {
    let prefix = if kind == ProcessKind::Task {
        "task"
    } else {
        "agent"
    };
    ProcessSummary {
        id: format!("{prefix}:{id}"),
        kind,
        label: format!("{id} (scout)"),
        status: ProcessStatus::Running,
        owner_agent_id: owner.into(),
        started_ms,
        jump_agent_id: if kind == ProcessKind::Task {
            id.into()
        } else {
            LEAD.into()
        },
        ..ProcessSummary::default()
    }
}

fn app_with_lead() -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetAgents(vec![AgentSummary {
        id: LEAD.into(),
        name: "Lead".into(),
        ..AgentSummary::default()
    }]));
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    app
}

fn listed(app: &App) -> Vec<(String, usize)> {
    app.members()
        .iter()
        .map(|row| (row.agent_id.clone(), row.depth))
        .collect()
}

fn rows(pairs: &[(&str, usize)]) -> Vec<(String, usize)> {
    pairs.iter().map(|&(id, d)| (id.to_string(), d)).collect()
}

#[test]
fn the_lead_comes_first_and_members_sit_under_who_started_them() {
    let mut app = app_with_lead();
    let mut grandchild = member("cc", ProcessKind::Agent, "bb", 30);
    grandchild.parent_chain = vec![LEAD.into()];
    app.process_command(UiCommand::SetProcesses(vec![
        grandchild,
        member("bb", ProcessKind::Task, LEAD, 10),
        member("dd", ProcessKind::Task, LEAD, 20),
    ]));
    assert_eq!(
        listed(&app),
        rows(&[(LEAD, 0), ("bb", 1), ("cc", 2), ("dd", 1)])
    );
    assert_eq!(app.members()[0].label, "Lead");
    assert_eq!(app.members()[0].kind, None);
}

#[test]
fn agents_a_task_member_spawned_are_members_too() {
    let mut app = app_with_lead();
    // A task's instance roots its own tree of background agents, so
    // their rows name it alone above them.
    app.process_command(UiCommand::SetProcesses(vec![
        member("ee", ProcessKind::Agent, "bb", 40),
        member("bb", ProcessKind::Task, LEAD, 10),
    ]));
    assert_eq!(listed(&app), rows(&[(LEAD, 0), ("bb", 1), ("ee", 2)]));
}

#[test]
fn other_runs_and_other_work_are_left_out() {
    let mut app = app_with_lead();
    let command = ProcessSummary {
        id: "cmd:1".into(),
        kind: ProcessKind::Command,
        owner_agent_id: LEAD.into(),
        ..ProcessSummary::default()
    };
    let turn = ProcessSummary {
        id: format!("turn:{LEAD}"),
        kind: ProcessKind::Turn,
        owner_agent_id: LEAD.into(),
        owner_name: "Lead".into(),
        ..ProcessSummary::default()
    };
    app.process_command(UiCommand::SetProcesses(vec![
        turn,
        command,
        member("bb", ProcessKind::Task, LEAD, 10),
        member("zz", ProcessKind::Task, "ff", 10),
    ]));
    assert_eq!(listed(&app), rows(&[(LEAD, 0), ("bb", 1)]));
    assert_eq!(app.members()[0].status, ProcessStatus::Running);

    // Another agent on screen lists its own run.
    app.process_command(UiCommand::SetActiveAgent("ff".into()));
    assert_eq!(listed(&app), rows(&[("ff", 0), ("zz", 1)]));
}

#[test]
fn members_stay_listed_after_the_process_table_drops_them() {
    let mut app = app_with_lead();
    app.process_command(UiCommand::SetProcesses(vec![member(
        "bb",
        ProcessKind::Task,
        LEAD,
        10,
    )]));
    let mut done = member("bb", ProcessKind::Task, LEAD, 10);
    done.status = ProcessStatus::Exited;
    done.ended_ms = Some(50);
    app.process_command(UiCommand::SetProcesses(vec![done]));
    app.process_command(UiCommand::SetProcesses(Vec::new()));
    assert_eq!(listed(&app), rows(&[(LEAD, 0), ("bb", 1)]));
    assert_eq!(app.members()[1].status, ProcessStatus::Exited);
    assert_eq!(app.members()[1].ended_ms, Some(50));
}

#[test]
fn a_new_session_starts_a_run_no_one_has_joined() {
    let mut app = app_with_lead();
    app.process_command(UiCommand::SetProcesses(vec![member(
        "bb",
        ProcessKind::Task,
        LEAD,
        10,
    )]));
    app.handle_command("/new");
    assert_eq!(listed(&app), rows(&[(LEAD, 0)]));

    let later = now_ms() + 1_000;
    app.process_command(UiCommand::SetProcesses(vec![member(
        "cc",
        ProcessKind::Task,
        LEAD,
        later,
    )]));
    assert_eq!(listed(&app), rows(&[(LEAD, 0), ("cc", 1)]));
}

#[test]
fn the_highlight_stays_on_its_member_as_others_join() {
    let mut app = app_with_lead();
    app.process_command(UiCommand::SetProcesses(vec![
        member("bb", ProcessKind::Task, LEAD, 10),
        member("dd", ProcessKind::Task, LEAD, 20),
    ]));
    app.show_members();
    assert!(app.handle_records_shortcut(KeyEvent::from(KeyCode::End)));
    assert_eq!(app.members()[app.selected_member()].agent_id, "dd");
    app.process_command(UiCommand::SetProcesses(vec![
        member("bb", ProcessKind::Task, LEAD, 10),
        member("cc", ProcessKind::Task, LEAD, 15),
        member("dd", ProcessKind::Task, LEAD, 20),
    ]));
    assert_eq!(app.members()[app.selected_member()].agent_id, "dd");
}

#[test]
fn enter_opens_a_members_process_and_o_its_chat() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = app_with_lead().with_event_sender(tx);
    app.process_command(UiCommand::SetProcesses(vec![
        member("bb", ProcessKind::Task, LEAD, 10),
        member("cc", ProcessKind::Agent, LEAD, 20),
    ]));
    app.show_members();
    app.handle_members_key(KeyEvent::from(KeyCode::Down));
    app.handle_members_key(KeyEvent::from(KeyCode::Enter));
    assert!(
        matches!(app.detail_view(), Some(DetailView::Process(p)) if p.id == "task:bb"),
        "Enter opens the member's process"
    );

    assert!(app.handle_records_shortcut(KeyEvent::from(KeyCode::Char('o'))));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    // A background agent's chat is its tree's, so there is none to open.
    app.handle_members_key(KeyEvent::from(KeyCode::Down));
    assert!(app.handle_records_shortcut(KeyEvent::from(KeyCode::Char('o'))));
    assert!(rx.try_recv().is_err());
}

#[test]
fn shift_left_and_right_switch_the_kernel_panels_tab() {
    let mut app = app_with_lead();
    assert_eq!(app.records_tab(), RecordsTab::Kernel);
    let shift_right = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
    assert!(app.handle_records_shortcut(shift_right));
    assert_eq!(app.records_tab(), RecordsTab::Members);
    assert!(app.handle_records_shortcut(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)));
    assert_eq!(app.records_tab(), RecordsTab::Kernel);
    // Plain letters stay the input's while KERNEL is open.
    assert!(!app.handle_records_shortcut(KeyEvent::from(KeyCode::Char('o'))));
}

#[test]
fn keys_reach_the_members_tab_through_the_focused_kernel_panel() {
    let mut app = app_with_lead();
    app.process_command(UiCommand::SetProcesses(vec![member(
        "bb",
        ProcessKind::Task,
        LEAD,
        10,
    )]));
    app.focus = PanelFocus::Records;
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.records_tab(), RecordsTab::Members);
    app.handle_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(app.selected_member(), 1);
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(app.detail_view(), Some(DetailView::Process(p)) if p.id == "task:bb"));

    // A record jump goes back to the records.
    app.close_detail();
    app.show_turn_records(0);
    assert_eq!(app.records_tab(), RecordsTab::Members, "no records to show");
    app.process_command(UiCommand::SetRecords(vec![
        crate::app::detail_tests::record(1),
    ]));
    app.show_turn_records(0);
    assert_eq!(app.records_tab(), RecordsTab::Kernel);
    assert_eq!(app.focus(), PanelFocus::Records);
}

#[test]
fn clicking_a_tab_in_the_kernel_bar_shows_it() {
    let mut app = app_with_lead();
    let bar = Rect::new(80, 0, 40, 3);
    app.set_record_tabs_area(bar);
    let members = RecordsTab::layout(bar.width, false).cells[1];
    app.click_records_panel(bar.x + members.start, 1);
    assert_eq!(app.records_tab(), RecordsTab::Members);
}

#[test]
fn the_members_command_shows_the_tab_focused() {
    let mut app = app_with_lead();
    app.handle_command("/kernel");
    assert!(!app.record_panel_visible());
    app.handle_command("/members");
    assert!(app.record_panel_visible());
    assert_eq!(app.records_tab(), RecordsTab::Members);
    assert_eq!(app.focus(), PanelFocus::Records);
}
