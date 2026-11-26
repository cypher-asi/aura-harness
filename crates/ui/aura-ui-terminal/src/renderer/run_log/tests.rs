
use super::*;
use crate::events::{LogRunView, LogStepView, LogTurnView, UiCommand};
use ratatui::{backend::TestBackend, Terminal};
use std::time::Instant;

fn view() -> LogView {
    LogView {
        agent_id: String::new(),
        path: "/home/me/.aura/runs/1a-x/log.json".into(),
        json: "{}".into(),
        runs: vec![
            LogRunView {
                run_id: "1a-x".into(),
                title: "fix the parser".into(),
                detail: "sonnet-5-5 · 1 turn".into(),
                files: vec![("log".into(), "/home/me/.aura/runs/1a-x/log.json".into())],
                fields: vec![("model".into(), "claude/claude-sonnet-5-5".into())],
                turns: vec![LogTurnView {
                    turn: 1,
                    detail: "3 calls · end_turn".into(),
                    tokens: "2.2M tok".into(),
                    cost: "$1.45".into(),
                    took: "4m 02s".into(),
                    steps: vec![
                        LogStepView {
                            seq: 2,
                            kind: "model".into(),
                            label: "task @explore find the token bug".into(),
                            fields: vec![("model".into(), "m".into())],
                            sections: vec![LogSection {
                                title: "Thinking".into(),
                                body: "the parser drops the last token".into(),
                                code: false,
                            }],
                            spawned: vec![1],
                            tokens: "7k tok".into(),
                            cost: "$0.04".into(),
                            took: "6s".into(),
                            ..LogStepView::default()
                        },
                        LogStepView {
                            seq: 3,
                            kind: "model".into(),
                            label: "read_file src/parser.rs · run_command cargo test".into(),
                            tokens: "14k tok".into(),
                            cost: "$0.0091".into(),
                            took: "3s".into(),
                            ..LogStepView::default()
                        },
                        LogStepView {
                            seq: 4,
                            kind: "tool".into(),
                            label: "read_file src/parser.rs".into(),
                            summary: "340 lines".into(),
                            parent: Some(3),
                            took: "19ms".into(),
                            ..LogStepView::default()
                        },
                        LogStepView {
                            seq: 5,
                            kind: "tool".into(),
                            label: "run_command cargo test".into(),
                            summary: "exit 101 · 40 lines".into(),
                            parent: Some(3),
                            status: LogStatus::Error,
                            took: "2m 10s".into(),
                            ..LogStepView::default()
                        },
                    ],
                    ..LogTurnView::default()
                }],
                ..LogRunView::default()
            },
            LogRunView {
                run_id: "1b-y".into(),
                title: "find the token bug".into(),
                depth: 1,
                ..LogRunView::default()
            },
        ],
    }
}

fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    terminal
        .draw(|frame| render_run_log(frame, frame.area(), app, &Theme::default(), true))
        .unwrap();
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

/// The row of `screen` that holds `needle`.
fn row<'a>(screen: &'a str, needle: &str) -> &'a str {
    screen
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{screen}"))
}

/// The index of the header's rule on `screen`.
fn rule_row(screen: &str) -> usize {
    screen
        .lines()
        .position(|line| line.contains("│ ────────"))
        .unwrap_or_else(|| panic!("no rule:\n{screen}"))
}

#[test]
fn shows_the_run_the_copy_button_and_nested_subagents() {
    let mut app = App::new();
    app.process_command(UiCommand::LogView(Box::new(view())));
    app.run_log_mut().toggle(&LogPanel::Step("1a-x".into(), 2));
    let text = screen(&mut app);
    let top = row(&text, "fix the parser");
    assert!(top.contains("Copy log"), "{text}");
    assert!(top.contains('●'), "the run's status leads: {top}");
    let info = row(&text, "sonnet-5-5 · 1 turn");
    assert!(info.contains("/home/me/.aura/runs/1a-x/log.json"), "{text}");
    assert!(
        !text.contains("RUN  ") && !text.contains("SUBAGENT"),
        "{text}"
    );
    assert!(text.contains("Turn 1"), "{text}");
    assert!(text.contains("the parser drops the last token"), "{text}");
    let child = text
        .lines()
        .find(|l| l.contains("find the token bug") && !l.contains("task @explore"))
        .unwrap_or_else(|| panic!("the subagent's row:\n{text}"));
    assert!(child.contains('▸'), "{child}");

    let button = app.run_log().copy_button_area().expect("drawn");
    assert!(app.click_log(button.x + 1, button.y));
    let buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 1, 1));
    assert_eq!(app.take_copy_text(&buffer).as_deref(), Some("{}"));
    app.notify_copied(2, &Ok(()));
    assert!(
        screen(&mut app).contains("✓ Copied"),
        "the button says it copied"
    );
}

#[test]
fn steps_read_as_calls_and_results_with_their_columns_lined_up() {
    let mut app = App::new();
    app.process_command(UiCommand::LogView(Box::new(view())));
    let text = screen(&mut app);
    let model = row(&text, "read_file src/parser.rs · run_command cargo test");
    assert!(model.contains("model"), "{text}");
    let read = row(&text, "↳ read_file src/parser.rs  340 lines");
    let failed = row(&text, "↳ run_command cargo test  exit 101 · 40 lines");
    assert!(failed.contains('✗'), "a failure is marked: {failed}");
    for line in [model, read, failed] {
        assert!(!line.contains('✓') && !line.contains('▸'), "{line}");
    }
    let at = |line: &str, c: char| line.chars().position(|x| x == c);
    assert_eq!(
        at(failed, '✗'),
        at(row(&text, "Turn 1"), 'T').map(|t| t - 4),
        "the failure sits in the gutter, left of every row:\n{text}"
    );
    let end = |line: &str, cell: &str| {
        line.find(cell)
            .map(|at| line[..at].chars().count() + cell.chars().count())
    };
    let turn = row(&text, "Turn 1");
    assert!(turn.contains("3 calls · end_turn"), "{turn}");
    assert_eq!(
        end(model, "3s"),
        end(turn, "4m 02s"),
        "times line up:\n{text}"
    );
    assert_eq!(end(read, "19ms"), end(model, "3s"), "{text}");
    assert_eq!(end(model, "$0.0091"), end(turn, "$1.45"), "{text}");
    assert_eq!(end(model, "14k tok"), end(turn, "2.2M tok"), "{text}");
    let heads = row(&text, "tokens");
    assert_eq!(end(heads, "time"), end(turn, "4m 02s"), "{text}");
    assert_eq!(end(heads, "cost"), end(turn, "$1.45"), "{text}");
}

#[test]
fn the_details_start_shut_and_hold_the_files() {
    let mut app = App::new();
    app.process_command(UiCommand::LogView(Box::new(view())));
    let text = screen(&mut app);
    assert!(row(&text, "Details").contains('▸'), "{text}");
    assert!(!text.contains("claude/claude-sonnet-5-5"), "{text}");
    app.run_log_mut().toggle(&LogPanel::Details("1a-x".into()));
    let text = screen(&mut app);
    assert!(text.contains("claude/claude-sonnet-5-5"), "{text}");
    assert!(
        row(&text, "log  /home").contains("/home/me/.aura/runs/1a-x/log.json"),
        "{text}"
    );
}

#[test]
fn a_long_title_is_cut_so_the_header_keeps_three_lines() {
    let mut log = view();
    log.runs[0].title = "remember the tab selected with each agent, and default to the chat \
            tab when none was selected before, in the terminal and the desktop app"
        .into();
    let mut app = App::new();
    app.process_command(UiCommand::LogView(Box::new(log)));
    let text = screen(&mut app);
    let lines: Vec<&str> = text.lines().collect();
    let top = lines.iter().position(|l| l.contains("Copy log")).unwrap();
    assert!(lines[top].contains('…'), "{text}");
    assert!(!text.contains("the desktop app"), "{text}");
    assert!(lines[top + 1].contains("log.json"), "{text}");
    assert_eq!(rule_row(&text), top + 2, "{text}");
}

#[test]
fn a_notice_comes_alone_and_fades_in() {
    let mut app = App::new();
    app.process_command(UiCommand::LogNotice {
        agent_id: String::new(),
        notice: "This agent has no run log yet; send it a prompt".into(),
    });
    let text = screen(&mut app);
    assert!(!text.contains("Run log"), "{text}");
    assert!(!text.contains("esc back"), "no keys without a log: {text}");
    assert!(app.run_log().fading().is_some());
}

#[test]
fn coming_back_draws_neither_the_old_log_nor_an_old_notice() {
    let mut app = App::new();
    app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Log));
    app.process_command(UiCommand::LogView(Box::new(view())));
    app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Chat));
    app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Log));
    let text = screen(&mut app);
    assert!(!text.contains("fix the parser"), "{text}");
    assert!(!text.contains("Copy log"), "{text}");

    app.process_command(UiCommand::LogNotice {
        agent_id: String::new(),
        notice: "This agent has no run log yet; send it a prompt".into(),
    });
    app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Chat));
    app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Log));
    let text = screen(&mut app);
    assert!(!text.contains("no run log yet"), "{text}");
}

#[test]
fn says_why_there_is_no_log() {
    let mut app = App::new();
    app.process_command(UiCommand::LogNotice {
        agent_id: String::new(),
        notice: "This agent has no run log yet; send it a prompt".into(),
    });
    let text = screen(&mut app);
    assert!(text.contains("no run log yet"), "{text}");
    assert!(text.contains("r to retry"), "{text}");
    assert!(!text.contains("Copy log"));
}

#[test]
fn nothing_is_drawn_before_the_log_arrives() {
    let mut app = App::new();
    app.run_log_mut().loading_since = Instant::now().checked_sub(std::time::Duration::from_secs(5));
    let text = screen(&mut app);
    let inside: String = text
        .lines()
        .skip(1)
        .take(28)
        .map(|line| line.trim_matches(|c: char| c == '│' || c.is_whitespace()))
        .collect();
    assert!(
        inside.is_empty(),
        "a slow load still draws nothing:\n{text}"
    );

    app.process_command(UiCommand::LogView(Box::new(view())));
    let text = screen(&mut app);
    assert!(text.contains("fix the parser"), "{text}");
    assert!(app.run_log().fading().is_some(), "the log fades in");
}

#[test]
fn the_log_of_another_agent_is_never_drawn() {
    let mut app = App::new();
    app.run_log_mut().view = Some(LogView {
        agent_id: "someone-else".into(),
        ..view()
    });
    let text = screen(&mut app);
    assert!(!text.contains("fix the parser"), "{text}");
    assert!(!text.contains("Copy log"), "{text}");
    assert!(app.run_log().copy_button_area().is_none());
}
