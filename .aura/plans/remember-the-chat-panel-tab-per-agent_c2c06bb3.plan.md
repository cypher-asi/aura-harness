---
name: Remember the chat panel tab per agent
overview: Each agent keeps the chat panel tab it was last left on (CHAT, TERMINAL, GRAPH, TIME, BENCH, LOG). Switching to it brings that tab back, and an agent that has never been shown opens on CHAT.
todos:
- id: agent-view-tab
  content: Add `chat_tab` to `AgentView`; `show_agent_chat` saves the outgoing tab and returns the incoming one
  status: pending
- id: restore-fn
  content: Split `show_chat_tab` into `set_chat_tab` + `restore_chat_tab` (no scroll, no requests)
  status: pending
- id: switch-restore
  content: Restore the tab in `cmd_set_active_agent`; keep LOG override; drop bench-run rule; ask once
  status: pending
- id: update-tests
  content: Update `picking_a_run_shows_its_chat_once` and `opening_the_tab_asks_for_the_flow_of_the_agent_on_screen`
  status: pending
- id: new-tests
  content: Add per-agent tab tests (restore, single ask, scroll kept, background untouched, log override)
  status: pending
- id: docs
  content: Update doc comments, `docs/cli.md` and CHANGELOG
  status: pending
- id: verify
  content: Run fmt, clippy -D warnings, and tests for aura-ui-terminal and aura-desktop
  status: pending
---

# Remember the chat panel tab per agent

Each agent keeps the chat panel tab it was last left on (CHAT, TERMINAL, GRAPH, TIME, BENCH, LOG). Switching to it brings that tab back, and an agent that has never been shown opens on CHAT.

## Todos

- [ ] Add `chat_tab` to `AgentView`; `show_agent_chat` saves the outgoing tab and returns the incoming one
- [ ] Split `show_chat_tab` into `set_chat_tab` + `restore_chat_tab` (no scroll, no requests)
- [ ] Restore the tab in `cmd_set_active_agent`; keep LOG override; drop bench-run rule; ask once
- [ ] Update `picking_a_run_shows_its_chat_once` and `opening_the_tab_asks_for_the_flow_of_the_agent_on_screen`
- [ ] Add per-agent tab tests (restore, single ask, scroll kept, background untouched, log override)
- [ ] Update doc comments, `docs/cli.md` and CHANGELOG
- [ ] Run fmt, clippy -D warnings, and tests for aura-ui-terminal and aura-desktop

<!-- end of generated header: edit below -->

## Current behavior

The chat panel's tab is global. It is derived from `terminal_mode`, `flow.is_open()`/`flow.mode()`, `bench.is_open()` and `log.is_open()` (`ChatTab` in `app/chat_tabs.rs`). When an agent switch lands (`cmd_set_active_agent` in `app/commands.rs`), the tab on screen carries over to the next agent. There are two exceptions: a pending "open this agent's log" request shows LOG, and picking a benchmark run while on BENCH shows CHAT.

Per-agent chat state already exists. `AgentView` (`app/agent_views.rs`) holds the messages, scroll and HUD of each agent that is not on screen, and `show_agent_chat` swaps them in and out.

## Approach

- Add `chat_tab: ChatTab` to `AgentView`. Its `Default` is `ChatTab::Chat`, which gives "default to chat" for any agent never shown.
- In `show_agent_chat`, store `self.chat_tab()` in the outgoing agent's view and return the incoming agent's tab as `Option<ChatTab>`. It returns `None` when nothing was swapped (same agent, or no agent on screen before).
- Leave `swap_chat` alone. `process_agent_command` uses it for background agents, so their output never changes the tab on screen.
- In `cmd_set_active_agent`, restore that tab without scrolling or sending requests, then ask once for what the tab shows.

## Behavior decisions

- **Scope:** only the chat panel's tabs. The agents, KERNEL/MEMBERS and side panel tabs stay global.
- **Lifetime:** in memory for the session, and per `App`, so per desktop window tab. Nothing is persisted across restarts, which matches the scroll position and folds.
- **Explicit log request wins:** `open_log_of` (for example `L` on a bench run) still opens LOG on the target agent, whatever tab it was left on.
- **Drop the bench-run rule** ("a run is picked to watch it"). A run that was never shown now opens on CHAT by default anyway. A run that was revisited now gets its own remembered tab, as requested.
- **Scroll is preserved:** restoring does not call `scroll_chat_to_bottom`, so each agent's chat comes back where it was.
- **Same agent re-sent** (list refreshes): there is no switch, so the tab is untouched. This is unchanged.

## Code sketch

`app/chat_tabs.rs`: split the flag-setting out of `show_chat_tab`:

```rust
/// Make `tab` the panel's view: flags, flow mode, input history.
fn set_chat_tab(&mut self, tab: ChatTab) {
    self.terminal_mode = tab == ChatTab::Terminal;
    self.flow.set_open(matches!(tab, ChatTab::Graph | ChatTab::Time));
    self.bench.set_open(tab == ChatTab::Bench);
    self.log.set_open(tab == ChatTab::Log);
    self.history().reset();
    match tab {
        ChatTab::Graph => self.flow.set_mode(FlowMode::Canvas),
        ChatTab::Time => self.flow.set_mode(FlowMode::Timeline),
        ChatTab::Chat | ChatTab::Terminal | ChatTab::Bench | ChatTab::Log => {}
    }
}

/// Put the panel back on the tab the agent coming on screen was left on:
/// the chat keeps its scroll; the switch asks for what the tab shows.
pub(super) fn restore_chat_tab(&mut self, tab: ChatTab) {
    if tab != self.chat_tab() {
        self.set_chat_tab(tab);
    }
}
```

`show_chat_tab` then becomes: record `flow_was_open`, `set_chat_tab(tab)`, `clear_selection`, `scroll_chat_to_bottom`, and the same per-tab requests as today.

`app/commands.rs`: `cmd_set_active_agent`:

```rust
let switched = agent_id != self.active_agent_id;
let left_on = self.show_agent_chat(agent_id);
self.refresh_side_scope();
if self.log_wanted_for(agent_id) {
    self.show_chat_tab(super::ChatTab::Log);
} else {
    if let Some(tab) = left_on {
        self.restore_chat_tab(tab);
    }
    if switched && self.log.is_open() {
        self.request_log();
    }
}
if self.flow.is_open() { self.request_flow(); }      // once, for the agent on screen
if self.bench.is_open() { self.request_benchmarks(); }
// rest unchanged (is_active flags, open_ancestors, select_visible_active)
```

This also removes the `bench_run` lookup and its branch.

## Files to change

| File | Change |
|---|---|
| `crates/ui/aura-ui-terminal/src/app/agent_views.rs` | `chat_tab` field and default; `show_agent_chat` saves and returns the tab; module and field docs |
| `crates/ui/aura-ui-terminal/src/app/chat_tabs.rs` | `set_chat_tab` and `restore_chat_tab`; `show_chat_tab` uses the shared setter |
| `crates/ui/aura-ui-terminal/src/app/commands.rs` | `cmd_set_active_agent` restores the tab; drop the bench-run rule; update comments |
| `crates/ui/aura-ui-terminal/src/app/flow/mod.rs` | `open_flow_agent` doc: the agent opens on its own tab, CHAT the first time |
| `crates/ui/aura-ui-terminal/src/app/benchmarks.rs` | `picking_a_run_shows_its_chat_once`: "another agent keeps BENCH" becomes CHAT; switching back to `user` gives BENCH |
| `crates/ui/aura-ui-terminal/src/app/flow/tests.rs` | `opening_the_tab_asks_for_the_flow_of_the_agent_on_screen`: CODER opens on CHAT with no request; on GRAPH it asks for its own flow; LEAD comes back on GRAPH with exactly one `OpenFlow(LEAD)` |
| `crates/ui/aura-ui-terminal/src/app/tests.rs` | New tests (below) |
| `docs/cli.md` | Chat panel tabs paragraph and the GRAPH `o` bullet |
| `CHANGELOG.md` | `[Unreleased]` entry "Each agent keeps its chat panel tab" (Changed) |

## New tests (`app/tests.rs`, using `app_with_agents()`)

- `each_agent_comes_back_on_the_chat_tab_it_was_left_on`: aa on TERMINAL, then bb opens on CHAT. bb on LOG, then aa is back on TERMINAL (`terminal_pane()`). Re-sending aa keeps TERMINAL. bb is back on LOG.
- `an_agent_coming_back_asks_once_for_what_its_tab_shows`: GRAPH sends exactly one `OpenFlow`. BENCH sends exactly List and Sessions. CHAT sends nothing. TIME comes back with `FlowMode::Timeline`.
- `an_agent_coming_back_keeps_its_chat_scroll`: `scroll_offset` survives aa to bb to aa.
- `background_output_leaves_the_tab_on_screen_alone`: `ForAgent { bb, .. }` while aa is on GRAPH leaves aa on GRAPH.
- `asking_for_an_agents_log_wins_over_its_tab`: bb left on TERMINAL, then `open_log_of(bb)` lands on LOG.
- Confirm that `turns_are_open_until_folded_and_stay_folded_as_the_flow_grows` still passes. Folds and selection now survive through the restored GRAPH tab.

## Risks

- Tests elsewhere might assume the tab carries across agents. A search found only the two listed above. `aura-desktop` tests drive the same `App`, so run them too.
- `from_timeline`, which makes Esc go back to TIME, is not restored after a round trip. Esc on a restored GRAPH goes to CHAT. This is minor and acceptable.
