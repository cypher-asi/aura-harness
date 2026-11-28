---
name: 'To-do drawer: active only, agent info, task history'
overview: The to-do drawer above a channel's input lists only pending and in-progress to-dos, with markers that can't be mistaken for one another and the assigned agent's code, status and running time on each. Finished to-dos move into a "Task history" row that is collapsed by default and opens by click or key.
todos:
- id: branch
  content: 'Merge PR #62 (tests on latest main), then move the branch onto the new origin/main'
  status: pending
- id: markers
  content: New state markers in renderer/plans.rs (☐ ◐ ✓ ⊘); update TASKS and drawer tests
  status: pending
- id: agent-info
  content: 'Agent info: match assignee to member/agent, status, in-progress clock; never dropped'
  status: pending
- id: drawer
  content: 'Drawer: active-only rows, header "N active · M done", Task history row (click, Alt+H, /todos history)'
  status: pending
- id: tests
  content: 'Tests: markers, filtering, agent info, history by click/key/slash'
  status: pending
- id: docs
  content: STYLEGUIDE.md and help modal
  status: pending
- id: ship
  content: fmt/clippy/tests aura-ui-terminal + aura-desktop; push, PR, merge latest main, retest, merge
  status: pending
---

# To-do drawer: active only, agent info, task history

The to-do drawer above a channel's input lists only pending and in-progress to-dos, with markers that can't be mistaken for one another and the assigned agent's code, status and running time on each. Finished to-dos move into a "Task history" row that is collapsed by default and opens by click or key.

## Todos

- [ ] Merge PR #62 (tests on latest main), then move the branch onto the new origin/main
- [ ] New state markers in renderer/plans.rs (☐ ◐ ✓ ⊘); update TASKS and drawer tests
- [ ] Agent info: match assignee to member/agent, status, in-progress clock; never dropped
- [ ] Drawer: active-only rows, header "N active · M done", Task history row (click, Alt+H, /todos history)
- [ ] Tests: markers, filtering, agent info, history by click/key/slash
- [ ] STYLEGUIDE.md and help modal
- [ ] fmt/clippy/tests aura-ui-terminal + aura-desktop; push, PR, merge latest main, retest, merge

<!-- end of generated header: edit below -->

## What is wrong today

The drawer (`renderer/todo_drawer.rs`) draws every to-do with the shared box helpers in `renderer/plans.rs`. In progress is `▣`, a box with a filled inner square. The desktop app sketches that as a filled square, so it reads as checked. Done (`☒`) and pending (`☐`) rows sit in the same list. The header counts `done/total`.

The `→ FEN6` label comes from `TodoView::assignee`, which `assign_task` sets from the `assign`/`task` call's `to`, `agent` or `subagent_type`. `todo_line` drops the label whenever the row is narrow, which is why only some rows show it. The label is also the raw text of the call, so it can be a permanent agent's name (`frontend-engineer`) instead of a code.

## New markers (shared: drawer and TASKS tab)

| State | Marker | Colour |
|---|---|---|
| Pending | `☐` | `muted` |
| In progress | `◐` (half-filled circle) | bold `foreground` |
| Done | `✓` | `faint` |
| Cancelled | `⊘` | `faint`, text struck through |

The shapes differ by state, not only by fill, and the desktop app already sketches `◐`, `✓` and `⊘`. The helpers stay in one place, so the TASKS tab keeps matching the drawer, as STYLEGUIDE.md requires.

## The drawer

- **Header:** `⬢ 2 active · 5 done`, with `Working on 2 active` during a turn. `· N done` appears only when something is done. The `—`/`+` fold toggle stays.
- **Active rows (pending, in progress):**
  - Each row is `◐ Fix the redirect  → FEN6 · working 4m`.
  - The agent part is kept whenever there is any room for text; the to-do text is cut first.
  - The assignee is matched to an agent among the open channel's members, else among the agents list, by code and then by permanent agent. That gives its code, working or idle (`AgentSummary::running`), and how long the to-do has been in progress.
  - The time is measured in the UI from when the to-do was first seen in progress (an `Instant` per agent and to-do), so it starts over when the app restarts.
  - An unknown assignee shows as written.
  - In-progress rows stay in view first, as today.
- **History:** a `▸ Task history  5` row under the active rows, folded by default.
  - Opened, it shows `▾` and one row per closed to-do: `✓ Read the parser  → FEN6 · done` or `⊘ …  cancelled`.
  - It toggles on a click on its row, `Alt+H`, or `/todos history`.
  - It is part of the drawer's row budget, and the `+N more` row covers what doesn't fit.
- **Unchanged:** the drawer still shows only while some to-do is open, and `/todos` still brings a finished list back.

## Code

- `renderer/plans.rs`: the new markers.
- `renderer/hud/mod.rs`: `todo_line` and `checklist_header` take agent info and the new counts.
- `renderer/todo_drawer.rs`: its rows (active, history row, history items) move into a new `renderer/todo_drawer/rows.rs` so files stay under 500 lines.
- `app/todo_drawer.rs`: history open or closed, where its row was drawn, the clicks, and the in-progress clock.
- `app/keys`: `Alt+H`. `app/slash/session.rs`: `/todos history`.
- `STYLEGUIDE.md`: the to-do markers, the drawer section and its sketch. `renderer/modals/help.rs` lists the new key.
- Tests: markers per state, active-only rows, agent info (code, status, time, kept on narrow rows, matched by name), and history collapse and expand by click, key and slash. Existing drawer and TASKS tests change to the new markers.

## Branch and merge

This machine can only push `aura/software-factory-rus197`, and that branch still holds PR #62 (the members fold), which isn't merged.

The safe default, following "finished work gets merged":
- merge #62 first with a normal merge commit, once its tests pass on the latest main;
- move the branch onto the new main, which needs no force;
- build this change there and open its own PR;
- bring in the latest main, especially FEN6's mouse-wheel work, rerun the tests for `aura-ui-terminal` and `aura-desktop`, and merge.

If #62 can't merge cleanly, I stop and report.

## Unsure

- Assignments made without a `todo_id` never link a to-do to an agent, so those rows can't show one. This change doesn't alter that.
- The running time is the to-do's time in progress as this app saw it, not the agent's turn time.
