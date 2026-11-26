use super::strip::{pieces, window, Piece};
use super::*;
use crate::app::{PaneTab, SidePanelTab};
use crate::events::UiCommand;
use crate::repo::{
    ChangeKind, EventKind, EventState, RepoBranch, RepoChange, RepoEvent, RepoTarget, RepoView,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, Terminal};

fn screen(app: &mut App, width: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
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

fn row_with(screen: &[String], text: &str) -> usize {
    screen
        .iter()
        .position(|row| row.contains(text))
        .unwrap_or_else(|| panic!("no {text} in:\n{}", screen.join("\n")))
}

/// The column `text` starts at in `row`, whose cells are one char each.
fn column_of(row: &str, text: &str) -> u16 {
    let at = row
        .find(text)
        .unwrap_or_else(|| panic!("no {text} in {row}"));
    u16::try_from(row[..at].chars().count()).unwrap()
}

fn click(app: &mut App, column: u16, row: usize) {
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row: u16::try_from(row).unwrap(),
            modifiers: KeyModifiers::empty(),
        });
    }
}

fn repo() -> RepoView {
    RepoView {
        agent_id: "aa".into(),
        branch: Some("main".into()),
        head: "bbbb2222".into(),
        upstream: Some("origin/main".into()),
        ahead: 1,
        changes: vec![RepoChange {
            path: "src/lib.rs".into(),
            from: None,
            kind: ChangeKind::Modified,
            staged: true,
            unstaged: false,
        }],
        events: events(),
        branches: vec![RepoBranch {
            name: "main".into(),
            current: true,
            upstream: Some("origin/main".into()),
            ahead: 1,
            behind: 0,
            gone: false,
            at: 50,
            subject: "Add the REPO tab".into(),
        }],
        ..RepoView::default()
    }
}

/// The timeline of [`repo`], newest first.
fn events() -> Vec<RepoEvent> {
    let event = |kind, state, at, title: &str, target| RepoEvent {
        kind,
        state,
        at,
        title: title.into(),
        detail: String::new(),
        target,
        url: None,
    };
    let commit = |sha: &str| RepoTarget::Commit { sha: sha.into() };
    let pushed = RepoTarget::Range {
        from: None,
        to: "aaaa1111".into(),
    };
    vec![
        event(
            EventKind::Commit,
            EventState::Pending,
            50,
            "Add the REPO tab",
            commit("b"),
        ),
        event(
            EventKind::Push,
            EventState::Done,
            40,
            "Pushed 2 commits",
            pushed,
        ),
        event(
            EventKind::Commit,
            EventState::Done,
            30,
            "Start",
            commit("a"),
        ),
    ]
}

fn repo_app() -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent("aa".into()));
    app.process_command(UiCommand::SetRepo(Box::new(repo())));
    app.select_pane_tab(PaneTab::Side(SidePanelTab::Repo));
    app
}

#[test]
fn the_tab_lists_the_sync_the_changes_and_the_timeline_under_its_strip() {
    let mut app = repo_app();
    let rows = screen(&mut app, 220);
    let strip = row_with(&rows, " All  Changes  Commits  Pushes │ Branches ");
    for (offset, parts) in [
        (1, &["↑", "main", "1 ahead of origin/main", "bbbb2222"][..]),
        (2, &["▾", "Uncommitted changes", "1 file"]),
        (3, &["M  src/lib.rs", "staged"]),
        (4, &["○", "Add the REPO tab"]),
        (5, &["↑", "Pushed 2 commits"]),
        (6, &["●", "Start"]),
    ] {
        let row = &rows[strip + offset];
        assert!(
            parts.iter().all(|part| row.contains(part)),
            "{parts:?} in {row}"
        );
    }

    let changes = column_of(&rows[strip + 2], "▾");
    click(&mut app, changes, strip + 2);
    let rows = screen(&mut app, 220);
    assert!(!rows.join("\n").contains("src/lib.rs"), "folded");
    assert!(rows[strip + 2].contains('▸'), "{}", rows[strip + 2]);

    click(&mut app, column_of(&rows[strip], "Commits"), strip);
    assert_eq!(app.repo_filter(), RepoFilter::Commits);
    let rows = screen(&mut app, 220);
    assert!(
        rows[strip + 2].contains("Add the REPO tab"),
        "{}",
        rows[strip + 2]
    );
    assert!(rows[strip + 3].contains("Start"), "{}", rows[strip + 3]);
    assert!(!rows.join("\n").contains("Pushed"));
}

#[test]
fn a_narrow_strip_is_cut_around_the_picked_filter() {
    let mut app = repo_app();
    let rows = screen(&mut app, 150);
    let strip = row_with(&rows, " All  Changes ");
    assert!(rows[strip].contains('›'), "{}", rows[strip]);
    assert!(!rows[strip].contains("Branches"), "{}", rows[strip]);

    for _ in 0..4 {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    }
    assert_eq!(app.repo_filter(), RepoFilter::Branches);
    let rows = screen(&mut app, 150);
    assert!(
        rows[strip].contains("‹") && rows[strip].contains("Branches"),
        "{}",
        rows[strip]
    );
    assert!(
        rows[strip + 1].contains("main"),
        "the branches alone: {}",
        rows[strip + 1]
    );
    click(&mut app, column_of(&rows[strip], "‹"), strip);
    assert_eq!(app.repo_filter(), RepoFilter::Pushes);
}

const LISTED: [RepoFilter; 6] = [
    RepoFilter::All,
    RepoFilter::Changes,
    RepoFilter::Commits,
    RepoFilter::Pushes,
    RepoFilter::Branches,
    RepoFilter::Tags,
];

#[test]
fn the_lists_sit_past_a_bar() {
    let pieces = pieces(&LISTED);
    assert_eq!(pieces[4], Piece::Bar);
    assert_eq!(pieces[5], Piece::Filter(RepoFilter::Branches));
    assert_eq!(pieces.len(), 7);
    assert!(!super::strip::pieces(&LISTED[..4]).contains(&Piece::Bar));
    assert_eq!(
        super::strip::pieces(&[RepoFilter::Branches]),
        [Piece::Filter(RepoFilter::Branches)],
        "no bar before the first piece"
    );
}

#[test]
fn a_strip_too_wide_keeps_the_picked_filter_in_view() {
    // " All ", " Changes ", " Commits ", " Pushes ", "│", " Branches ",
    // " Tags ": 48 columns in all.
    let widths: Vec<u16> = pieces(&LISTED).iter().map(|p| p.width()).collect();
    assert_eq!(widths.iter().sum::<u16>(), 48);
    assert_eq!(window(&widths, 0, 48), 0..7);
    assert_eq!(window(&widths, 0, 30), 0..3, "28 columns left");
    assert_eq!(window(&widths, 6, 30), 3..7);
    assert_eq!(window(&widths, 3, 20), 2..5, "from Commits to the bar");
    assert_eq!(window(&widths, 1, 5), 1..2, "one too wide still shows");
}
