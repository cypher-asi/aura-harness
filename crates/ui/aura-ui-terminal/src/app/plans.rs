//! Plan files as the TASKS side tab opens them: as a document over the
//! chat panel, at the todo the user picked. Their source, which `s` shows,
//! puts the cursor on that todo's item in the plan's generated `## Todos`
//! checklist when it has one, else on a line mentioning it.

use super::plan_doc::TodoFocus;
use super::tasks::TaskRow;
use super::App;
use crate::app::file_view::FileView;

/// Line ending the generated header of a plan file, as
/// `aura_exec_tools::plan_tools::HEADER_END` writes it (this crate does
/// not depend on the tools crate).
pub(super) const PLAN_HEADER_END: &str = "<!-- end of generated header: edit below -->";

/// Text of a generated checklist item `- [x] text`, or `None` when
/// `line` is not one.
fn checklist_text(line: &str) -> Option<&str> {
    let mut rest = line.strip_prefix("- [")?.chars();
    rest.next()?;
    rest.as_str().strip_prefix("] ")
}

/// 0-based lines of the items in a plan's generated `## Todos`
/// checklist, in todo order.
///
/// The checklist is the block just before the header's marker line:
/// checklist items, their 2-space continuation lines and blank lines, back
/// to the nearest `## Todos` heading. The first marker with such a block
/// wins. Empty for files without a generated header, so a `## Todos` the
/// author wrote in an old plan's body is not used. Callers check the
/// length against the plan's todo count before trusting it.
fn checklist_lines(lines: &[&str]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_end() == PLAN_HEADER_END)
        .map(|(marker, _)| checklist_before(lines, marker))
        .find(|items| !items.is_empty())
        .unwrap_or_default()
}

/// Item lines of the checklist ending at the marker on line `marker`, or
/// empty when the lines before it are not a `## Todos` checklist.
pub(super) fn checklist_before(lines: &[&str], marker: usize) -> Vec<usize> {
    let Some(heading) = (0..marker).rev().find(|&index| {
        let line = lines.get(index).copied().unwrap_or_default();
        !is_checklist_block_line(line)
    }) else {
        return Vec::new();
    };
    if lines.get(heading).map(|line| line.trim_end()) != Some("## Todos") {
        return Vec::new();
    }
    let mut items = Vec::new();
    for (index, line) in lines.iter().enumerate().take(marker).skip(heading + 1) {
        if checklist_text(line).is_some() {
            items.push(index);
        } else if items.is_empty() && !line.trim().is_empty() {
            // A continuation line with no item to continue.
            return Vec::new();
        }
    }
    items
}

/// Whether `line` can sit inside a generated checklist: an item, a
/// 2-space continuation line or a blank line.
fn is_checklist_block_line(line: &str) -> bool {
    line.trim().is_empty() || line.starts_with("  ") || checklist_text(line).is_some()
}

/// 0-based line of `view` showing todo number `index` of `todo_count`,
/// whose content is `content`: its checklist item when the plan's
/// checklist has one item per todo, else the checklist item or first line
/// mentioning the content's first line, so opening a plan from a todo
/// lands on it.
pub(super) fn todo_line(
    view: &FileView,
    index: usize,
    todo_count: usize,
    content: &str,
) -> Option<usize> {
    let lines = view.body.lines();
    let items = checklist_lines(&lines);
    if items.len() == todo_count {
        if let Some(&line) = items.get(index) {
            return Some(line);
        }
    }
    let needle = content.lines().next()?.trim();
    if needle.is_empty() {
        return None;
    }
    let assigned = format!("{needle} (@");
    items
        .into_iter()
        .find(|&index| {
            lines
                .get(index)
                .and_then(|line| checklist_text(line))
                .is_some_and(|text| text == needle || text.starts_with(&assigned))
        })
        .or_else(|| lines.iter().position(|line| line.contains(needle)))
}

impl App {
    /// Open the plan of the TASKS tab's highlighted plan header or todo
    /// as a document over the chat panel, at the todo when it is one.
    pub(super) fn open_selected_plan(&mut self) {
        let list = self.task_list();
        let (Some(plan), Some(root)) = (list.selected_plan(), self.sandbox.root()) else {
            return;
        };
        let file = root.join(&plan.path);
        // Owned, so the list's borrow of `self` ends before opening.
        let todo = match list.rows().get(list.selected()) {
            Some(&TaskRow::Todo { todo, .. }) => plan
                .todos
                .get(todo)
                .map(|found| (todo, plan.todos.len(), found.content.clone())),
            _ => None,
        };
        let focus = todo.as_ref().map(|(index, count, content)| TodoFocus::Nth {
            index: *index,
            count: *count,
            content,
        });
        self.open_plan_doc(&file, focus);
    }

    /// Open the plan at workspace-relative `path` as a document over the
    /// chat panel, at its todo with id `todo` when there is one.
    pub(crate) fn open_plan_at(&mut self, path: &str, todo: Option<&str>) {
        let Some(root) = self.sandbox.root() else {
            return;
        };
        let file = root.join(path);
        self.open_plan_doc(&file, todo.map(TodoFocus::Id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::DetailView;
    use crate::events::{PlanView, TodoState, TodoView, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent};

    fn plan(path: &str, states: &[TodoState]) -> PlanView {
        PlanView {
            path: format!(".aura/plans/{path}.plan.md"),
            name: path.to_string(),
            todos: states
                .iter()
                .enumerate()
                .map(|(i, &state)| TodoView {
                    content: format!("{path} {i}"),
                    state,
                    assignee: None,
                    task: None,
                })
                .collect(),
        }
    }

    use TodoState::Pending as P;

    #[test]
    fn plans_with_a_generated_header_open_at_the_checklist_item() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
        std::fs::write(
            dir.path().join(".aura/plans/a.plan.md"),
            format!(
                "---\nname: a\noverview: ''\ntodos:\n\
                 - id: x\n  content: a 0\n  status: pending\n\
                 - id: y\n  content: a 1\n  status: completed\n---\n\n\
                 # a\n\n## Todos\n\n- [ ] a 0\n- [x] a 1 (@bot)\n\n\
                 {PLAN_HEADER_END}\n\nBody mentions a 1.\n"
            ),
        )
        .unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        app.process_command(UiCommand::SetPlans(vec![plan("a", &[P, P])]));
        app.focus = super::super::PanelFocus::Sandbox;
        app.show_side_tab(super::super::SidePanelTab::Tasks);

        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(focus(&app), Some(1), "the plan opens at the todo");

        for (id, index) in [("x", 0), ("y", 1)] {
            app.close_detail();
            app.open_plan_at(".aura/plans/a.plan.md", Some(id));
            assert_eq!(focus(&app), Some(index), "todo {id}");
        }
    }

    #[test]
    fn the_header_marker_matches_the_one_plan_tools_writes() {
        // Must equal `aura_exec_tools::plan_tools::HEADER_END`, which has
        // the same literal assertion on its side.
        assert_eq!(
            PLAN_HEADER_END,
            "<!-- end of generated header: edit below -->"
        );
    }

    #[test]
    fn checklist_lines_takes_the_block_just_before_the_marker() {
        let cases: [(&str, &str, &[usize]); 6] = [
            (
                "plain",
                "# a\n\n## Todos\n\n- [ ] one\n- [x] two\n\n{M}\n\nBody",
                &[4, 5],
            ),
            (
                "continuation lines",
                "## Todos\n\n- [ ] one\n  more\n\n  after blank\n- [~] two\n\n{M}",
                &[2, 6],
            ),
            (
                "todos heading in the overview",
                "# a\n\n## Todos\n\nOverview text.\n\n## Todos\n\n- [ ] one\n\n{M}",
                &[8],
            ),
            (
                "marker in the overview is skipped",
                "# a\n\n{M}\n\n## Todos\n\n- [ ] one\n\n{M}\n\n- [ ] body",
                &[6],
            ),
            (
                "other text between checklist and marker",
                "## Todos\n\n- [ ] one\n\nNot a todo.\n\n{M}",
                &[],
            ),
            ("no marker", "## Todos\n\n- [ ] one\n", &[]),
        ];
        for (label, text, expected) in cases {
            let text = text.replace("{M}", PLAN_HEADER_END);
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(checklist_lines(&lines), expected, "{label}");
        }
    }

    /// A plan file with a generated header in a temp sandbox, opened in
    /// the TASKS tab.
    fn app_with_plan(text: &str, todos: &[&str]) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
        std::fs::write(dir.path().join(".aura/plans/a.plan.md"), text).unwrap();
        let mut view = plan("a", &vec![P; todos.len()]);
        for (todo, content) in view.todos.iter_mut().zip(todos) {
            todo.content = (*content).to_string();
        }
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        app.process_command(UiCommand::SetPlans(vec![view]));
        app.focus = super::super::PanelFocus::Sandbox;
        app.show_side_tab(super::super::SidePanelTab::Tasks);
        (app, dir)
    }

    /// The todo the open plan document is focused on.
    fn focus(app: &App) -> Option<usize> {
        let Some(DetailView::Plan(doc)) = app.detail_view() else {
            panic!("the plan opens as a document");
        };
        doc.focus
    }

    #[test]
    fn duplicate_todo_texts_open_at_their_own_checklist_item() {
        let text = format!(
            "---\nname: a\noverview: ''\ntodos:\n\
             - id: '1'\n  content: same\n  status: pending\n\
             - id: '2'\n  content: same\n  status: pending\n---\n\n\
             # a\n\n## Todos\n\n- [ ] same\n- [ ] same\n\n\
             {PLAN_HEADER_END}\n"
        );
        let (mut app, _dir) = app_with_plan(&text, &["same", "same"]);
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(focus(&app), Some(1), "the second todo");

        for (id, index) in [("1", 0), ("2", 1)] {
            app.close_detail();
            app.open_plan_at(".aura/plans/a.plan.md", Some(id));
            assert_eq!(focus(&app), Some(index), "quoted id {id}");
        }
    }

    #[test]
    fn a_checklist_that_disagrees_with_the_todos_is_not_trusted() {
        // A hand-edited header with one item for two todos: the
        // frontmatter's todos are the plan's.
        let text = format!(
            "---\nname: a\ntodos:\n\
             - id: x\n  content: first\n\
             - id: y\n  content: second\n---\n\n\
             ## Todos\n\n- [ ] second\n\n{PLAN_HEADER_END}\n"
        );
        let (mut app, _dir) = app_with_plan(&text, &["first", "second"]);
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(focus(&app), Some(0), "first is in the frontmatter");

        for (id, index) in [("x", 0), ("y", 1)] {
            app.close_detail();
            app.open_plan_at(".aura/plans/a.plan.md", Some(id));
            assert_eq!(focus(&app), Some(index), "todo {id}, not the lone item");
        }
    }
}
