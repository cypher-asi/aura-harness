//! The PLANS side tab: every plan in the active agent's workspace, newest
//! first, each a foldable row with its todos underneath. The plan being
//! worked on starts open, as does any plan with todos left; the rest start
//! folded until the user opens them.

use std::collections::HashMap;

use super::{contains, follow_selection, App, DetailView};
use crate::app::file_view::FileView;
use crate::events::PlanView;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

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
    rows: Vec<PlanRow>,
    selected: usize,
}

impl PlanList {
    /// The plans, newest first.
    #[must_use]
    pub fn plans(&self) -> &[PlanView] {
        &self.plans
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

    /// Keys of the PLANS tab.
    pub(super) fn handle_plans_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.plan_list.move_by(-1),
            KeyCode::Down => self.plan_list.move_by(1),
            KeyCode::PageUp => self.plan_list.move_by(-10),
            KeyCode::PageDown => self.plan_list.move_by(10),
            KeyCode::Home => self.plan_list.select(0),
            KeyCode::End => self.plan_list.select(usize::MAX),
            KeyCode::Enter => self.activate_selected_plan_row(),
            KeyCode::Left => self.show_side_tab(self.side_tab.prev()),
            KeyCode::Right => self.show_side_tab(self.side_tab.next()),
            _ => {}
        }
    }

    /// A plan row folds or opens; a todo opens its plan's file.
    fn activate_selected_plan_row(&mut self) {
        match self.plan_list.rows.get(self.plan_list.selected) {
            Some(PlanRow::Plan { .. }) => self.plan_list.toggle_selected(),
            Some(PlanRow::Todo { .. }) => self.open_selected_plan(),
            None => {}
        }
    }

    fn open_selected_plan(&mut self) {
        let (Some(plan), Some(root)) = (self.plan_list.selected_plan(), self.sandbox.root()) else {
            return;
        };
        let view = FileView::load(&root.join(&plan.path), Some(root));
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Clicking a plan folds or opens it; clicking a todo highlights it,
    /// and clicking the highlighted todo opens its plan.
    pub(super) fn click_plan_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.plan_row_at(column, row) else {
            return;
        };
        let again = index == self.plan_list.selected;
        self.plan_list.select(index);
        match self.plan_list.rows[index] {
            PlanRow::Plan { .. } => self.plan_list.toggle_selected(),
            PlanRow::Todo { .. } if again => self.open_selected_plan(),
            PlanRow::Todo { .. } => {}
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
    fn enter_folds_plans_and_opens_the_plan_file_from_a_todo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
        std::fs::write(
            dir.path().join(".aura/plans/a.plan.md"),
            "---\nname: a\n---\n",
        )
        .unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        app.process_command(UiCommand::SetPlans(vec![plan("a", &[P])]));
        app.focus = super::super::PanelFocus::Sandbox;
        app.show_side_tab(super::super::SidePanelTab::Plans);

        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.detail_view(), Some(DetailView::File(_))));

        app.close_detail();
        app.handle_key(KeyEvent::from(KeyCode::Up));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(shape(app.plan_list()), ["+a"]);
    }
}
