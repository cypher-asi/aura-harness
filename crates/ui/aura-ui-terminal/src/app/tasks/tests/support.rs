use super::*;

pub(super) fn switched(rx: &mut mpsc::Receiver<UiEvent>) -> Option<String> {
    match rx.try_recv() {
        Ok(UiEvent::SelectAgent(id)) => Some(id),
        _ => None,
    }
}

pub(super) fn notice(app: &App) -> Option<&str> {
    app.notification().map(|(text, _)| text.as_str())
}

pub(super) fn item(content: &str, state: TodoState, task: Option<&str>) -> TodoView {
    TodoView {
        content: content.to_string(),
        state,
        assignee: None,
        task: task.map(str::to_string),
    }
}

/// Request `id` of agent `agent`, with `items`.
pub(super) fn request(id: &str, agent: &str, items: Vec<TodoView>) -> TaskView {
    TaskView {
        id: id.to_string(),
        title: format!("asked {id}"),
        state: R,
        agent_id: agent.to_string(),
        agent_name: agent.to_uppercase(),
        items,
        ..TaskView::default()
    }
}

/// Task `id` handed out by `parent`, with an item of its own the tab
/// never lists.
pub(super) fn part(id: &str, parent: &str, state: TaskState) -> TaskView {
    TaskView {
        id: id.to_string(),
        title: format!("do {id}"),
        state,
        agent_id: format!("agent-{id}"),
        agent_name: format!("Agent {id}"),
        assignee: Some("coder".to_string()),
        parent: Some(parent.to_string()),
        items: vec![item(&format!("inside {id}"), Todo, None)],
        ..TaskView::default()
    }
}

pub(super) fn path(name: &str) -> String {
    format!(".aura/plans/{name}.plan.md")
}

pub(super) fn plan(name: &str, states: &[TodoState]) -> PlanView {
    PlanView {
        path: path(name),
        name: name.to_string(),
        todos: states
            .iter()
            .enumerate()
            .map(|(i, &state)| item(&format!("{name} {i}"), state, None))
            .collect(),
    }
}

/// The rows as text, indented two spaces per level: `-`/`+` before
/// what is open/folded, `#` before a todo, `~` before a round's first
/// request, `@` before a task; items are their content.
pub(super) fn shape(list: &TaskList) -> Vec<String> {
    list.rows()
        .iter()
        .map(|&row| {
            let fold = match row.fold() {
                Some(true) => "-",
                Some(false) => "+",
                None => "",
            };
            let text = match row {
                TaskRow::Plan { plan, .. } => plan
                    .map_or("No Plan", |plan| list.plans()[plan].name.as_str())
                    .to_string(),
                TaskRow::Todo { plan, todo, .. } => {
                    format!("#{}", list.plans()[plan].todos[todo].content)
                }
                TaskRow::Item { task, item, .. } => list.tasks()[task].items[item].content.clone(),
                TaskRow::Round { first, .. } => format!("~{}", list.tasks()[first].id),
                TaskRow::Task { task, .. } => format!("@{}", list.tasks()[task].id),
            };
            let text = match row {
                TaskRow::Todo { .. } => format!("#{fold}{}", &text[1..]),
                TaskRow::Round { .. } => format!("~{fold}{}", &text[1..]),
                _ => format!("{fold}{text}"),
            };
            format!("{}{text}", "  ".repeat(row.depth()))
        })
        .collect()
}

pub(super) fn list_of(plans: Vec<PlanView>, tasks: Vec<TaskView>) -> TaskList {
    let mut list = TaskList::default();
    list.set_plans(plans);
    list.set_tasks(tasks);
    list
}

/// An app on the focused TASKS tab listing `plans` and `tasks`.
pub(super) fn app_on_tasks(plans: Vec<PlanView>, tasks: Vec<TaskView>) -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetPlans(plans));
    app.process_command(UiCommand::SetTasks(tasks));
    app.focus = PanelFocus::Sandbox;
    app.show_side_tab(SidePanelTab::Tasks);
    app
}

pub(super) fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::from(code));
}

pub(super) fn selected_shape(app: &App) -> String {
    shape(app.task_list())[app.task_list().selected()].clone()
}
