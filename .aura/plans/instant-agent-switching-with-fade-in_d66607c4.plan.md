---
name: Instant agent switching with fade-in
overview: Switch the chat to the chosen agent as soon as the user picks it, without waiting for the session to answer. A chat already held in memory shows at once with no fade. A chat that has to be loaded shows a blank (then "Loading chat…") panel and fades in when its history arrives.
todos:
- id: commands
  content: Add UiCommand::ChatReady and UiCommand::SwitchFailed in aura-ui events.rs
  status: pending
- id: view-state
  content: Add loaded, loading_since and revealed_at to AgentView and swap them in swap_chat
  status: pending
- id: optimistic
  content: Make request_agent_switch switch locally and send FocusAgent (loaded) or SelectAgent (not loaded)
  status: pending
- id: echo
  content: Ignore stale SetActiveAgent echoes; handle LoadConversation, ChatReady and SwitchFailed
  status: pending
- id: render
  content: Draw the loading placeholder and fade the chat area, using the fade helper shared with run_log
  status: pending
- id: desktop
  content: 'Desktop: add chat_animating to busy(), route tab Select/Focus and SwitchFailed in the router, update demo'
  status: pending
- id: session
  content: 'Session: SwitchFailed on open error, ChatReady for running agents, reload on re-select, send the chat first'
  status: pending
- id: tests
  content: Add UI, renderer, router and session tests; run fmt, clippy and tests
  status: pending
---

# Instant agent switching with fade-in

Switch the chat to the chosen agent as soon as the user picks it, without waiting for the session to answer. A chat already held in memory shows at once with no fade. A chat that has to be loaded shows a blank (then "Loading chat…") panel and fades in when its history arrives.

## Todos

- [ ] Add UiCommand::ChatReady and UiCommand::SwitchFailed in aura-ui events.rs
- [ ] Add loaded, loading_since and revealed_at to AgentView and swap them in swap_chat
- [ ] Make request_agent_switch switch locally and send FocusAgent (loaded) or SelectAgent (not loaded)
- [ ] Ignore stale SetActiveAgent echoes; handle LoadConversation, ChatReady and SwitchFailed
- [ ] Draw the loading placeholder and fade the chat area, using the fade helper shared with run_log
- [ ] Desktop: add chat_animating to busy(), route tab Select/Focus and SwitchFailed in the router, update demo
- [ ] Session: SwitchFailed on open error, ChatReady for running agents, reload on re-select, send the chat first
- [ ] Add UI, renderer, router and session tests; run fmt, clippy and tests

<!-- end of generated header: edit below -->

## Why it waits today

- `App::request_agent_switch` (`crates/ui/aura-ui-terminal/src/app/mod.rs:3320`) only sends `UiEvent::SelectAgent`. Nothing on screen changes until the session replies with `UiCommand::SetActiveAgent`.
- On the session side, `activate_agent` (`crates/surface/aura-app/src/event_loop/agents.rs:792`) first runs `open_session` and `registry.set_active`, which are synchronous. Only then does `show_active_agent` send `SetActiveAgent`. After that it reads the records, the traces and the conversation history from the store before it sends `LoadConversation`.
- The UI already keeps every agent's chat in memory (`AgentView` in `app/agent_views.rs`, kept live through `ForAgent` commands). But `SelectAgent` always asks for `Chat::Reload`, so a chat that is already in memory is rebuilt anyway.

## Approach

1. **Optimistic local switch.** `request_agent_switch` switches the chat on screen right away for any agent in `self.agents`. It sends the event first, then calls `cmd_set_active_agent(id)`, so any LOG, GRAPH or BENCH requests that the switch sends reach the session after the switch. An agent that is not listed (for example, one that belongs to another desktop tab) keeps today's behaviour.
2. **In memory = no reload, no fade.** Each chat gets a `loaded: bool` flag. It is set by `cmd_load_conversation`, swapped in `swap_chat`, and true for the chat loaded at startup. A switch to a loaded chat sends `UiEvent::FocusAgent`, which the session handles as `Chat::Keep`, and draws it at once. A switch to a chat that is not loaded sends `SelectAgent` and puts the chat into a loading state.
3. **Loading, then fade-in.** Each chat gets `loading_since` and `revealed_at` (`Option<Instant>`), swapped with the rest of the view. While a chat is loading, the panel draws no welcome screen and no old content. After `LOADING_DELAY` it shows a dim "Loading chat…". When the chat's `LoadConversation` or the new `ChatReady` arrives, `revealed_at = now`, and the panel fades from the background colour over `FADE_IN` (180 ms). Both constants already exist in `app/run_log.rs`.
4. **Ignore stale echoes.** `switch_sent: Option<String>` holds the switch the UI is waiting on. When `SetActiveAgent(x)` arrives:
   - if `x == sent`, clear `switch_sent`; the screen is already on `x`, so nothing else runs;
   - if it is a different agent, ignore it (it answers an earlier request, as in A→B→A);
   - if no switch is pending, apply it as today.

   `confirmed_agent` remembers the session's last active agent, so the UI can go back to it.
5. **The session always answers a select.** Two new commands make sure the UI never waits forever:
   - `UiCommand::SwitchFailed(String)`: `open_session` failed. The UI goes back to `confirmed_agent`.
   - `UiCommand::ChatReady` (sent inside `ForAgent`): the agent is running, so its live chat is left as it is. This ends the loading state.

## Files to change

| File | Change |
|------|--------|
| `crates/ui/aura-ui/src/events.rs` | Add `UiCommand::ChatReady` and `UiCommand::SwitchFailed(String)`, with docs |
| `crates/ui/aura-ui-terminal/src/app/agent_views.rs` | Add `loaded`, `loading_since` and `revealed_at` to `AgentView` and `swap_chat`. Add a helper that starts loading when the incoming chat is not loaded |
| `crates/ui/aura-ui-terminal/src/app/mod.rs` | New fields (`chat_loaded`, `chat_loading_since`, `chat_revealed_at`, `switch_sent`, `confirmed_agent`). Rewrite `request_agent_switch`. Add `chat_loading()`, `chat_fading()` and `chat_animating()` |
| `crates/ui/aura-ui-terminal/src/app/commands.rs` | Add the `SetActiveAgent` echo rules. `cmd_load_conversation` sets `loaded` and reveals a loading chat. Handle `ChatReady` and `SwitchFailed` |
| `crates/ui/aura-ui-terminal/src/animation/mod.rs` | Move `fade_in` here from `renderer/run_log.rs`. Add `fade_area(buf, rect, progress, theme)`, which fades the cells of an area |
| `crates/ui/aura-ui-terminal/src/renderer/run_log.rs` | Use the shared fade helper |
| `crates/ui/aura-ui-terminal/src/renderer/panels.rs` | In `render_chat_panel`, before the `pane_is_empty` and welcome check, return a blank or "Loading chat…" panel while loading. After `paint_user_band`, fade `body`, and the smooth-scroll row below it, while fading |
| `crates/surface/aura-desktop/src/shell.rs` | Add `front.chat_animating()` to `busy()` so frames keep coming while the chat loads or fades |
| `crates/surface/aura-desktop/src/router.rs` | See "Desktop router" below |
| `crates/surface/aura-desktop/src/demo.rs` | Send `ChatReady` when `show_agent(reload)` sends no history |
| `crates/surface/aura-app/src/event_loop/agents.rs` | See "Session" below |

### Desktop router

`forward()` currently asks the session for `shown_by(from)` before every event. After an optimistic switch, `shown_by(from)` is already the new agent, so it would send `FocusAgent(new)` ahead of `SelectAgent(new)`, and the session would then skip the reload. The router changes so that:

- a tab's own `SelectAgent` goes through `load_into(from, agent)`;
- a tab's own `FocusAgent` goes through `request(agent, false)`;
- if the session already has that agent active and nothing is in flight, the router passes `SetActiveAgent` straight to the tab as the reply;
- `SwitchFailed` goes to `owner(agent)` and clears `focus_sent`.

### Session

In `activate_agent` and `show_active_agent`:

- **Already active.** If the agent is already active and `chat == Chat::Reload`, run `show_active_agent` instead of only sending the list.
- **Open fails.** If `open_session` fails, send `SwitchFailed(hex)` after the error.
- **Agent running.** If `chat == Reload` and the agent is running, send `ForAgent { ChatReady }`.
- **Order.** Send the chat (`LoadConversation`) right after `SetActiveAgent` and the agent list, before the records and traces, so the content the user is looking at arrives first.

## Risks and notes

- **Side panels.** For the moment before the session confirms, the Record, Plans and Sandbox panels can still show the previous agent. They are replaced once `show_active_agent` resends them. `NewRecord` from the old agent is already filtered out by `agent_id`.
- **Existing tests.** Tests that assert "until the switch lands" only use agents that are not listed, so they should keep passing. Tests where the target chat was already loaded will now see `FocusAgent` and need updating.
- **Terminal redraws.** The plain terminal runner needs to redraw while the fade runs. Check that its tick loop redraws without input, as it already does for the LOG fade.

## Tests

- **UI (`app/tests.rs`, `agent_views.rs`):**
  - The switch is immediate for an agent that is not loaded: `SelectAgent` is sent, the chat is loading, then `LoadConversation` makes it fade.
  - A loaded agent gets `FocusAgent`, with no loading and no fade.
  - A→B→A drops the stale `SetActiveAgent(B)`.
  - `SwitchFailed` goes back to the previous agent.
  - `ChatReady` ends loading.
  - An agent that is not listed keeps the old behaviour.
- **Renderer (`renderer/tests.rs`):** no welcome screen while loading, the placeholder appears only after `LOADING_DELAY`, and the fade runs from the background colour to the full colours.
- **Router (`router.rs` tests):** a tab's own select or focus is sent once and in the right order, and the router replies itself when the session is already on that agent.
- **Session (`event_loop/tests.rs`):** extend the busy-agent test (line 1310) to expect `ChatReady`. Add tests for `SwitchFailed` and for reloading on a re-select.
- **Gates:** `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all --all-features`.
