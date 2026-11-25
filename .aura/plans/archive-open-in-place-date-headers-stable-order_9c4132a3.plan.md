---
name: 'Archive: open in place, date headers, stable order'
overview: Clicking or pressing Enter on an archived agent opens its chat on the right and leaves it in the Archive. Only Unarchive brings it back to the list. The Archive gets grouped under headers for the day each agent was archived, sorted by that date so sending a message never moves a row. You can chat with an archived agent without unarchiving it.
todos:
- id: registry-archived-at
  content: Add archived_at, make archive() a no-op when already archived, add archive_at/current(), sort archived_tree newest first
  status: pending
- id: rows-active
  content: Send archived_at_ms; use current() in ListRows/agent_summaries; mark active and plan mode on archived rows
  status: pending
- id: shrink-active
  content: 'after_list_shrank: set the next agent active before sending lists'
  status: pending
- id: summary-field
  content: Add AgentSummary::archived_at_ms in aura-ui events
  status: pending
- id: ui-archive-rows
  content: ArchiveRow model with day headers; click/Enter open the agent; selection by id; keys skip headers
  status: pending
- id: ui-render
  content: Render day headers and the active highlight in build_archived_lines
  status: pending
- id: ui-unarchive-cmd
  content: Add /unarchive slash command; mark_active_agent covers archived rows
  status: pending
- id: tests
  content: Registry, event-loop, UI and desktop router tests; update order assertions
  status: pending
- id: gates
  content: Run fmt, clippy -D warnings, and tests for the touched crates
  status: pending
- id: commit
  content: 'Commit (blocked: another agent is mid-edit in aura-ui-terminal, tree does not compile)'
  status: pending
---

# Archive: open in place, date headers, stable order

Clicking or pressing Enter on an archived agent opens its chat on the right and leaves it in the Archive. Only Unarchive brings it back to the list. The Archive gets grouped under headers for the day each agent was archived, sorted by that date so sending a message never moves a row. You can chat with an archived agent without unarchiving it.

## Todos

- [ ] Add archived_at, make archive() a no-op when already archived, add archive_at/current(), sort archived_tree newest first
- [ ] Send archived_at_ms; use current() in ListRows/agent_summaries; mark active and plan mode on archived rows
- [ ] after_list_shrank: set the next agent active before sending lists
- [ ] Add AgentSummary::archived_at_ms in aura-ui events
- [ ] ArchiveRow model with day headers; click/Enter open the agent; selection by id; keys skip headers
- [ ] Render day headers and the active highlight in build_archived_lines
- [ ] Add /unarchive slash command; mark_active_agent covers archived rows
- [ ] Registry, event-loop, UI and desktop router tests; update order assertions
- [ ] Run fmt, clippy -D warnings, and tests for the touched crates
- [ ] Commit (blocked: another agent is mid-edit in aura-ui-terminal, tree does not compile)

<!-- end of generated header: edit below -->

## What happens today

- `click_archived_row` and Enter in `handle_archive_key` (`crates/ui/aura-ui-terminal/src/app/archive.rs`) call `restore_selected()`. That sends `UiEvent::RestoreAgent` and switches to the Projects tab.
- `AgentRegistry::archived_tree` (`crates/surface/aura-app/src/agent_registry.rs`) orders rows by project, then by registry order. Nothing records when an agent was archived.
- The backend can already open an archived agent: `activate_agent` has no "listed" check. But `ListRows::build` (`record_loader.rs`) marks as active `registry.active()`, which skips archived agents. Every `send_agent_list` then sends `SetActiveAgent(<first listed agent>)`, so the screen would jump away from an archived chat as soon as a message refreshes the lists. Fixing this is what lets you keep talking to an archived agent.

## Approach

### 1. Registry: record when, sort by when (`agent_registry.rs`)
- Add `archived_at: Option<DateTime<Utc>>` to `RegisteredAgent` (`#[serde(default, skip_serializing_if = "Option::is_none")]`, so older files still load).
- `archive()`: if the agent is already archived, do nothing (return `Ok`). This stops `/archive` on an archived agent from resetting its date and moving it. Otherwise set `archived = true` and `archived_at = now`. Add a `pub(crate) fn archive_at(id, at)` that `archive()` calls, so tests can pass fixed times.
- `restore()`: clear `archived_at` along with `archived` on every agent it brings back.
- `archived_tree()`: collect the heads (archived agents that no archived agent spawned), sort them by `archived_at` newest first with a stable sort, put agents with no date (from before this change) last, and keep registry order for ties. Then visit each head as today. Only archiving changes this order, never activity. Agents an archived agent spawns later go under it.
- Add `pub fn current(&self) -> &RegisteredAgent`: the stored active agent even if it is archived, falling back to `active()`. `active()` keeps its startup behaviour (it never opens on an archived agent).

### 2. Rows sent to the UI (`record_loader.rs`, `event_loop/subagents.rs`, `event_loop/agents.rs`)
- `aura-ui/src/events.rs`: add `#[serde(default)] pub archived_at_ms: Option<u64>` to `AgentSummary`.
- `rows()`: fill `archived_at_ms` from `agent.archived_at`.
- `ListRows::build` and `Hub::agent_summaries`: use `registry.current()` instead of `active()`, and pass that agent as `active` to the archived rows too (not `unknown`). The archived agent on screen then stays active and its Archive row is marked.
- `mark_plan_mode`: also apply it to archived rows, since they can run turns now (update the comment).
- `after_list_shrank`: when the active agent is among `gone`, call `registry.set_active(next)` before `send_agent_list()`. The list sent right after `/archive` then still points at the next agent, as it does today.

### 3. Terminal/desktop UI (`aura-ui-terminal`)
- **`app/archive.rs`**
  - Add a display-row model: `enum ArchiveRow { Day(String), Agent { index: usize } }`, built by `archive_rows()`. A `Day` header comes before each new day of a head row's `archived_at_ms`. Agents spawned under a head stay in its group. Labels are `Today`, `Yesterday`, then `%Y-%m-%d` (local time, the same format as project dates), and `Earlier` for rows with no date. Put the label logic in a pure function `archive_day(ms, now: DateTime<Local>)`.
  - `selected_archived` stays an index into `archived`. Scrolling, `sync_archived_scroll` and `archived_row_at` work in display rows and skip headers. A click on a header does nothing.
  - Up/Down/PageUp/PageDown/Home/End only land on agent rows.
  - Clicking a row and Enter call a new `open_archived_selected()`, which calls `request_agent_switch(id)` and stays on the Archive tab. Restoring now happens only through the menu's Unarchive item and `/unarchive`.
  - `set_archived()` keeps the highlight on the same agent id, not the same index, so a refresh never moves the cursor.
- **`app/commands.rs` + `slash_menu.rs`**: add `/unarchive`, which restores the active agent when it is in `archived`, and warns "Not archived" otherwise. `mark_active_agent` also updates `is_active` on `self.archived`.
- **`renderer/panels.rs`**: `build_archived_lines` draws from `archive_rows()`. Day headers use the projects list's header style (muted, bold, not selectable). When the panel is not focused, the row of the agent on screen (`is_active`) gets the highlight, as in the projects list.
- **`app/context_menu.rs`**: keep the Archived menu as `[Unarchive]` (`MenuItem::Restore`). It still restores and shows Projects, because that is the explicit way out of the Archive.

### 4. Desktop routing (`crates/surface/aura-desktop/src/router.rs`)
Opening an archived agent already works, through the path for agents that are not listed: `SetActiveAgent` is held until the next `SetAgents`, then shown in the front tab. No code change is expected. Add a test that selecting an archived agent from a tab shows it in that tab and later lists don't move it away.

## Files to change

| File | Change |
|---|---|
| `crates/surface/aura-app/src/agent_registry.rs` | `archived_at`, idempotent `archive`, `archive_at`, sorted `archived_tree`, `current()` |
| `crates/surface/aura-app/src/record_loader.rs` | `archived_at_ms`, `current()`, active mark on archived rows |
| `crates/surface/aura-app/src/event_loop/agents.rs` | `after_list_shrank` sets the next agent active first |
| `crates/surface/aura-app/src/event_loop/subagents.rs` | `current()` in `agent_summaries`, plan mark on archived rows |
| `crates/ui/aura-ui/src/events.rs` | `AgentSummary::archived_at_ms` |
| `crates/ui/aura-ui-terminal/src/app/archive.rs` | row model, open on click/Enter, selection by id |
| `crates/ui/aura-ui-terminal/src/app/commands.rs`, `slash_menu.rs` | `/unarchive`, active mark in the Archive |
| `crates/ui/aura-ui-terminal/src/renderer/panels.rs` | day headers, active highlight |
| `crates/surface/aura-desktop/src/router.rs` | test only |

## Tests
- Registry: `archived_tree` lists newest first, by fixed `archive_at` times. Agents with no date come last. Archiving an already-archived agent keeps its date and place. Restoring clears `archived_at`. An old file without the field still loads. Update the existing order assertions in `agent_registry/tests.rs` and `record_loader/tests.rs` for the new order.
- Event loop: `SelectAgent(archived)` makes it active. A message sent to it runs, and after the turn the lists still send `SetActiveAgent(archived)`. The agent stays archived and its Archive order is unchanged. Archiving the active agent still opens the next one (the existing test passes).
- UI: clicking a row and Enter send `SelectAgent`, keep `AgentsTab::Archive`, and send no `RestoreAgent`. Headers render, and keys and clicks skip them. The highlight follows the id across `SetArchivedAgents`. `/unarchive` sends `RestoreAgent(active)`. Unit tests for `archive_day`. Rewrite `the_archive_lists_archived_agents_and_restores_them`.
- Gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test -p aura-app -p aura-ui -p aura-ui-terminal -p aura-desktop`.

## Risks
- Other UI code (chat header, project info, `/project`) may assume the active agent is in `agents`. Check what it shows for an archived agent and make sure it degrades to "no project" without errors.
- Switching to an archived agent waits for the session's answer (the existing path for agents that are not listed), so it is a little less instant than for listed agents. If needed later, `request_agent_switch` can treat `archived` ids as listed.
- Agents archived before this change have no date and sit under `Earlier`. We could fill in a date from their last activity instead.
