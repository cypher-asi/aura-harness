use super::*;
use crate::events::UiCommand;
use std::path::Path;

const PLAN: &str = "---\nname: Read plans\noverview: Show plans as documents.\ntodos:\n\
        - id: one\n  content: first\n  status: completed\n\
        - id: two\n  content: second\n  status: pending\n---\n\n\
        # Read plans\n\nShow plans as documents.\n\n## Todos\n\n- [x] first\n- [ ] second\n\n\
        <!-- end of generated header: edit below -->\n\n\
        ## Why\n\nPlans read like a diff in `src/a.rs` today.\n\n\
        ```rust\nfn main() {}\n```\n";

fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn a_plan_lays_out_as_a_document_without_its_source() {
    let doc = PlanDoc::parse(PLAN, Path::new("/x/a.plan.md"), ".aura/plans/a.plan.md");
    let theme = Theme::cyber();
    let laid = layout(&doc, 60, Measure::GRID, &theme);
    let rows = texts(&laid.lines);
    let all = rows.join("\n");
    assert_eq!(rows[0], "Read plans", "{all}");
    assert!(all.contains("Show plans as documents."), "{all}");
    assert!(all.contains("TODOS  1/2"), "{all}");
    assert!(all.contains("Why"), "{all}");
    assert!(all.contains("fn main() {}"), "{all}");
    for source in ["---", "name:", "<!--", "- [x]", "```"] {
        assert!(
            !rows.iter().any(|row| row.starts_with(source)),
            "{source} in {all}"
        );
    }
    let todos: Vec<&str> = laid
        .todo_rows
        .iter()
        .map(|&row| rows[row].as_str())
        .collect();
    assert_eq!(todos, ["☒ first", "☐ second"]);
    assert_eq!(laid.prose.len(), laid.lines.len());
    assert_eq!(laid.prose[0], Some(0), "the title is prose");
    assert_eq!(
        laid.prose[laid.todo_rows[0]],
        Some(TODO_INDENT),
        "todo text after its box"
    );
    let rule = rows.iter().position(|row| row.starts_with('─')).unwrap();
    assert_eq!(laid.prose[rule], None, "the rule stays on the grid");
    let code = rows.iter().position(|row| row.contains("fn main")).unwrap();
    assert_eq!(laid.prose[code], None, "code stays on the grid");
}

#[test]
fn titles_that_look_like_markdown_stay_text() {
    assert_eq!(escape_heading("# not a heading"), "\\# not a heading");
    assert_eq!(escape_heading("1. first"), "1\\. first");
    assert_eq!(escape_heading("2024 plans"), "2024 plans");
    assert_eq!(escape_heading("Plain"), "Plain");
}

fn screen(app: &mut App) -> String {
    screen_at(app, 120)
}

fn screen_at(app: &mut App, width: u16) -> String {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &Theme::cyber()))
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

fn app_with(text: &str) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
    std::fs::write(dir.path().join(".aura/plans/a.plan.md"), text).unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    (app, dir)
}

#[test]
fn an_open_plan_reads_as_a_document_with_prose_rows() {
    let (mut app, _dir) = app_with(PLAN);
    app.open_plan_at(".aura/plans/a.plan.md", None);
    let text = screen(&mut app);
    assert!(text.contains("PLAN Read plans"), "{text}");
    assert!(text.contains("1/2 TODOS"), "{text}");
    assert!(text.contains("☒ first"), "{text}");
    assert!(text.contains("Plans read like a diff"), "{text}");
    assert!(text.contains("s source"), "{text}");
    for source in ["name:", "<!--", "- [x]"] {
        assert!(!text.contains(source), "{source} shows: {text}");
    }
    assert!(
        app.prose_rows().len() >= 4,
        "title, overview, todos and body are prose"
    );
}

/// Every character as wide as a cell, but not on the grid.
#[allow(clippy::cast_precision_loss)]
fn narrow(text: &str, _: ratatui::style::Modifier) -> f32 {
    text.chars().count() as f32 * 0.8
}

#[test]
fn a_proportional_face_gets_the_plans_prose() {
    let (mut app, _dir) = app_with(PLAN);
    app.set_prose_measure(narrow);
    app.open_plan_at(".aura/plans/a.plan.md", None);
    screen(&mut app);
    let rows = app.prose_rows();
    assert!(rows.len() >= 4, "{rows:?}");
    assert!(
        rows.iter().any(|row| !row.code.is_empty()),
        "inline code stays monospace: {rows:?}"
    );
}

#[test]
fn a_plan_opened_at_a_todo_scrolls_to_it_once() {
    let todos: String = (0..60)
        .map(|i| format!("- id: t{i}\n  content: todo number {i}\n"))
        .collect();
    let (mut app, _dir) = app_with(&format!("---\nname: Long\ntodos:\n{todos}---\n"));
    app.open_plan_at(".aura/plans/a.plan.md", Some("t40"));
    let text = screen(&mut app);
    assert!(text.contains("todo number 40"), "{text}");
    assert!(!text.contains("todo number 10\n"), "{text}");
    let scroll = app.detail_scroll();
    assert!(scroll > 0);

    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Up,
    ));
    screen(&mut app);
    assert_eq!(app.detail_scroll(), scroll - 1, "scrolling is not undone");
}

#[test]
fn s_shows_the_plans_source() {
    let (mut app, _dir) = app_with(PLAN);
    app.open_plan_at(".aura/plans/a.plan.md", Some("two"));
    screen(&mut app);
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Char('s'),
    ));
    let Some(crate::app::DetailView::File(view)) = app.detail_view() else {
        panic!("s opens the file view");
    };
    assert_eq!(view.vim.line(), 19, "on the todo's checklist item");
}

#[test]
fn a_plans_text_sits_in_the_chats_column() {
    let long = "A paragraph long enough to wrap across the chat's whole column, \
                    so its rows reach the right edge of the text. "
        .repeat(6);
    let plan = format!("---\nname: Read plans\n---\n\n{long}\n");
    for width in [80, 200] {
        let (mut app, _dir) = app_with(&plan);
        app.add_message(crate::components::Message::new(
            crate::components::MessageRole::Assistant,
            &long,
        ));
        screen_at(&mut app, width);
        let chat = app.areas.chat_text.expect("the chat is drawn");

        app.open_plan_at(".aura/plans/a.plan.md", None);
        screen_at(&mut app, width);
        let rows = app.prose_rows();
        assert!(!rows.is_empty(), "width {width}");
        let left = rows.iter().map(|row| row.x).min().unwrap();
        let right = rows.iter().map(|row| row.x + row.width).max().unwrap();
        assert_eq!(left, chat.x, "starts where the chat does at width {width}");
        assert!(
            right <= chat.right(),
            "{right} past the chat's edge {} at width {width}",
            chat.right()
        );
        assert!(
            right + 12 >= chat.right(),
            "{right} fills the chat's column up to {} at width {width}",
            chat.right()
        );
    }
}
