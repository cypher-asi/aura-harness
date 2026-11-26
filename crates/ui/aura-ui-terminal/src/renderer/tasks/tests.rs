use super::*;
use crate::events::{AgentSummary, TodoState, UiCommand};
use ratatui::{backend::TestBackend, Terminal};

/// The buffer after drawing the TASKS tab into a 60-column frame
/// `height` rows tall.
fn buffer_of(app: &mut App, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(60, height)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            render_tasks(frame, area, area, app, &Theme::default());
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text_of(buffer: &ratatui::buffer::Buffer) -> String {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The screen after drawing the TASKS tab into a 60x4 frame.
fn drawn(app: &mut App) -> String {
    text_of(&buffer_of(app, 4))
}

#[test]
fn plans_of_other_agents_only_say_so_when_no_task_is_hidden() {
    let mut app = App::new();
    let agent = |id: &str| AgentSummary {
        id: id.into(),
        name: id.to_uppercase(),
        project_id: Some("site".into()),
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetAgents(vec![agent("a"), agent("b")]));
    app.process_command(UiCommand::SetActiveAgent("a".into()));
    app.process_command(UiCommand::SetPlans(vec![PlanView {
        path: ".aura/plans/b.plan.md".into(),
        name: "b".into(),
        todos: vec![TodoView {
            content: "step".into(),
            state: TodoState::Pending,
            assignee: None,
            task: Some("tb".into()),
        }],
    }]));
    assert!(app.task_list().rows().is_empty());
    assert_eq!(app.task_list().hidden(), 0);
    assert_eq!(app.task_list().hidden_plans(), 1);

    let screen = drawn(&mut app);
    assert!(screen.contains("No plans for this agent"), "{screen}");
    assert!(!screen.contains("No tasks yet."), "{screen}");
}

/// A Lead's request whose item a Coder's task does.
fn coder_tasks() -> Vec<TaskView> {
    vec![
        TaskView {
            id: "part".into(),
            title: "Parse".into(),
            state: TaskState::Completed,
            agent_name: "Coder".into(),
            assignee: Some("coder".into()),
            parent: Some("request".into()),
            items: vec![TodoView {
                content: "lex".into(),
                state: TodoState::Pending,
                assignee: None,
                task: None,
            }],
            ..TaskView::default()
        },
        TaskView {
            id: "request".into(),
            title: "Build it".into(),
            agent_name: "Lead".into(),
            items: vec![TodoView {
                content: "Write the parser".into(),
                state: TodoState::InProgress,
                assignee: Some("coder".into()),
                task: Some("part".into()),
            }],
            ..TaskView::default()
        },
    ]
}

#[test]
fn row_icons_sit_under_the_headers_arrow_and_tasks_one_level_in() {
    let mut app = App::new();
    app.process_command(UiCommand::SetTasks(coder_tasks()));
    let screen = drawn(&mut app);
    let lines: Vec<&str> = screen.lines().collect();
    let column = |line: &str, glyph: char| {
        line.chars()
            .position(|c| c == glyph)
            .unwrap_or_else(|| panic!("no {glyph} in:\n{screen}"))
    };
    assert!(lines[0].contains("No Plan"), "{screen}");
    assert!(lines[1].contains("Write the parser  → coder"), "{screen}");
    assert!(
        lines[2].contains("Write the parser  → coder"),
        "a task is titled by the item it does: {screen}"
    );
    assert!(!screen.contains("Parse "), "{screen}");
    assert_eq!(column(lines[1], '▣'), column(lines[0], '▾'), "{screen}");
    assert_eq!(
        column(lines[2], '✓').saturating_sub(column(lines[0], '▾')),
        2,
        "{screen}"
    );
    assert!(
        !screen.contains("Build it"),
        "the request has no row: {screen}"
    );
    assert!(!screen.contains("Lead"), "{screen}");
    assert!(
        !screen.contains("lex"),
        "a task's own list is not listed: {screen}"
    );
}

const ROUND_PLAN: &str = ".aura/plans/ch.plan.md";

fn round_todo(content: &str, state: TodoState, task: Option<&str>) -> TodoView {
    TodoView {
        content: content.into(),
        state,
        assignee: None,
        task: task.map(Into::into),
    }
}

fn failed_try(id: &str) -> TaskView {
    TaskView {
        id: id.into(),
        title: format!("Work ONLY in the worktree {id}"),
        state: TaskState::Failed,
        agent_id: format!("agent-{id}"),
        assignee: Some("coder".into()),
        parent: Some("b1".into()),
        plan: Some(ROUND_PLAN.into()),
        plan_todo: Some("a".into()),
        ..TaskView::default()
    }
}

fn round_build(id: &str, items: Vec<TodoView>) -> TaskView {
    TaskView {
        id: id.into(),
        title: format!("asked {id}"),
        agent_id: "lead".into(),
        plan: Some(ROUND_PLAN.into()),
        builds: true,
        items,
        ..TaskView::default()
    }
}

/// Two rounds of the plan: `b1` with two failed tries, then `b2`.
fn two_rounds() -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetPlans(vec![PlanView {
        path: ROUND_PLAN.into(),
        name: "ch".into(),
        todos: vec![round_todo("new", TodoState::Pending, None)],
    }]));
    app.process_command(UiCommand::SetTasks(vec![
        round_build("b2", vec![round_todo("new", TodoState::Pending, None)]),
        failed_try("t2"),
        failed_try("t1"),
        round_build(
            "b1",
            vec![round_todo(
                "Add the store",
                TodoState::InProgress,
                Some("t2"),
            )],
        ),
    ]));
    app
}

#[test]
fn an_earlier_round_lists_its_items_and_a_replaced_failure_is_muted() {
    let mut app = two_rounds();
    let buffer = buffer_of(&mut app, 6);
    let screen = text_of(&buffer);
    let lines: Vec<&str> = screen.lines().collect();
    assert!(
        lines[2].contains("asked b1") && lines[2].contains("0/1"),
        "{screen}"
    );
    assert!(lines[3].contains("▣  Add the store"), "{screen}");
    assert!(lines[4].contains("✗  Add the store  → coder"), "{screen}");
    assert!(lines[5].contains("✗  Add the store  → coder"), "{screen}");
    assert!(!screen.contains("Work ONLY"), "{screen}");

    let theme = Theme::default();
    let icon_color = |y: u16| {
        (0..buffer.area.width)
            .map(|x| &buffer[(x, y)])
            .find(|cell| cell.symbol() == "✗")
            .map_or_else(|| panic!("no ✗ on row {y}:\n{screen}"), |cell| cell.fg)
    };
    assert_eq!(icon_color(4), theme.colors.muted, "t1 was replaced by t2");
    assert_eq!(icon_color(5), theme.colors.error, "t2 is the newest try");
}
