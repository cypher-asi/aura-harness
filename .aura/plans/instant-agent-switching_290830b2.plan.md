---
name: Instant agent switching
overview: 'Make agent switches feel instant: the UI swaps in its cached chat right away, and the backend stops re-reading and rebuilding history the UI already has. Store reads move off the event loop and are batched, and state for removed or archived agents, plus long-unused agents, gets evicted.'
todos:
- id: ui-optimistic
  content: 'aura-ui-terminal: optimistic local switch in request_agent_switch, AgentView.loaded flag, send FocusAgent for loaded views else SelectAgent, clear records locally; update app tests'
  status: pending
- id: set-records
  content: 'aura-ui events: add UiCommand::SetRecords(Vec<RecordSummary>) handled as clear+replace in commands.rs; route it in aura-desktop router like ClearRecords'
  status: pending
- id: backend-load
  content: 'aura-app: refactor record_loader to one scan_tail in spawn_blocking deriving both summaries and history; reorder show_active_agent to send SetActiveAgent and cheap state first; use SetRecords; skip history on Chat::Keep'
  status: pending
- id: backend-io
  content: 'aura-app: move registry save off the loop (spawn_blocking/debounce), cache created sandbox dirs, send_agent_list after activation error for UI rollback'
  status: pending
- id: evict-state
  content: Prune UI views on SetAgents + LRU cap of non-busy views; backend removes archived/deleted agents from state.idle and LRU-caps idle sessions
  status: pending
- id: tests-verify
  content: Update event_loop/router tests for new command sequence; add tests for optimistic swap, rollback, pruning, LRU, single-scan; run fmt, clippy -D warnings, tests for aura-ui-terminal, aura-app, aura-desktop
  status: pending
---

## Why switching is slow today

1. **The UI waits for the backend.** `request_agent_switch` (`aura-ui-terminal/src/app/mod.rs:2305`) only sends `UiEvent::SelectAgent`. Nothing changes on screen until the backend's `UiCommand::SetActiveAgent` comes back.
2. **The cache is thrown away.** The terminal app already keeps one `AgentView` per agent (`app/agent_views.rs`), including its `ChatLineCache`. But `SelectAgent` goes to `activate_agent(.., Chat::Reload)` (`aura-app/src/event_loop/agents.rs:370`). For an idle agent, `show_active_agent` then sends `LoadConversation`. `cmd_load_conversation` (`commands.rs:1098`) clears the messages and rebuilds them, which invalidates the line cache on every switch.
3. **Blocking reads on the async loop.** `show_active_agent` (`agents.rs:797`):
   - calls `recent_record_summaries` (scan_tail 100) and `conversation_history` (scan_tail 1000, with a JSON decode of every entry), inline with no `spawn_blocking`;
   - makes about 100 awaited `NewRecord` sends, one at a time;
   - does a synchronous registry `save()` in `set_active`;
   - runs `create_dir_all` on every switch.

   This breaks rules.md §6.1.
4. **Old state is never removed.**
   - In the UI, `App.views` never drops agents that were deleted or archived (`cmd_set_agents` does not prune it).
   - In the backend, `state.idle` keeps an open kernel/session for every agent ever visited. It is only cleaned up on project removal (`agents.rs:593`).

The desktop router already turns `SelectAgent` into `FocusAgent` (`Chat::Keep`) when a tab holds the agent (`aura-desktop/src/router.rs:496-547`). The terminal UI should do the same.

## Changes

### A. UI: switch on screen first (aura-ui-terminal)
- `request_agent_switch`:
  - Call `show_agent_chat(&id)` and the rest of `cmd_set_active_agent`'s local updates right away: the `is_active` flags, `select_visible_active`, and the flow refresh.
  - Then send the event:
    - `UiEvent::FocusAgent(id)` if the agent's `AgentView` is marked `loaded`;
    - otherwise `UiEvent::SelectAgent(id)`, so the backend sends history once.
- Add `loaded: bool` to `AgentView`. It is set when `LoadConversation` is applied, or when any live chat command for that agent was processed.
- The backend's later `SetActiveAgent(id)` for the same id becomes a no-op. `show_agent_chat` already returns early if the id is unchanged.
- **Rollback:** if the backend can't activate the agent (open fails), it now ends with `send_agent_list()`. That sends `SetActiveAgent(previous)`, which swaps back.
- **Record panel:** on optimistic switch, clear `records` locally so the previous agent's records don't stay on screen.

### B. Backend: cheap activation (aura-app event_loop)
- `show_active_agent`:
  1. Send `SetActiveAgent` first. Then send `SetSession`, mode, instructions and plans, which are cheap in-memory sends.
  2. After that, load the store data.
- Store reads:
  - Do a single `scan_tail(1000)` inside `tokio::task::spawn_blocking`.
  - Derive both the record summaries (last 100) and the conversation history from that one scan, by refactoring `record_loader.rs` (`conversation_history_from(&[RecordEntry])` and `summaries_from(&[RecordEntry])`).
  - Skip the history part entirely for `Chat::Keep`.
- Replace the about 100 `NewRecord` sends with one new `UiCommand::SetRecords(Vec<RecordSummary>)`. It is handled in `commands.rs` as clear-and-replace, keeping the selection rules of `cmd_new_record`. Desktop routes it to the active tab, like `ClearRecords`.
- `registry.set_active`: keep the update in memory, but move the `save()` into `spawn_blocking`, or debounce it. A failure is logged as before.
- `send_sandbox_root`: remember the workspaces already created in a `HashSet<AgentId>` on `LoopState`, and skip `create_dir_all` for those.
- `activate_agent` error path: after `show_error`, call `hub.send_agent_list()` so the UI rolls back.

### C. Remove old state
- **UI:**
  - In `cmd_set_agents`, `views.retain` only ids present in `agents`, which drops deleted and archived agents.
  - Cap the number of non-busy cached views with a small LRU, about 16 entries, tracked by a last-shown `Instant`. An evicted view loses `loaded`, so the next switch sends `SelectAgent` and reloads.
- **Backend:**
  - When agents are archived or deleted (`after_list_shrank` and the archive handlers), remove them from `state.idle` and the new sandbox set.
  - Cap idle, non-running, non-active sessions with a matching LRU, about 16, dropping the oldest kernels.

## Risks
- **Stale cached chat if history changed outside the live stream** (for example compaction or an external writer). Mitigation: any backend path that rewrites history sends `LoadConversation` for that agent explicitly, wrapped in `ForAgent`, so cached views update.
- **Ordering:** `SetRecords` and `LoadConversation` arrive after `SetActiveAgent`. Their `ForAgent` routing must target the right view if the user has already switched again. `LoadConversation` is already wrapped by `AgentUi`; send `SetRecords` with the agent id and ignore it if it doesn't match the active agent (as `cmd_new_record` does).
- **Tests:** existing tests assert the exact `SetActiveAgent` → `ClearRecords` → `NewRecord` sequence (`event_loop/tests.rs:984-991, 2360-2374`; `app/tests.rs` `SelectAgent` asserts). Update them to `SetRecords`, and to `FocusAgent` for views already loaded.

## Verification
- `cargo test -p aura-ui-terminal -p aura-app -p aura-desktop`
- `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all -- --check`
- New tests:
  - the optimistic swap happens before any backend reply;
  - loaded view → `FocusAgent`, never-loaded → `SelectAgent`;
  - rollback on activation error;
  - `views` pruned on `SetAgents`;
  - LRU eviction in UI and backend;
  - a single scan produces both summaries and history.
- Optional: extend `aura-desktop/src/shell/bench.rs` / `renderer/bench.rs` with a switch benchmark (a cached switch should not rebuild `ChatLineCache`).