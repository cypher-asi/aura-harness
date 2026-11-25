---
name: Finish a plan when its todos are done
overview: Once every todo in the agent's list is closed, mark the plan it was building as complete and drop the list, so the checklist stops showing at the bottom of the chat. The agent then has no active plan, and the user can run /plan, then /build, again.
todos:
- id: finish-fn
  content: Add finish_done_todos and unfinished_todos in plan_mode.rs; call it from handle_agent_success
  status: completed
- id: restore-paths
  content: Apply unfinished_todos on checkpoint restore, history rebuild and rewind; filter finished plans out of latest_plan_path
  status: completed
- id: live-tail
  content: Add Hud::has_open_todos and hide the live-tail checklist once every todo is closed
  status: completed
- id: tasks-badge
  content: Show a check mark on finished plan headers in the TASKS tab
  status: completed
- id: tests
  content: Add app and renderer tests, then run fmt, clippy and the aura-app and aura-ui-terminal tests
  status: completed
---

# Finish a plan when its todos are done

Once every todo in the agent's list is closed, mark the plan it was building as complete and drop the list, so the checklist stops showing at the bottom of the chat. The agent then has no active plan, and the user can run /plan, then /build, again.

## Todos

- [x] Add finish_done_todos and unfinished_todos in plan_mode.rs; call it from handle_agent_success
- [x] Apply unfinished_todos on checkpoint restore, history rebuild and rewind; filter finished plans out of latest_plan_path
- [x] Add Hud::has_open_todos and hide the live-tail checklist once every todo is closed
- [x] Show a check mark on finished plan headers in the TASKS tab
- [x] Add app and renderer tests, then run fmt, clippy and the aura-app and aura-ui-terminal tests

<!-- end of generated header: edit below -->

## How it works today

When you run `/build`, the plan's todos become the agent's todo list (`session.todos` in `crates/surface/aura-app/src/event_loop/plan_mode.rs`), `building_plan` is set to true, and every `todo_write` is copied into the plan file. Nothing ever ends that state. Four things go wrong once all the todos are done:

- The fully checked list stays in `session.todos`. It is handed to every later turn as `initial_todos`, so `live_tail` in `crates/ui/aura-ui-terminal/src/renderer/panels.rs` draws "Working on 8 to-dos" with every box ticked on each new turn. It also shows for the rest of the current turn, which is what your screenshot shows.
- `building_plan` stays true, so `request_plan` in `turns.rs` keeps filing new requests under the finished plan. Worse, `sync_plan` in `agent_events.rs` copies any new, unrelated todo list into the finished plan's file and overwrites its todos.
- `active_plan` still points at the finished plan, so typing "implement" or a bare `/build` builds it again instead of telling you to make a new one.
- The same list comes back after a restart or rewind. `todos_from_history`, the checkpoint restore and `latest_plan_path` all rebuild it.

## Finishing the plan at the end of a turn

Add `finish_done_todos(session, ui)` to `plan_mode.rs`. Call it from `handle_agent_success` in `handlers.rs`, right after `session.todos.clone_from(&result.todos)` and before `SetTodos` is sent. If the list is not empty and every item `is_closed()` (completed or cancelled), it does the following:

- If the session was building `active_plan`, it posts a system message such as "Plan complete: <name>". It then sets `building_plan` to false and `active_plan` to `None`, sends `SetPlans(plan_views(&workspace))` so the TASKS tab shows the final state, and sends `SetPlan(None)` so the plan is no longer marked as current.
- It always clears `session.todos`. The `SetTodos(vec![])` that follows already closes `/todos` and resets the expanded state in the UI (`commands.rs`, around line 979).

The checkpoint is saved after this, so it stores the empty list. Because `active_plan` is now `None`, "implement" and a bare `/build` show the usual "No plan to build yet. Switch to plan mode with /plan…" warning. The next `/plan` writes a new plan instead of revising the old one, since `open_plan` already ignores finished plans. `/build <path>` still works for any plan.

Add a small helper, `unfinished_todos(Vec<TodoItem>) -> Vec<TodoItem>`, that returns an empty list when every item is closed. Use it wherever a list is rebuilt, so a finished list does not come back:

- In `AgentSession::restore`, in `agents.rs`, for `checkpoint.todos`.
- At the `todos_from_history` call sites in `agents.rs` (around line 380) and `rewind.rs` (around line 97).
- When the session is built, filter `latest_plan_path(...)` in `agents.rs` (around line 351) so a finished plan (it has todos and all are closed) does not become `active_plan` again. A plan with no todos yet stays, as it does now.

## Hiding the checklist as soon as it is done

The list is only cleared when the turn ends, but the agent often keeps working after ticking the last box. Add `Hud::has_open_todos()` in `crates/ui/aura-ui-terminal/src/app/hud.rs`, which is true when any item is pending or in progress. In `live_tail`, build the checklist only when the app is processing and `has_open_todos()` is true. If the agent adds new todos, the checklist shows again.

## Showing that a plan is complete

In `plan_line` in `crates/ui/aura-ui-terminal/src/renderer/tasks.rs`, a plan whose todos are all closed already draws in the muted style. Show `✓ 8/8` instead of `8/8` so finished plans are easy to spot in the TASKS tab.

## Tests

- In `event_loop/tests/open_plan.rs`, a build turn whose scripted `todo_write` closes the last todo should leave `building_plan` false, `todos` empty and `active_plan` as `None`. The UI should get `SetTodos([])`, `SetPlan(None)` and the "Plan complete" message. A request after that should be kept as `(None, false)`, and its todo list must not change the plan file.
- Write unit tests for `unfinished_todos` and for the `latest_plan_path` filter.
- In `renderer/tests.rs`, the live tail should show no checklist while processing when every todo is closed. It should still show one when a todo is open.
- In `renderer/tasks.rs` tests, a finished plan's header should end in `✓ n/n`.
- Finish with `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test -p aura-app -p aura-ui-terminal`.

## What could go wrong

Clearing a finished list also affects todo lists that were not part of a plan, including subagents' lists. That is intended: a fully closed list carries nothing forward. The model still sees its old `todo_write` calls in its history, so it loses no context. Headless runs report todos from their own events, and this change leaves them alone.
