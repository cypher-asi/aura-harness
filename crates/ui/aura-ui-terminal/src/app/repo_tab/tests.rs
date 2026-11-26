use super::*;
use crate::app::ChatTab;
use crate::events::UiCommand;
use crate::repo::{
    ChangeKind, EventKind, EventState, RepoBranch, RepoChange, RepoEvent, RepoTarget, RepoWorktree,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use tokio::sync::mpsc;

fn event(kind: EventKind, at: i64, title: &str, target: RepoTarget) -> RepoEvent {
    RepoEvent {
        kind,
        state: EventState::Done,
        at,
        title: title.into(),
        detail: String::new(),
        target,
        url: None,
    }
}

fn commit(at: i64, title: &str, sha: &str) -> RepoEvent {
    event(
        EventKind::Commit,
        at,
        title,
        RepoTarget::Commit { sha: sha.into() },
    )
}

fn view(agent: &str) -> RepoView {
    let range = RepoTarget::Range {
        from: Some("aaaa1111".into()),
        to: "bbbb2222".into(),
    };
    RepoView {
        agent_id: agent.into(),
        root: PathBuf::from("/src/aura"),
        branch: Some("main".into()),
        head: "bbbb2222".into(),
        upstream: Some("origin/main".into()),
        ahead: 1,
        worktree: false,
        changes: vec![RepoChange {
            path: "src/lib.rs".into(),
            from: None,
            kind: ChangeKind::Modified,
            staged: false,
            unstaged: true,
        }],
        events: vec![
            commit(50, "Add REPO", "bbbb2222"),
            event(
                EventKind::Push,
                40,
                "Pushed 2 commits to origin/main",
                range,
            ),
            commit(30, "Start", "aaaa1111"),
        ],
        branches: vec![RepoBranch {
            name: "main".into(),
            current: true,
            upstream: Some("origin/main".into()),
            ahead: 1,
            behind: 0,
            gone: false,
            at: 50,
            subject: "Add REPO".into(),
        }],
        worktrees: vec![RepoWorktree {
            path: "/src/aura".into(),
            branch: Some("main".into()),
            head: "bbbb2222".into(),
            current: true,
            locked: false,
            prunable: false,
        }],
        ..RepoView::default()
    }
}

fn app() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new().with_event_sender(tx);
    app.active_agent_id = "aa".into();
    app.focus = PanelFocus::Sandbox;
    app.show_side_tab(SidePanelTab::Repo);
    (app, rx)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn opened_target(rx: &mut mpsc::Receiver<UiEvent>) -> Option<(RepoTarget, String)> {
    while let Ok(event) = rx.try_recv() {
        if let UiEvent::Repo {
            request: RepoRequest::Open { target, title },
            ..
        } = event
        {
            return Some((target, title));
        }
    }
    None
}

#[test]
fn filters_name_what_they_show() {
    assert_eq!(RepoFilter::named("PRs"), Some(RepoFilter::Prs));
    assert_eq!(RepoFilter::named("push"), Some(RepoFilter::Pushes));
    assert_eq!(RepoFilter::named("branch"), Some(RepoFilter::Branches));
    assert_eq!(RepoFilter::named("changes"), Some(RepoFilter::Changes));
    assert_eq!(RepoFilter::named("fetch"), Some(RepoFilter::Pulls));
    assert_eq!(RepoFilter::named("nope"), None);

    let view = view("aa");
    let shown: Vec<RepoFilter> = RepoFilter::ALL
        .into_iter()
        .filter(|filter| filter.has_any(&view))
        .collect();
    assert_eq!(
        shown,
        [
            RepoFilter::All,
            RepoFilter::Changes,
            RepoFilter::Commits,
            RepoFilter::Pushes,
            RepoFilter::Branches
        ],
        "one worktree is no list, and a repo off GitHub has no PRs"
    );
}

#[test]
fn all_lists_the_summary_changes_and_timeline_and_a_filter_narrows_it() {
    let (mut app, _rx) = app();
    app.process_command(UiCommand::SetRepo(Box::new(view("aa"))));
    assert_eq!(
        app.repo_rows(),
        [
            RepoRow::Summary,
            RepoRow::Changes,
            RepoRow::Change(0),
            RepoRow::Event(0),
            RepoRow::Event(1),
            RepoRow::Event(2),
        ]
    );
    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(app.repo_filter(), RepoFilter::Changes);
    assert_eq!(app.repo_rows(), [RepoRow::Summary, RepoRow::Change(0)]);
    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(app.repo_filter(), RepoFilter::Commits);
    assert_eq!(
        app.repo_rows(),
        [RepoRow::Summary, RepoRow::Event(0), RepoRow::Event(2)]
    );
    app.handle_key(key(KeyCode::Char('[')));
    app.handle_key(key(KeyCode::Char('[')));
    app.handle_key(key(KeyCode::Char('[')));
    assert_eq!(app.repo_filter(), RepoFilter::Branches, "it wraps around");
    assert_eq!(app.repo_rows(), [RepoRow::Branch(0)]);
}

#[test]
fn the_changes_fold_and_the_highlight_keeps_its_row() {
    let (mut app, _rx) = app();
    app.process_command(UiCommand::SetRepo(Box::new(view("aa"))));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.repo_rows()[app.repo_selected()], RepoRow::Change(0));
    app.handle_key(key(KeyCode::Left));
    assert!(app.repo_changes_folded());
    assert_eq!(app.repo_rows()[app.repo_selected()], RepoRow::Changes);
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(!app.repo_changes_folded());

    // A refresh with a new commit on top keeps the highlight on its push.
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Up));
    let mut newer = view("aa");
    newer.events.insert(0, commit(60, "Newer", "cccc3333"));
    app.process_command(UiCommand::SetRepo(Box::new(newer)));
    assert_eq!(app.repo_rows()[app.repo_selected()], RepoRow::Event(2));

    // One whose kind is gone gives way to All.
    app.set_repo_filter(RepoFilter::Pushes);
    let mut pushed_none = view("aa");
    pushed_none.events.retain(|e| e.kind != EventKind::Push);
    app.process_command(UiCommand::SetRepo(Box::new(pushed_none)));
    assert_eq!(app.repo_filter(), RepoFilter::All);
}

#[test]
fn enter_posts_the_rows_diff_and_brings_the_chat_forward() {
    let (mut app, mut rx) = app();
    app.process_command(UiCommand::SetRepo(Box::new(view("aa"))));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        opened_target(&mut rx),
        Some((
            RepoTarget::Unpushed,
            "main: 1 commit not on origin/main".into()
        ))
    );
    app.select_repo_row(3);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        opened_target(&mut rx),
        Some((
            RepoTarget::Commit {
                sha: "bbbb2222".into()
            },
            "bbbb2222 Add REPO".into()
        ))
    );
    assert_eq!(app.chat_tab(), ChatTab::Chat);

    app.set_repo_filter(RepoFilter::Branches);
    app.handle_key(key(KeyCode::Enter));
    assert!(
        opened_target(&mut rx).is_none(),
        "the branch checked out has no diff of its own"
    );
    assert!(app.notification().is_some());
}

#[test]
fn o_opens_a_prs_page_on_github_alone() {
    let (mut app, _rx) = app();
    let with_pr = |url: &str| {
        let mut view = view("aa");
        let pr = RepoTarget::Pr { number: 7 };
        view.events.insert(
            0,
            RepoEvent {
                url: Some(url.into()),
                ..event(EventKind::Pr, 60, "#7 Add REPO", pr)
            },
        );
        Box::new(view)
    };
    let page = "https://github.com/acme/aura/pull/7";
    app.process_command(UiCommand::SetRepo(with_pr(page)));
    let pr = app
        .repo_rows()
        .iter()
        .position(|&row| row == RepoRow::Event(0));
    app.select_repo_row(pr.unwrap());
    assert_eq!(app.selected_repo_page().as_deref(), Some(page));

    app.process_command(UiCommand::SetRepo(with_pr("file:///etc/passwd")));
    assert_eq!(app.repo_rows()[app.repo_selected()], RepoRow::Event(0));
    assert_eq!(app.selected_repo_page(), None);
    app.handle_key(key(KeyCode::Char('o')));
    assert!(app.notification().is_some(), "o says why it opens nothing");

    app.select_repo_row(0);
    assert_eq!(app.selected_repo_page(), None, "the sync row has no page");
}

#[test]
fn another_agents_view_is_not_shown_or_opened() {
    let (mut app, mut rx) = app();
    app.process_command(UiCommand::SetRepo(Box::new(view("bb"))));
    assert!(app.repo_view().is_none());
    assert!(app.repo_rows().is_empty());
    app.handle_key(key(KeyCode::Enter));
    assert!(opened_target(&mut rx).is_none());
}

#[test]
fn the_tab_asks_at_once_then_waits_for_its_answer() {
    let (mut app, mut rx) = app();
    app.panels.sandbox_visible = true;
    app.refresh_repo_tab();
    let asked = |rx: &mut mpsc::Receiver<UiEvent>| {
        let mut n = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(
                event,
                UiEvent::Repo {
                    request: RepoRequest::Load { .. },
                    ..
                }
            ) {
                n += 1;
            }
        }
        n
    };
    assert_eq!(asked(&mut rx), 1);
    app.refresh_repo_tab();
    assert_eq!(asked(&mut rx), 0, "the ask is out");
    let unanswered = Instant::now().checked_sub(ANSWER_WAIT).unwrap();
    app.repo.asked = Some(("aa".into(), unanswered));
    app.refresh_repo_tab();
    assert_eq!(asked(&mut rx), 1, "an ask never answered is made again");
    app.process_command(UiCommand::SetRepo(Box::new(view("aa"))));
    app.refresh_repo_tab();
    assert_eq!(asked(&mut rx), 0, "answered just now");
    let then = Instant::now().checked_sub(REFRESH_EVERY).unwrap();
    app.repo.asked = Some(("aa".into(), then));
    app.refresh_repo_tab();
    assert_eq!(asked(&mut rx), 1);
    app.handle_key(key(KeyCode::Char('r')));
    assert_eq!(asked(&mut rx), 1, "r asks at once");
}
