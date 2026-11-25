---
name: 'Channels as their own app: list on the left, chat in the middle, Members and Info on the right'
overview: Make the CHANNELS tab work like a separate app. The left panel becomes a plain list of channels; clicking one opens it as a chat-only middle panel; the right column becomes a single channel panel with MEMBERS and INFO tabs and none of the agent panels.
todos:
- id: app-model
  content: 'Tie the open channel to the CHANNELS tab: leave and reopen on tab switches, remember the previous agent'
  status: completed
  assignee: rust-engineer
- id: left-list
  content: Make the left CHANNELS tab a plain list; remove the open-channel drill-in view and its state
  status: completed
- id: chat-only
  content: 'Guard chat tabs to CHAT in a channel, title the bar #slug, hide the bench and todo overlays'
  status: completed
- id: empty-state
  content: Empty chat state and send guard for CHANNELS with no channel open
  status: completed
- id: side-tabs
  content: Replace SidePanelTab::Projects with ChannelMembers and ChannelInfo and update every match arm
  status: completed
- id: side-render
  content: Render the MEMBERS tree and the INFO view in channel_side.rs; move the context menu to INFO
  status: completed
- id: cleanup
  content: Remove the channel task tree, hide the scope button, update the panel-refusal message
  status: completed
- id: tests
  content: Update and add app and renderer tests, then run fmt, clippy, tests and the aura-desktop check
  status: completed
---

# Channels as their own app: list on the left, chat in the middle, Members and Info on the right

Make the CHANNELS tab work like a separate app. The left panel becomes a plain list of channels; clicking one opens it as a chat-only middle panel; the right column becomes a single channel panel with MEMBERS and INFO tabs and none of the agent panels.

## Todos

- [x] Tie the open channel to the CHANNELS tab: leave and reopen on tab switches, remember the previous agent (@rust-engineer)
- [x] Make the left CHANNELS tab a plain list; remove the open-channel drill-in view and its state
- [x] Guard chat tabs to CHAT in a channel, title the bar #slug, hide the bench and todo overlays
- [x] Empty chat state and send guard for CHANNELS with no channel open
- [x] Replace SidePanelTab::Projects with ChannelMembers and ChannelInfo and update every match arm
- [x] Render the MEMBERS tree and the INFO view in channel_side.rs; move the context menu to INFO
- [x] Remove the channel task tree, hide the scope button, update the panel-refusal message
- [x] Update and add app and renderer tests, then run fmt, clippy, tests and the aura-desktop check

<!-- end of generated header: edit below -->

## How channels work today

All of this lives in `crates/ui/aura-ui-terminal`. Opening a channel (`App::open_channel_by_id` in `app/channels.rs`) does three things:

- It swaps the left CHANNELS list for a drill-in view: a back row, the title and description, and a MEMBERS tree (`renderer/channels.rs::render_open_channel`).
- It makes the coordinator the active agent through the session (`UiEvent::OpenChannel`, which ends in `handle_select_agent`). The chat panel keeps every agent tab (CHAT, TERMINAL, GRAPH, TIME, BENCH, LOG) and only shortens their labels to fit `#slug · coordinator` (`chat_tab_layout` in `app/chat_tabs.rs`).
- It maximizes the side panel over the Kernel panel (`side_maximized` in `app/mod.rs`). That panel puts a channel-only PROJECTS tab in front of SANDBOX, PROCESSES, HOST, TRACE and TASKS (`SidePanelTab::IN_CHANNEL` in `app/processes.rs`, drawn by `renderer/channel_side.rs`).

An open channel also stays open when the user switches to PROJECTS, so the agent layout and the channel layout get mixed.

## The model: the left tab picks the app

The agents panel's tab decides which app is on screen. CHANNELS is the channel app. PROJECTS, ROSTER and ARCHIVE stay the agent app, with today's layout unchanged. One rule holds this together: **a channel is open only while the CHANNELS tab is showing.**

- `show_agents_tab` in `app/definitions.rs` enforces the rule. Leaving CHANNELS closes the open channel and sends `CloseChannel`. It also puts back the agent that was on screen before the channel opened, through `request_agent_switch`, if that agent is still listed.
- Coming back to CHANNELS reopens the last channel, if it is still listed.
- `ChannelState` gains two fields: `last_open`, the channel to reopen, and `agent_before`, the agent to give back.
- If the session shows an agent that is not a member of the open channel (from a notification, say), the UI switches to PROJECTS without asking for another switch. The hook goes in the `SetActiveAgent` handling in `app/agent_views.rs`.
- On CHANNELS with no channel open (first visit, or no channels exist), the chat panel shows a short "Pick a channel" empty state with no tabs, and the right column is hidden. The input placeholder says the same, and `submit_input` in `app/keys.rs` refuses to send, with a notice.

The session in `aura-app` needs no change, because `OpenChannel` and `CloseChannel` already do what this needs.

## Left panel: a plain list

`renderer/channels.rs` keeps only `render_channel_list`, which draws `#slug` rows plus the "+ New channel" row pinned at the bottom, as PROJECTS has "+ New project". `render_open_channel`, `about_lines` and the back row are removed. When the panel is not focused, the open channel's row stays highlighted, the way the agent on screen stays highlighted under PROJECTS.

`app/channels.rs` gets simpler to match:

- These fields and methods go: `selected_row`, `back_area`, `member_rows_area`, `selectable_rows`' open-channel branch, and `channel_row_at`'s open-channel branch.
- Enter and a click only ever open the highlighted channel, or start a new one from "+ New channel".
- Esc and Backspace no longer close the channel. Switching tabs is how you leave.
- The right-click menu on a channel row (Link project, Unlink project, Archive channel) stays, as on project rows.

## Middle panel: chat only

- `show_chat_tab` and `restore_chat_tab` in `app/chat_tabs.rs` only accept `ChatTab::Chat` while a channel is open. Every route to the other tabs goes through these two functions: the tab keys, the `/log`, `/bench` and terminal commands, the bench-progress jump, and an agent's saved tab coming back. So this one guard covers all of them.
- Opening a channel shows CHAT and closes any detail view (`close_detail`).
- `chat_tab_layout` returns `TabLayout::untabbed` in a channel. Clicks and hovers then find no tab, and the desktop's hosted chrome gets an empty tab list.
- `render_chat_panel` in `renderer/panels.rs` titles the bar `#slug`, with the channel's title as muted info. This replaces `coordinator_chat_title`. The coordinator's history and the channel posts still render as they do now.
- The agent overlays in the chat column are hidden in the channel app: the bench panel (`bench_panel::height` returns 0) and the todo panel (`hud::render_todo_panel`). The queue panel stays, because it holds the user's own queued messages.
- The input placeholder becomes "Message #slug". The member-first `@` picker in `app/mentions.rs` stays.

## Right column: one channel panel with MEMBERS and INFO

The mechanism that makes the side panel fill the column while a channel is open (`side_maximized` and `sandbox_panel_visible`) stays as it is. Only the tabs change.

- In `app/processes.rs`, `SidePanelTab::Projects` is replaced by `ChannelMembers` (label `MEMBERS`, short `MBR`) and `ChannelInfo` (label `INFO`, short `INF`). `IN_CHANNEL` becomes those two tabs only, so SANDBOX, PROCESSES, HOST, TRACE and TASKS never show in a channel. Opening a channel selects MEMBERS. `show_side_tab` refuses both channel tabs when no channel is open.
- The Kernel panel (KERNEL, MEMBERS, POLICIES, INBOX) stays hidden for the whole time a channel is open, as it is today.
- `/kernel`, `/files`, `/inbox`, `/members` and `/policies` keep refusing in a channel. `notify_channel_owns_column` gets a new message: "Channels have only a chat; switch to PROJECTS for agent panels".
- `renderer/sandbox.rs` hides the `*` scope button in a channel, as it already hides the expand button. Both are agent features.
- `renderer/channel_side.rs` is rewritten into two views:
  - **MEMBERS** draws the member tree that was in the left panel: the coordinator first, spawned agents nested under spawn guides, a spinner while one works, and its code on the right. `member_row` and `member_code` move here from `renderer/channels.rs`. Up, Down, PageUp, PageDown, Home, End, the wheel and hover move the highlight. Rows do nothing when activated, because switching to a member's own chat would leave the channel app.
  - **INFO** shows `#slug`, the bold title, the wrapped description, the coordinator's name and code, a member count, and LINKED PROJECTS with each project's folder. Right-clicking anywhere in INFO opens the channel's menu. `open_channel_side_menu` in `app/context_menu.rs` moves from the PROJECTS tab to INFO.
- The channel task tree (`task_list`, `tasks_scroll`, `handle_channel_side_key`, `move_channel_task` and the rest) is removed from the UI. `UiCommand::SetChannelTasks` becomes a documented no-op. The session can stop sending it in a follow-up.
- In `app/mod.rs`, `app/keys.rs` and `app/processes.rs`, every match arm on `SidePanelTab::Projects` (scroll, keys, clicks, hover, the list shortcuts) moves to the two new tabs. In `app/agent_views.rs`, the trace-feed redraw test no longer counts an open channel.

## Tests

The channel tests in `app/tests.rs` (from about line 5600) and `renderer/tests.rs` (from about line 6430) are updated, and new tests are added for:

- Clicking or pressing Enter on a list row opens the channel and keeps the list on screen. No back row or members are drawn on the left.
- In a channel, the chat tabs refuse TERMINAL, GRAPH, TIME, BENCH and LOG, whether asked for by key, by command or by a saved tab. The bar shows `#slug` and no tabs.
- The right column lists exactly MEMBERS and INFO, with MEMBERS selected. Shift+Left and Shift+Right cycle between those two only. No Kernel-panel or agent side tabs are reachable.
- Switching to PROJECTS closes the channel, sends `CloseChannel` and asks for the agent that was on screen before. Switching back reopens the last channel.
- CHANNELS with nothing open shows the empty state, hides the right column and refuses to send.
- The renderer draws the MEMBERS tree and the INFO contents.

The gates are `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test -p aura-ui-terminal`, and `cargo check -p aura-desktop` for the hosted chrome.

## Choices made, to confirm

- "+ New channel" and the right-click menu stay on the list, because PROJECTS has both. Removing them would leave nowhere to create, link or archive a channel except slash commands.
- Member rows on MEMBERS can be highlighted but do nothing when activated.
- The channel's task tree is dropped rather than moved into INFO.
- Leaving CHANNELS closes the channel and gives back the previous agent. Returning reopens the last channel, but nothing opens on the first visit.

## What could go wrong

- The session switches to the coordinator asynchronously after `OpenChannel`. For a moment the chat-only panel may still show the previous agent's history. The member check on `SetActiveAgent` has to let that in-flight switch finish and not treat it as leaving the channel.
- Removing `Projects` from `SidePanelTab` is a breaking change to a public enum. The compiler will flag every match, including any in `aura-desktop`.
