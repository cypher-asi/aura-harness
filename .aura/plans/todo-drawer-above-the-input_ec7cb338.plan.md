---
name: Todo drawer above the input
overview: Move the agent's to-do checklist out of the chat's live tail into a collapsible drawer docked at the bottom of the Chat column, shown whenever the session on screen has an active to-do list.
todos:
- id: drawer-state
  content: Add app/todo_drawer.rs with fold state, header area, hover and mouse handling
  status: completed
- id: drawer-render
  content: Add renderer/todo_drawer.rs with height() and render() modeled on queue_panel.rs
  status: completed
- id: layout
  content: Insert the drawer under the chat body in renderer/mod.rs and budget its height
  status: completed
- id: mouse
  content: Route mouse events and pointer state to the drawer in app/mouse.rs
  status: completed
- id: remove-inline
  content: Remove the checklist from LiveTail and the TodoChecklist toggle target
  status: completed
- id: remove-overlay
  content: Delete render_todo_panel and todo_checklist from hud.rs; make /todos fold the drawer
  status: completed
- id: docs
  content: Update slash_menu description and STYLEGUIDE.md for the drawer
  status: completed
- id: tests
  content: Replace checklist tests with drawer tests; run fmt, clippy and tests
  status: completed
---

# Todo drawer above the input

Move the agent's to-do checklist out of the chat's live tail into a collapsible drawer docked at the bottom of the Chat column, shown whenever the session on screen has an active to-do list.

## Todos

- [x] Add app/todo_drawer.rs with fold state, header area, hover and mouse handling
- [x] Add renderer/todo_drawer.rs with height() and render() modeled on queue_panel.rs
- [x] Insert the drawer under the chat body in renderer/mod.rs and budget its height
- [x] Route mouse events and pointer state to the drawer in app/mouse.rs
- [x] Remove the checklist from LiveTail and the TodoChecklist toggle target
- [x] Delete render_todo_panel and todo_checklist from hud.rs; make /todos fold the drawer
- [x] Update slash_menu description and STYLEGUIDE.md for the drawer
- [x] Replace checklist tests with drawer tests; run fmt, clippy and tests

<!-- end of generated header: edit below -->

## How it works today

The to-do list is drawn in two places in `crates/ui/aura-ui-terminal`. While a turn is in flight, `live_tail` in `renderer/chat_panel.rs` puts `hud::todo_checklist` (the `⬢ Working on N to-dos` header, up to ten items, then `N more` or `show less`) at the end of the conversation, above the `⬢ Thinking…` line. It scrolls with the chat, and clicking it flips `App::todos_expanded` through `ToggleTarget::TodoChecklist`. Separately, `/todos` sets `todos_open`, and `hud::render_todo_panel` draws a bordered `TODOS 1/3` overlay over the bottom of the chat.

The queued-messages panel (`renderer/queue_panel.rs` with its state in `app/queue_panel.rs`) and the benchmark panel already show the pattern we want: a strip between the Chat body and the input, sharing the column's borders through `input::join_divider`, with a `—`/`+` toggle in its header that folds it to one row. The to-do drawer will be a third panel of that kind.

## What the drawer looks like and when it shows

```text
├──────────────────────────────────────────────────────────┤
│ ⬢ Working on 24 to-dos  19/24                          — │
│ ☒ Files and functions pass: aura-ui and aura-ui-terminal │
│ ▣ Files and functions pass: app, node, cli, desktop, ... │
│ ☐ Replace remaining anyhow in library crates with ...    │
│ +2 more                                                  │
├──────────────────────────────────────────────────────────┤
│ → Add a follow-up                                        │
```

- The drawer shows whenever the agent on screen has a to-do list with at least one open item, during a turn and between turns, so it lasts as long as the list is active in the session. A list with every item closed hides it, as the live tail does today. The channel app and screens with no agent never show it.
- The header reads `⬢ Working on N to-dos` with `Working` shimmering while a turn runs, and `⬢ N to-dos` when idle, then the `done/total` count and the `—`/`+` toggle at the right, styled like the queue panel's header.
- Folded, the drawer is just the header row, with the in-progress item's text after the count when it fits, so the current step stays visible.
- Open, it lists up to `MAX_TODO_ROWS` items, one row each, using the existing `todo_line`. Long lists show a window that keeps the in-progress item in view (the logic already in `render_todo_panel`), with a `+N more` row for the rest.
- Clicking the header or its toggle folds and opens the drawer. `/todos` does the same, and on a finished list it brings the drawer back to show the closed items until toggled away or a new list arrives.
- The fold state persists across turns; a new empty list (`SetTodos(vec![])`) resets it to open, as `todos_expanded` resets today.

## Layout and height budget

`renderer/mod.rs::render` splits the Chat column into body, bench, queue, input and footer. The drawer goes right under the chat body, above the bench and queue panels, so the queue stays next to the input it belongs to. It gives up rows before the queue and bench do: its height is clamped to what is left after the chat keeps three rows, the input one, and the queue and bench take theirs.

## Changes by file

- Add `renderer/todo_drawer.rs` with `height(app)` and `render(frame, area, app, theme)`, built like `queue_panel.rs`: left, right and bottom borders, `join_divider`, a header row, and item rows. It records the header area on the `App` for the mouse.
- Add `app/todo_drawer.rs` with a small `TodoDrawer` state (folded flag, header area, hovered flag) and `todo_drawer_mouse`, `toggle_todo_drawer`, `todo_drawer_folded`, plus a pointer-state getter so hovering the toggle redraws.
- Wire the drawer into `renderer/mod.rs` (new row in the Chat column layout and height budget) and `app/mouse.rs` (call `todo_drawer_mouse` next to `queue_mouse`, and include its pointer state in the redraw check).
- Remove the checklist from the chat in `renderer/chat_panel.rs`: `LiveTail` loses `checklist_rows` and `checklist_footer`, and `render_chat_lines` stops adding the checklist toggle rows and hover.
- Remove `ToggleTarget::TodoChecklist`, `todos_expanded` and `todos_open` from `app/mod.rs`, replacing them with the drawer state; update `/todos` in `app/slash.rs`, the reset in `app/commands.rs` (`SetTodos`), and line 271 of `app/slash.rs`.
- In `renderer/hud.rs`, delete `render_todo_panel`, `todo_checklist`, `TodoChecklist` and `todo_lines_in_full`, keep `todo_line` and `checklist_header` (adapted for the idle label) for the drawer, and update the module doc.
- Update the `/todos` description in `app/slash_menu.rs` to "Fold or open the to-do drawer".
- Update `STYLEGUIDE.md`: replace the live-tail checklist mockup and the "Menus and the `/todos` list" border note with the drawer, and describe its header and fold toggle.

## Tests

- Replace `test_todo_checklist_and_list`, `a_todo_list_with_every_item_closed_leaves_the_live_tail` and `test_clicking_the_todo_checklist_lists_every_item` in `renderer/tests.rs` with drawer tests: the drawer sits between the chat body and the input, shows when idle and during a turn, hides when every item is closed, folds to one row on a header click and on `/todos`, keeps the in-progress item in view in a long list, and never appears in the chat transcript rows.
- Replace `test_clicking_the_todo_checklist_expands_and_collapses_it` in `app/tests.rs` with a toggle test on the drawer state, including the reset on an empty list.
- Keep `test_todos_stay_with_their_agent` passing, now without needing `/todos` to see the list.
- Replace the `todo_checklist` unit tests in `hud.rs` with tests for the drawer's header and rows in `todo_drawer.rs`, including a squeezed height budget on a short terminal.
- Check `crates/surface/aura-desktop/src/shell/snapshot.rs`, which asserts `Working on 3 to-dos` on a rendered screen; it should still pass because the drawer keeps that header, but adjust it if the snapshot is taken idle.
- Run `cargo fmt`, `cargo clippy -p aura-ui-terminal --all-targets -- -D warnings` and `cargo test -p aura-ui-terminal -p aura-desktop`.

## What could go wrong

The drawer takes rows from the chat on short terminals; the height clamp and the fold toggle cover that, and the test with a small screen guards it. Removing `todos_open` and `todos_expanded` touches public `App` getters, so any other crate calling them has to move to the new drawer getters; a workspace build will catch that.
