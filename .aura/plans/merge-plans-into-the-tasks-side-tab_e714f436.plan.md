---
name: Merge PLANS into the TASKS side tab
overview: Drop the separate PLANS tab in the bottom-right side panel. The TASKS tab groups tasks under the plan they work on, with each plan's header and todos shown above its tasks. Tasks that work on no plan go under a "No Plan" header.
todos:
- id: merge-list
  content: Fold PlanList data into TaskList with grouped rows (Plan/Todo/Task/Item) and internal plan filter
  status: pending
- id: tab-enum
  content: Remove SidePanelTab::Plans and PLANS-only hover/areas/key/click arms
  status: pending
- id: keys-clicks
  content: Enter/p/t/Space and click behavior for plan headers, todos, No Plan and tasks
  status: pending
- id: plans-helpers
  content: Trim app/plans.rs to plan-file helpers; SetPlan switches to TASKS
  status: pending
- id: render
  content: Render plan headers, todos and No Plan header in renderer/tasks.rs; trim renderer/plans.rs; update hints
  status: pending
- id: tests
  content: Update/port unit, header-bar and renderer tests; add grouping and No Plan tests
  status: pending
- id: docs
  content: Update STYLEGUIDE.md and module docs
  status: pending
- id: gate
  content: Run fmt, clippy -D warnings and cargo test -p aura-ui-terminal
  status: pending
---

# Merge PLANS into the TASKS side tab

Drop the separate PLANS tab in the bottom-right side panel. The TASKS tab groups tasks under the plan they work on, with each plan's header and todos shown above its tasks. Tasks that work on no plan go under a "No Plan" header.

## Todos

- [ ] Fold PlanList data into TaskList with grouped rows (Plan/Todo/Task/Item) and internal plan filter
- [ ] Remove SidePanelTab::Plans and PLANS-only hover/areas/key/click arms
- [ ] Enter/p/t/Space and click behavior for plan headers, todos, No Plan and tasks
- [ ] Trim app/plans.rs to plan-file helpers; SetPlan switches to TASKS
- [ ] Render plan headers, todos and No Plan header in renderer/tasks.rs; trim renderer/plans.rs; update hints
- [ ] Update/port unit, header-bar and renderer tests; add grouping and No Plan tests
- [ ] Update STYLEGUIDE.md and module docs
- [ ] Run fmt, clippy -D warnings and cargo test -p aura-ui-terminal

<!-- end of generated header: edit below -->

## Target look

```text
 ▾  Auth rework                       1/3     plan header (fold, name, todo progress)
 ☒  Add session table                         plan todos, as the PLANS tab drew them
 ▣  Write the login handler  → coder
 ☐  Wire the middleware
 ⢎⡱ Build the auth  Lead              1/2     tasks working on the plan (existing tree)
 ☒  Find the handler
   ⢎⡱ Write the login handler → coder  0/2
 ▸  Parser cleanup                    5/5
 ▾  No Plan                                   only when some task has no plan
 ⢎⡱ Fix the login bug  Agent          1/3
```

The tabs become `SANDBOX | PROCESSES | TRACE | TASKS` (short labels `SBX PRC TRC TSK`).

## Approach

**Data: fold `PlanList` into `TaskList`** (`app/tasks.rs`). Right now the PLANS tab has its own list, and `refresh_plan_scope` keeps the two lists in sync. One list that holds both plans and tasks can build the grouped rows in a single `rebuild`.

- New fields on `TaskList`: `plans: Vec<PlanView>`, `current_plan: Option<String>`, `plan_folds: HashMap<String, bool>`, `no_plan_fold: Option<bool>`.
- Move these from `PlanList` into `TaskList`: `set_plans`, `set_current` (renamed `set_current_plan`), `plans`, `is_current`.
- The plan filter is now worked out inside `rebuild` from the scoped tasks, using the same rule as `refresh_plan_scope` (plans the shown tasks work on, plans whose todos they do, and the current plan). `App::refresh_plan_scope` and its calls in `commands.rs` and `side_scope.rs` go away.
- Rows:

```rust
pub enum TaskRow {
    /// A group header: a plan, or `None` for the No Plan group
    Plan { plan: Option<usize>, open: bool },
    /// A todo of an open plan
    Todo { plan: usize, todo: usize },
    Task { task: usize, depth: usize, open: bool },
    Item { task: usize, item: usize, depth: usize },
}
```

`TaskRow::task()` now returns `Option<usize>`. Add `TaskRow::plan() -> Option<usize>`. `selected_key` / keep-highlight also covers plan and todo rows.

**Grouping rules (in `rebuild`)**
- A top-level listed task goes in the group of its own `plan`. Its whole subtree stays with it; spawned tasks already inherit the parent's plan (`aura-app/src/event_loop/tasks.rs`).
- A task whose plan is not loaded or not shown goes under **No Plan**.
- Group order: the current plan first, then the other shown plans newest first, then No Plan last. Plans with no tasks still show their header and todos, so a plan just written in plan mode stays visible. `test_active_plan_shows_in_the_side_panel_not_above_the_input` depends on this.
- Inside an open plan: its todos first, then its task trees. Top-level tasks are drawn at depth 0 (icon under the header's fold arrow), the same as todos, so the narrow panel loses no width.
- Default folds: a plan starts open if it is current, has open todos, or has a task that is not closed. No Plan starts open. User folds stick by path or id. `select_task` also opens the task's group.

**Tab and input**
- `app/processes.rs`: remove `SidePanelTab::Plans` from `ALL`, `label`, `short_label`, `next`, `prev`, and from the Home/End, shortcut, click and hover matches.
- `app/mod.rs`: remove `HoverTarget::PlanRow`, `areas.plan_rows` / `plans_scroll`, the `plan_list` field and the two `move_by` match arms.
- `app/keys.rs`: remove the `Plans` arm.
- `commands.rs` `SetPlan`: a new plan switches to `SidePanelTab::Tasks` instead of Plans.
- `handle_tasks_key` / Enter:
  - On a plan header: open the plan file.
  - On a todo: open the plan at that todo.
  - On No Plan: fold it.
  - On a task or item: show its agent, as today.
- Shortcuts: `p` opens the plan (task, plan or todo rows). `t` on a todo selects the task doing it in the same list, adapted from `jump_to_todo_task`. Space folds.
- `click_task_row`: clicking a header's fold glyph folds it. Clicking elsewhere on a header or todo opens the plan; clicking a task row works as today.
- `app/plans.rs`: keep the plan-file helpers (`checklist_lines`, `todo_line`, `todo_id_line`, `open_plan_at`). Point `open_selected_plan` at `task_list`. Delete `PlanList`, `PlanRow` and the PLANS-only App methods.

**Rendering**
- `renderer/tasks.rs`: draw all four row kinds.
  - A plan header uses `list_row` with the `▾`/`▸` glyph, the plan name in `Tone::active_if(total == 0 || has_open_todos)`, and `closed/total`.
  - "No Plan" is a muted header with no detail.
  - Todo rows use the old `render_plans` todo branch.
  - Empty state stays "No tasks yet." (no plans and no tasks), and `render_hidden("tasks", ...)` stays.
- `renderer/plans.rs`: keep only `marker`, `marker_color` and `todo_style`, which `hud.rs` and `tasks.rs` use. Delete `render_plans`.
- `renderer/sandbox.rs`: remove the `Plans` arm and update the module doc.
- `renderer/side_list.rs`: remove `PLANS_HINT`. `TASKS_HINT` becomes `"⏎ open  p plan  t task  ←→ fold  ⇧←→ tabs"`. Merge the tone table's PLANS and TASKS lines.

## Files to change

| File | Change |
|---|---|
| `crates/ui/aura-ui-terminal/src/app/tasks.rs` | Merged list, grouping, keys, clicks, tests |
| `crates/ui/aura-ui-terminal/src/app/plans.rs` | Drop `PlanList`/`PlanRow`; keep plan-file helpers |
| `crates/ui/aura-ui-terminal/src/app/processes.rs` | Remove `Plans` tab; update the tab-cycle test |
| `crates/ui/aura-ui-terminal/src/app/side_scope.rs` | Drop `refresh_plan_scope`; tests read plan headers from `task_list` |
| `crates/ui/aura-ui-terminal/src/app/{mod,keys,commands}.rs` | Remove plan list, hover, areas and the Plans arms |
| `crates/ui/aura-ui-terminal/src/renderer/{tasks,plans,sandbox,side_list}.rs` | Grouped rendering, helpers only, hints |
| `crates/ui/aura-ui-terminal/src/renderer/header.rs` | Doc comment and bar tests for 4 tabs |
| `crates/ui/aura-ui-terminal/src/renderer/tests.rs` | `PLN` becomes `TSK`; plans-tab test becomes grouped-tasks test; add No Plan test |
| `crates/ui/aura-ui-terminal/STYLEGUIDE.md` | Describe the merged tab |

## Tests

- `app/tasks.rs` unit tests:
  - Tasks group under their plan.
  - Subtrees stay with their root.
  - A no-plan task goes under "No Plan", and that header only appears when such tasks exist.
  - A plan with no tasks still lists its todos.
  - Folds stick across `set_tasks` / `set_plans`.
  - `select_task` opens the group.
  - `t` on a todo selects its task.
  - Enter on a header opens the plan file (tempdir).
- Port the existing PLANS tests that still apply (fold defaults, current plan on top, open at checklist item) to the merged list.
- Header bar tests: recompute the expected strings and widths for 4 tabs. Run the test and update the literals; do not guess them.
- Gate: `cargo fmt --all -- --check`, `cargo clippy -p aura-ui-terminal --all-targets --all-features -- -D warnings`, `cargo test -p aura-ui-terminal`.

## Risks / open choices

- **Showing plan todos above tasks** repeats some information when a task's items mirror the plan. I kept the todos so a fresh plan stays visible. Showing headers only is the other option.
- **No Plan at the bottom:** a running no-plan task can sit below folded plan headers. Each folded plan is one row, and agent scope keeps the list short.
- Persisted UI state: `SidePanelTab` is not serialized, and `aura-desktop` only refers to `SidePanelTab::Trace`, so removing the variant is safe.
