//! The PLANS side tab: every plan in the active agent's workspace, newest
//! first, each a foldable row with its todos underneath. The plan being
//! worked on starts open, as does any plan with todos left; the rest start
//! folded until the user opens them. Scoped to an agent, it lists the
//! plans that agent's tasks work on (see [`App::refresh_plan_scope`]).

use std::collections::{HashMap, HashSet};

use super::processes::SIDE_PAGE;
use super::{contains, follow_selection, App, DetailView, SideScope};
use crate::app::file_view::FileView;
use crate::events::PlanView;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// Line ending the generated header of a plan file, as
/// `aura_exec_tools::plan_tools::HEADER_END` writes it (this crate does
/// not depend on the tools crate).
const PLAN_HEADER_END: &str = "<!-- end of generated header: edit below -->";

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
fn checklist_before(lines: &[&str], marker: usize) -> Vec<usize> {
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
fn todo_line(view: &FileView, index: usize, todo_count: usize, content: &str) -> Option<usize> {
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

/// Whether `line` is the frontmatter `id:` line of the todo with id `id`,
/// plain or YAML-quoted.
fn is_id_line(line: &str, id: &str) -> bool {
    let Some(value) = line.trim_start_matches(['-', ' ']).strip_prefix("id: ") else {
        return false;
    };
    value == id
        || ['\'', '"'].iter().any(|&quote| {
            value
                .strip_prefix(quote)
                .and_then(|rest| rest.strip_suffix(quote))
                == Some(id)
        })
}

/// Number of todos in the frontmatter: the top-level `- ` lines between
/// the opening and closing `---`. `None` without a closed frontmatter.
fn frontmatter_todo_count(lines: &[&str]) -> Option<usize> {
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return None;
    }
    let close = lines
        .iter()
        .skip(1)
        .position(|line| line.trim_end() == "---")?;
    Some(
        lines
            .iter()
            .skip(1)
            .take(close)
            .filter(|line| line.starts_with("- "))
            .count(),
    )
}

/// 0-based line of `view` showing the todo with id `id`: its checklist
/// item when the plan's checklist has one item per todo, else its `id:`
/// line in the frontmatter.
fn todo_id_line(view: &FileView, id: &str) -> Option<usize> {
    let lines = view.body.lines();
    let id_line = lines.iter().position(|line| is_id_line(line, id))?;
    let items = checklist_lines(&lines);
    if frontmatter_todo_count(&lines) != Some(items.len()) {
        return Some(id_line);
    }
    // Todos are the only top-level `- ` items in the frontmatter, so the
    // number before this one is its place in the checklist.
    let ordinal = lines
        .iter()
        .take(id_line)
        .filter(|line| line.starts_with("- "))
        .count();
    Some(items.get(ordinal).copied().unwrap_or(id_line))
}

/// One row of the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanRow {
    /// A plan's title row
    Plan { plan: usize, open: bool },
    /// A todo of an open plan
    Todo { plan: usize, todo: usize },
}

impl PlanRow {
    /// Index of the plan the row belongs to.
    #[must_use]
    pub const fn plan(self) -> usize {
        match self {
            Self::Plan { plan, .. } | Self::Todo { plan, .. } => plan,
        }
    }
}

/// The plans and how the tab shows them.
#[derive(Debug, Default)]
pub struct PlanList {
    plans: Vec<PlanView>,
    /// Plans the user folded (`false`) or opened (`true`), by path
    folds: HashMap<String, bool>,
    /// Path of the plan being worked on
    current: Option<String>,
    /// Paths of the plans listed besides the current one; `None` lists
    /// every plan
    filter: Option<HashSet<String>>,
    rows: Vec<PlanRow>,
    selected: usize,
}

impl PlanList {
    /// The plans, newest first, listed or not.
    #[must_use]
    pub fn plans(&self) -> &[PlanView] {
        &self.plans
    }

    /// Plans the filter leaves out.
    #[must_use]
    pub fn hidden(&self) -> usize {
        self.plans
            .iter()
            .filter(|plan| !self.is_shown(plan))
            .count()
    }

    /// List only the current plan and the plans at `paths`, or every
    /// plan when `None`.
    pub fn set_filter(&mut self, paths: Option<HashSet<String>>) {
        if paths == self.filter {
            return;
        }
        let keep = self.selected_key();
        self.filter = paths;
        self.rebuild(keep);
    }

    fn is_shown(&self, plan: &PlanView) -> bool {
        self.is_current(plan)
            || self
                .filter
                .as_ref()
                .is_none_or(|paths| paths.contains(&plan.path))
    }

    /// Rows on screen, in order.
    #[must_use]
    pub fn rows(&self) -> &[PlanRow] {
        &self.rows
    }

    /// Highlighted row.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Whether `plan` is the one being worked on.
    #[must_use]
    pub fn is_current(&self, plan: &PlanView) -> bool {
        self.current.as_deref() == Some(plan.path.as_str())
    }

    /// Replace the plans, keeping the highlight on the same plan or todo
    /// when it is still listed.
    pub fn set_plans(&mut self, plans: Vec<PlanView>) {
        let keep = self.selected_key();
        self.plans = plans;
        self.rebuild(keep);
    }

    /// Mark `plan` as the one being worked on, replacing its listed copy,
    /// or adding it on top when it isn't listed. `None` leaves the list
    /// as it is with no plan marked.
    pub fn set_current(&mut self, plan: Option<&PlanView>) {
        let keep = self.selected_key();
        self.current = plan.map(|plan| plan.path.clone());
        if let Some(plan) = plan {
            match self.plans.iter_mut().find(|p| p.path == plan.path) {
                Some(listed) => *listed = plan.clone(),
                None => self.plans.insert(0, plan.clone()),
            }
        }
        self.rebuild(keep);
    }

    /// Move the highlight by `delta` rows.
    pub fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }

    /// Highlight row `index` (clamped).
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    /// The plan of the highlighted row.
    #[must_use]
    pub fn selected_plan(&self) -> Option<&PlanView> {
        let row = self.rows.get(self.selected)?;
        self.plans.get(row.plan())
    }

    /// Fold or open the plan of the highlighted row. Folding from a todo
    /// moves the highlight to its plan.
    pub fn toggle_selected(&mut self) {
        let Some(&row) = self.rows.get(self.selected) else {
            return;
        };
        let plan = &self.plans[row.plan()];
        let open = !self.is_open(plan);
        self.folds.insert(plan.path.clone(), open);
        let keep = (plan.path.clone(), None);
        self.rebuild(Some(keep));
    }

    /// Open (`true`) or fold the plan of the highlighted row, when it is
    /// not that way already. Folding from a todo moves the highlight to
    /// its plan.
    pub fn set_selected_open(&mut self, open: bool) {
        let Some(&row) = self.rows.get(self.selected) else {
            return;
        };
        if self.is_open(&self.plans[row.plan()]) != open {
            self.toggle_selected();
        }
    }

    fn is_open(&self, plan: &PlanView) -> bool {
        self.folds
            .get(&plan.path)
            .copied()
            .unwrap_or_else(|| self.is_current(plan) || plan.has_open_todos())
    }

    /// Path and todo index of the highlighted row.
    fn selected_key(&self) -> Option<(String, Option<usize>)> {
        let row = *self.rows.get(self.selected)?;
        let path = self.plans.get(row.plan())?.path.clone();
        let todo = match row {
            PlanRow::Plan { .. } => None,
            PlanRow::Todo { todo, .. } => Some(todo),
        };
        Some((path, todo))
    }

    fn rebuild(&mut self, keep: Option<(String, Option<usize>)>) {
        let mut rows = Vec::new();
        for (index, plan) in self.plans.iter().enumerate() {
            if !self.is_shown(plan) {
                continue;
            }
            let open = self.is_open(plan);
            rows.push(PlanRow::Plan { plan: index, open });
            if open {
                rows.extend((0..plan.todos.len()).map(|todo| PlanRow::Todo { plan: index, todo }));
            }
        }
        self.rows = rows;
        let kept = keep.and_then(|(path, todo)| {
            let plan = self.plans.iter().position(|p| p.path == path)?;
            self.rows.iter().position(|&row| match (row, todo) {
                (PlanRow::Plan { plan: p, .. }, None) => p == plan,
                (PlanRow::Todo { plan: p, todo: t }, Some(todo)) => p == plan && t == todo,
                _ => false,
            })
        });
        self.select(kept.unwrap_or(self.selected));
    }
}

impl App {
    /// The PLANS tab's plans.
    #[must_use]
    pub const fn plan_list(&self) -> &PlanList {
        &self.plan_list
    }

    /// Keep the highlighted row inside `visible` rows, returning the first
    /// row to draw.
    pub fn sync_plans_scroll(&mut self, visible: usize) -> usize {
        follow_selection(
            self.areas.plans_scroll,
            self.plan_list.selected,
            visible,
            self.plan_list.rows.len(),
        )
    }

    /// Where the plan rows were drawn, for mouse clicks.
    pub fn set_plan_rows_area(&mut self, area: Rect, scroll: usize) {
        self.areas.plan_rows = Some(area);
        self.areas.plans_scroll = scroll;
    }

    /// Keys of the PLANS tab: Enter opens the plan, Right unfolds it and
    /// Left folds it.
    pub(super) fn handle_plans_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.plan_list.move_by(-1),
            KeyCode::Down => self.plan_list.move_by(1),
            KeyCode::PageUp => self.plan_list.move_by(-SIDE_PAGE),
            KeyCode::PageDown => self.plan_list.move_by(SIDE_PAGE),
            KeyCode::Home => self.plan_list.select(0),
            KeyCode::End => self.plan_list.select(usize::MAX),
            KeyCode::Enter => self.open_selected_plan(),
            KeyCode::Right => self.plan_list.set_selected_open(true),
            KeyCode::Left => self.plan_list.set_selected_open(false),
            _ => {}
        }
    }

    /// Open the highlighted row's plan file over the chat panel, with the
    /// cursor on the highlighted todo when there is one.
    pub(super) fn open_selected_plan(&mut self) {
        let (Some(plan), Some(root)) = (self.plan_list.selected_plan(), self.sandbox.root()) else {
            return;
        };
        let todo = match self.plan_list.rows.get(self.plan_list.selected) {
            Some(PlanRow::Todo { todo, .. }) => plan.todos.get(*todo).map(|found| (*todo, found)),
            _ => None,
        };
        let mut view = FileView::load(&root.join(&plan.path), Some(root));
        if let Some(line) =
            todo.and_then(|(index, todo)| todo_line(&view, index, plan.todos.len(), &todo.content))
        {
            view.jump_to(line);
        }
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Open the plan at workspace-relative `path` over the chat panel,
    /// with the cursor on its todo with id `todo` when there is one.
    pub(super) fn open_plan_at(&mut self, path: &str, todo: Option<&str>) {
        let Some(root) = self.sandbox.root() else {
            return;
        };
        let mut view = FileView::load(&root.join(path), Some(root));
        if let Some(line) = todo.and_then(|id| todo_id_line(&view, id)) {
            view.jump_to(line);
        }
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Show the task doing the highlighted todo in the TASKS tab.
    pub(super) fn jump_to_todo_task(&mut self) {
        let task = match (
            self.plan_list.selected_plan(),
            self.plan_list.rows.get(self.plan_list.selected),
        ) {
            (Some(plan), Some(PlanRow::Todo { todo, .. })) => {
                plan.todos.get(*todo).and_then(|todo| todo.task.clone())
            }
            _ => None,
        };
        let Some(task) = task else {
            self.notification = Some((
                "No task is doing this todo".to_string(),
                super::NotificationType::Warning,
            ));
            return;
        };
        self.show_side_tab(super::SidePanelTab::Tasks);
        let hidden = !self.task_list.select_task(&task)
            && self
                .task_list
                .tasks()
                .iter()
                .any(|listed| listed.id == task);
        if hidden {
            self.notification = Some((
                format!(
                    "Another agent is doing this todo; {} lists it",
                    SideScope::BUTTON
                ),
                super::NotificationType::Warning,
            ));
        }
    }

    /// Clicking a plan's fold glyph folds or opens it; clicking anywhere
    /// else on a row opens the plan, at the todo when one was clicked.
    pub(super) fn click_plan_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.plan_row_at(column, row) else {
            return;
        };
        self.plan_list.select(index);
        let on_fold = self.areas.plan_rows.is_some_and(|rows| {
            matches!(self.plan_list.rows[index], PlanRow::Plan { .. })
                && super::processes::on_row_icon(column, rows.x, 0)
        });
        if on_fold {
            self.plan_list.toggle_selected();
        } else {
            self.open_selected_plan();
        }
    }

    pub(super) fn plan_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .plan_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.plans_scroll + usize::from(row - rows.y);
        (index < self.plan_list.rows.len()).then_some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{TodoState, TodoView, UiCommand};

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

    fn shape(list: &PlanList) -> Vec<String> {
        list.rows()
            .iter()
            .map(|row| match *row {
                PlanRow::Plan { plan, open } => {
                    format!(
                        "{}{}",
                        if open { "-" } else { "+" },
                        list.plans()[plan].name
                    )
                }
                PlanRow::Todo { plan, todo } => {
                    format!("  {}", list.plans()[plan].todos[todo].content)
                }
            })
            .collect()
    }

    use TodoState::{Completed as C, InProgress as I, Pending as P};

    #[test]
    fn plans_with_open_todos_start_open_and_done_ones_folded() {
        let mut list = PlanList::default();
        list.set_plans(vec![plan("a", &[C, I]), plan("b", &[C, C])]);
        assert_eq!(shape(&list), ["-a", "  a 0", "  a 1", "+b"]);
    }

    #[test]
    fn the_current_plan_starts_open_even_when_done() {
        let mut list = PlanList::default();
        list.set_plans(vec![plan("a", &[P]), plan("b", &[C])]);
        list.set_current(Some(&plan("b", &[C])));
        assert_eq!(shape(&list), ["-a", "  a 0", "-b", "  b 0"]);
        assert!(list.is_current(&list.plans()[1]));
    }

    #[test]
    fn folding_from_a_todo_moves_the_highlight_to_its_plan_and_sticks() {
        let mut list = PlanList::default();
        list.set_plans(vec![plan("a", &[P, P]), plan("b", &[P])]);
        list.select(2);
        list.toggle_selected();
        assert_eq!(shape(&list), ["+a", "-b", "  b 0"]);
        assert_eq!(list.selected(), 0);

        list.set_plans(vec![plan("a", &[P, P, P]), plan("b", &[P])]);
        assert_eq!(
            shape(&list),
            ["+a", "-b", "  b 0"],
            "the fold survives updates"
        );
        list.toggle_selected();
        assert_eq!(shape(&list).len(), 6);
    }

    #[test]
    fn current_plan_updates_its_listed_copy_or_goes_on_top() {
        let mut list = PlanList::default();
        list.set_plans(vec![plan("a", &[P]), plan("b", &[P])]);
        list.select(2);
        list.set_current(Some(&plan("b", &[C, P])));
        assert_eq!(list.plans()[1].todos.len(), 2);
        assert_eq!(list.selected(), 2, "the highlight stays on b's plan row");

        list.set_current(Some(&plan("new", &[P])));
        assert_eq!(list.plans()[0].name, "new");
        list.set_current(None);
        assert_eq!(list.plans().len(), 3);
        assert!(list.plans().iter().all(|p| !list.is_current(p)));
    }

    #[test]
    fn set_plan_marks_the_listed_plan() {
        let mut app = App::new();
        app.process_command(UiCommand::SetPlans(vec![plan("a", &[P]), plan("b", &[C])]));
        app.process_command(UiCommand::SetPlan(Some(plan("b", &[C, P]))));
        let list = app.plan_list();
        assert!(list.is_current(&list.plans()[1]));
        assert_eq!(list.plans()[1].todos.len(), 2);
    }

    #[test]
    fn a_new_plan_opens_the_plans_tab_but_its_updates_do_not() {
        use super::super::SidePanelTab;

        let mut app = App::new();
        app.process_command(UiCommand::SetPlan(Some(plan("a", &[P, P]))));
        assert_eq!(app.side_tab(), SidePanelTab::Plans);

        app.show_side_tab(SidePanelTab::Sandbox);
        app.process_command(UiCommand::SetPlan(Some(plan("a", &[C, P]))));
        assert_eq!(app.side_tab(), SidePanelTab::Sandbox);

        app.process_command(UiCommand::SetPlan(Some(plan("b", &[P]))));
        assert_eq!(app.side_tab(), SidePanelTab::Plans);
    }

    #[test]
    fn enter_opens_the_plan_at_the_todo_and_arrows_fold() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
        std::fs::write(
            dir.path().join(".aura/plans/a.plan.md"),
            "---\nname: a\ntodos:\n- content: a 0\n---\n",
        )
        .unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        app.process_command(UiCommand::SetPlans(vec![plan("a", &[P])]));
        app.focus = super::super::PanelFocus::Sandbox;
        app.show_side_tab(super::super::SidePanelTab::Plans);

        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let Some(DetailView::File(view)) = app.detail_view() else {
            panic!("the plan opens as a file");
        };
        assert_eq!(view.vim.line(), 3, "the cursor lands on the todo");

        app.close_detail();
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(shape(app.plan_list()), ["+a"]);
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(shape(app.plan_list()), ["+a"], "Left keeps it folded");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(shape(app.plan_list()).len(), 2);
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        assert_eq!(shape(app.plan_list()), ["+a"], "Space folds");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
    }

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
        app.show_side_tab(super::super::SidePanelTab::Plans);

        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let Some(DetailView::File(view)) = app.detail_view() else {
            panic!("the plan opens as a file");
        };
        assert_eq!(
            view.vim.line(),
            17,
            "the cursor lands on the checklist item"
        );

        for (id, line) in [("x", 16), ("y", 17)] {
            app.close_detail();
            app.open_plan_at(".aura/plans/a.plan.md", Some(id));
            let Some(DetailView::File(view)) = app.detail_view() else {
                panic!("the plan opens as a file");
            };
            assert_eq!(view.vim.line(), line, "todo {id}");
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

    #[test]
    fn todo_id_lines_match_quoted_ids() {
        for line in ["- id: 1", "  id: '1'", "- id: \"1\""] {
            assert!(is_id_line(line, "1"), "{line}");
        }
        for line in ["- id: 12", "- id: '1", "- id: \"1'", "- content: 1"] {
            assert!(!is_id_line(line, "1"), "{line}");
        }
    }

    /// A plan file with a generated header in a temp sandbox, opened in
    /// the PLANS tab.
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
        app.show_side_tab(super::super::SidePanelTab::Plans);
        (app, dir)
    }

    fn cursor_line(app: &App) -> usize {
        let Some(DetailView::File(view)) = app.detail_view() else {
            panic!("the plan opens as a file");
        };
        view.vim.line()
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
        assert_eq!(cursor_line(&app), 17, "the second todo's item");

        for (id, line) in [("1", 16), ("2", 17)] {
            app.close_detail();
            app.open_plan_at(".aura/plans/a.plan.md", Some(id));
            assert_eq!(cursor_line(&app), line, "quoted id {id}");
        }
    }

    #[test]
    fn a_checklist_that_disagrees_with_the_todos_falls_back() {
        // A hand-edited header with one item for two todos.
        let text = format!(
            "---\nname: a\ntodos:\n\
             - id: x\n  content: first\n\
             - id: y\n  content: second\n---\n\n\
             ## Todos\n\n- [ ] second\n\n{PLAN_HEADER_END}\n"
        );
        let (mut app, _dir) = app_with_plan(&text, &["first", "second"]);
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(cursor_line(&app), 4, "first is only in the frontmatter");

        app.close_detail();
        app.open_plan_at(".aura/plans/a.plan.md", Some("x"));
        assert_eq!(cursor_line(&app), 3, "the id line, not the lone item");
    }

    #[test]
    fn t_on_a_todo_shows_the_task_doing_it() {
        use crate::events::{TaskState, TaskView};

        let mut app = App::new();
        let mut planned = plan("a", &[I]);
        planned.todos[0].task = Some("t1".to_string());
        app.process_command(UiCommand::SetPlans(vec![planned]));
        app.process_command(UiCommand::SetTasks(vec![
            TaskView {
                id: "t2".to_string(),
                ..TaskView::default()
            },
            TaskView {
                id: "t1".to_string(),
                state: TaskState::Running,
                ..TaskView::default()
            },
        ]));
        app.focus = super::super::PanelFocus::Sandbox;
        app.show_side_tab(super::super::SidePanelTab::Plans);

        app.handle_key(KeyEvent::from(KeyCode::Char('t')));
        assert_eq!(
            app.side_tab(),
            super::super::SidePanelTab::Plans,
            "a plan row has no task"
        );
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Char('t')));
        assert_eq!(app.side_tab(), super::super::SidePanelTab::Tasks);
        assert_eq!(
            app.task_list().selected_task().map(|t| t.id.as_str()),
            Some("t1")
        );
    }

    #[test]
    fn a_click_opens_a_plan_and_its_fold_glyph_folds_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
        std::fs::write(dir.path().join(".aura/plans/a.plan.md"), "a\n").unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        app.process_command(UiCommand::SetPlans(vec![plan("a", &[P])]));
        app.show_side_tab(super::super::SidePanelTab::Plans);
        app.set_plan_rows_area(Rect::new(10, 5, 30, 8), 0);

        app.click_plan_row(11, 5);
        assert!(app.detail_view().is_none());
        assert_eq!(shape(app.plan_list()), ["+a"]);
        app.click_plan_row(20, 5);
        assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
    }
}
