//! Screens of the chat, the panels and the menus.

use super::fixtures::{agent, seed, session, steps};
use super::support::snapshot;
use crate::chrome::Divider;
use crate::geom::Point;
use crate::shell::tests::stage;
use crate::shell::TICK;
use aura_ui::events::{
    MessageData, MessageRole, PolicyView, ProcessKind, ProcessStatus, ProcessSummary, TodoState,
    TodoView, ToolPolicyState, ToolPolicyView, UiCommand,
};
use aura_ui::host::{HostAction, HostFact, HostSection, HostView, PlacementState, PlacementView};
use aura_ui::mode_command::PermissionMode;
use aura_ui::repo::{
    ChangeKind, EventKind, EventState, PrState, RepoChange, RepoEvent, RepoTarget, RepoView,
};
use aura_ui::ApprovalLevel;
use aura_ui_terminal::{AgentsTab, ChatTab, PaneTab, RecordsTab, SidePanelTab};

#[test]
fn prose_is_set_in_inter_and_code_stays_on_the_grid() {
    let mut h = stage();
    seed(&mut h);
    h.apply([
        UiCommand::StartStreaming,
        UiCommand::AppendText(
            "Hi! That last message looks like a typo. What would you like to do?\n\n\
             - **\"ls\":** I said I'd created a task, but I hadn't done anything. If you \
             want, I can list the files in the project folder now.\n\n\
             Here are some things I can help with in this repo (`aura-harness`): run \
             the build, tests, or `cargo clippy`.\n\n```rust\nfn main() {}\n```"
                .into(),
        ),
        UiCommand::FinishStreaming,
        UiCommand::Complete,
    ]);
    let text = h.screen();
    assert!(text.contains("files in the project folder"), "{text}");
    assert!(text.contains("fn main() {}"), "{text}");
    assert!(!h.shell.prose.is_empty(), "the reply's text is prose");
    snapshot("prose", &mut h);
}

#[test]
fn the_chat_slides_half_a_row_up() {
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta;

    let mut h = stage();
    seed(&mut h);
    let conversation: Vec<MessageData> = (0..30)
        .flat_map(|turn| {
            [
                MessageData::new(
                    MessageRole::User,
                    format!("Question {turn}: what runs next?"),
                ),
                MessageData::new(
                    MessageRole::Assistant,
                    format!(
                        "Turn {turn}: the kernel records the step, checks it against the \
                         policy and runs it. Then `cargo test` passes.\n\n- one\n- two"
                    ),
                ),
            ]
        })
        .collect();
    h.apply([UiCommand::LoadConversation(conversation)]);
    let (width, height) = (h.shell.window.width, h.shell.window.height);
    h.shell.pointer_moved(Point::new(width / 2.0, height / 2.0));
    h.shell.frame(h.now);
    let row = f64::from(h.shell.metrics.cell.height / crate::shell::glide::TOUCHPAD_GAIN);
    for _ in 0..41 {
        h.shell.wheel(
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, row * 1.5)),
            h.now,
        );
        h.shell.frame(h.now);
    }
    assert!(h.shell.tabs.front().tui.scroll_offset() > 0);
    snapshot("chat-sliding", &mut h);
}

#[test]
fn welcome_screen() {
    let mut h = stage();
    seed(&mut h);
    snapshot("welcome", &mut h);
}

#[test]
fn panels_with_a_tab_picked_and_a_divider_moved() {
    let mut h = stage();
    session(&mut h);
    let terminal = PaneTab::Chat(ChatTab::Terminal);
    let tab = h.shell.chrome.spot(terminal).expect("the tab is drawn");
    h.drag(tab, tab);
    assert_eq!(h.shell.tabs.front().tui.chat_tab(), ChatTab::Terminal);
    let band = h.shell.chrome.band(Divider::Agents);
    let band = band.expect("agents beside the chat");
    let cell = h.shell.metrics.cell.width;
    h.drag(band, Point::new(band.x + 10.0 * cell, band.y));
    snapshot("panes", &mut h);
}

#[test]
fn an_empty_archive_says_how_agents_get_there() {
    let mut h = stage();
    session(&mut h);
    let archive = PaneTab::Agents(AgentsTab::Archive);
    let tab = h.shell.chrome.spot(archive).expect("the tab is drawn");
    h.drag(tab, tab);
    let screen = h.screen();
    assert!(screen.contains("  No archived agents."), "{screen}");
    snapshot("archive", &mut h);
}

#[test]
fn cloud_agents_are_marked_in_the_list_and_the_footer() {
    let mut h = stage();
    seed(&mut h);
    let cloud = |owner: Option<&str>| PlacementState {
        placement: PlacementView::Cloud,
        owner: owner.map(Into::into),
        ..PlacementState::default()
    };
    h.apply([
        UiCommand::SetAgents(vec![
            agent("a1", "Agent01", 0, Some("aura-harness")),
            agent("a2", "Reviewer", 1, None),
            agent("a3", "Planner", 0, Some("aura-harness")),
        ]),
        UiCommand::SetPlacement {
            agent_id: "a1".into(),
            state: cloud(None),
        },
        UiCommand::SetPlacement {
            agent_id: "a3".into(),
            state: cloud(Some("Alice")),
        },
    ]);
    let screen = h.screen();
    assert!(screen.contains("Agent01 ☁  "), "{screen}");
    assert!(screen.contains("Planner ☁  Alice"), "{screen}");
    assert!(screen.contains("● Ready  ☁  cloud"), "{screen}");
    snapshot("cloud", &mut h);
}

#[test]
fn the_kernel_bar_opens_the_members_of_the_run() {
    let mut h = stage();
    session(&mut h);
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default();
    h.apply([UiCommand::SetProcesses(vec![ProcessSummary {
        id: "task:a2".into(),
        kind: ProcessKind::Task,
        label: "Reviewer (reviewer)".into(),
        detail: "Review the README change".into(),
        status: ProcessStatus::Running,
        owner_agent_id: "a1".into(),
        owner_name: "Agent01".into(),
        started_ms: now - 12_000,
        jump_agent_id: "a2".into(),
        killable: true,
        ..ProcessSummary::default()
    }])]);
    let members = PaneTab::Records(RecordsTab::Members);
    let tab = h.shell.chrome.spot(members).expect("the tab is drawn");
    h.drag(tab, tab);
    assert_eq!(h.shell.tabs.front().tui.records_tab(), RecordsTab::Members);
    let screen = h.screen();
    assert!(
        screen.contains("Agent01") && screen.contains("Reviewer"),
        "{screen}"
    );
    snapshot("members", &mut h);
}

fn policies() -> UiCommand {
    let tool = |name: &str, state, rule: &str| ToolPolicyView {
        name: name.into(),
        state,
        decided_by: rule.into(),
    };
    UiCommand::SetPolicies(PolicyView {
        mode: PermissionMode::Agent,
        approval: ApprovalLevel::Yolo,
        policy_hash: "3f9c0a7e5b2d4c18".into(),
        action_kinds: vec!["delegate".into(), "reason".into()],
        max_proposals: 8,
        capabilities: vec!["SpawnAgent".into(), "InvokeProcess".into()],
        capability_total: 12,
        scope: "all orgs, projects and agents".into(),
        tools: vec![
            tool("read_file", ToolPolicyState::On, "user_default.per_tool"),
            tool("write_file", ToolPolicyState::Ask, "user_default.fallback"),
            tool("git_push", ToolPolicyState::Off, "mode.plan"),
        ],
        commands_confined: true,
        live_prompts: true,
        prompt_timeout_secs: 600,
        ..PolicyView::default()
    })
}

fn host() -> UiCommand {
    UiCommand::SetHost(Box::new(HostView {
        agent_id: "a1".into(),
        placement: PlacementView::Cloud,
        title: "boxd · aura-re36-3f9c".into(),
        status: "● ready · up 2h 14m".into(),
        sections: vec![
            HostSection {
                title: "MACHINE".into(),
                facts: vec![
                    HostFact::new("Region", "fra"),
                    HostFact::new("Size", "4 vCPU · 16 GiB"),
                ],
            },
            HostSection {
                title: "RESOURCES".into(),
                facts: vec![
                    HostFact::new("Memory", "6.0 GiB of 16.0 GiB").with_percent(37.5),
                    HostFact::new("Disk", "91 GiB of 100 GiB").with_percent(91.0),
                ],
            },
        ],
        actions: vec![HostAction::Pull, HostAction::Sleep],
        ..HostView::default()
    }))
}

#[test]
fn policies_read_like_the_host_tab_below_them() {
    let mut h = stage();
    session(&mut h);
    h.apply([policies(), host()]);
    for tab in [
        PaneTab::Records(RecordsTab::Policies),
        PaneTab::Side(SidePanelTab::Host),
    ] {
        let spot = h.shell.chrome.spot(tab).expect("the tab is drawn");
        h.drag(spot, spot);
    }
    let screen = h.screen();
    for text in ["STATUS", "  Approval  yolo", "RESOURCES", "  Region    fra"] {
        assert!(screen.contains(text), "{text:?} in {screen}");
    }
    snapshot("policies_over_host", &mut h);
}

#[test]
fn the_repo_tab_lists_the_branch_its_changes_and_what_happened() {
    let mut h = stage();
    session(&mut h);
    let view = repo_view(chrono::Utc::now().timestamp());
    h.apply([UiCommand::SetRepo(Box::new(view))]);
    let repo = PaneTab::Side(SidePanelTab::Repo);
    let tab = h.shell.chrome.spot(repo).expect("the tab is drawn");
    h.drag(tab, tab);
    assert_eq!(h.shell.tabs.front().tui.side_tab(), SidePanelTab::Repo);
    let screen = h.screen();
    for text in [
        " All  Changes  Commits ",
        "main",
        "?  src/repo.rs",
        "Add the REPO tab",
        "#42 REPO tab",
        "Pushed 3",
    ] {
        assert!(screen.contains(text), "{text:?} in {screen}");
    }
    snapshot("repo", &mut h);
}

/// Agent `a1`'s repository at `now` (Unix seconds): on `main`, a commit
/// ahead of `origin/main`, with an untracked file and four events.
fn repo_view(now: i64) -> RepoView {
    RepoView {
        agent_id: "a1".into(),
        branch: Some("main".into()),
        head: "9984d116".into(),
        upstream: Some("origin/main".into()),
        ahead: 1,
        changes: vec![RepoChange {
            path: "src/repo.rs".into(),
            from: None,
            kind: ChangeKind::Untracked,
            staged: false,
            unstaged: true,
        }],
        events: repo_events(now),
        prs: PrState::Listed,
        ..RepoView::default()
    }
}

/// A pending commit, an open PR, a push and a merge, newest first.
fn repo_events(now: i64) -> Vec<RepoEvent> {
    let range = |from: &str, to: &str| RepoTarget::Range {
        from: Some(from.into()),
        to: to.into(),
    };
    vec![
        repo_event(
            EventKind::Commit,
            EventState::Pending,
            now - 600,
            "Add the REPO tab",
            "9984d116",
            RepoTarget::Commit {
                sha: "9984d116".into(),
            },
        ),
        repo_event(
            EventKind::Pr,
            EventState::Open,
            now - 3 * 3600,
            "#42 REPO tab and host icon",
            "repo-tab → main",
            RepoTarget::Pr { number: 42 },
        ),
        repo_event(
            EventKind::Push,
            EventState::Done,
            now - 5 * 3600,
            "Pushed 3 commits to origin/main",
            "5dc08c27",
            range("2b1c3d4e", "5dc08c27"),
        ),
        repo_event(
            EventKind::Merge,
            EventState::Done,
            now - 6 * 3600,
            "Merged channel-projects",
            "f5bc5cd4",
            range("edbd61b4", "f5bc5cd4"),
        ),
    ]
}

/// A REPO tab event at `at` (Unix seconds).
fn repo_event(
    kind: EventKind,
    state: EventState,
    at: i64,
    title: &str,
    detail: &str,
    target: RepoTarget,
) -> RepoEvent {
    RepoEvent {
        kind,
        state,
        at,
        title: title.into(),
        detail: detail.into(),
        target,
        url: None,
    }
}

#[test]
fn conversation() {
    let mut h = stage();
    session(&mut h);
    assert!(h.screen().contains("Here's what I found"));
    snapshot("conversation", &mut h);
}

#[test]
fn agent_working() {
    let mut h = stage();
    seed(&mut h);
    steps(&mut h);
    let todo = |content: &str, state| TodoView {
        content: content.into(),
        state,
        assignee: None,
        task: None,
    };
    h.apply([
        UiCommand::SetTodos(vec![
            todo("Read the failing test", TodoState::Completed),
            todo("Fix the parser", TodoState::InProgress),
            todo("Run the suite", TodoState::Pending),
        ]),
        UiCommand::AppendThinking("Checking the diff…".into()),
    ]);
    for _ in 0..3 {
        h.pass(TICK);
    }
    let screen = h.screen();
    assert!(screen.contains("Working on 3 to-dos"), "{screen}");
    snapshot("working", &mut h);
}

#[test]
fn slash_menu() {
    let mut h = stage();
    seed(&mut h);
    h.type_text("/");
    snapshot("slash", &mut h);
}

#[test]
fn model_picker() {
    let mut h = stage();
    session(&mut h);
    h.submit("/model");
    snapshot("models", &mut h);
}

#[test]
fn approval_request() {
    let mut h = stage();
    session(&mut h);
    h.apply([UiCommand::RequestApproval {
        id: "r1".into(),
        tool: "run_command".into(),
        description: "rm -rf target/".into(),
        always: "`rm` commands in this project".into(),
    }]);
    snapshot("approval", &mut h);
}
